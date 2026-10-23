//! Workflow cache and workflow run queries.

use chrono::{DateTime, Utc};
use rusqlite::{Row, params};

use crate::codec::*;
use crate::error::{Result, StoreError};
use crate::rows::{WorkflowCacheRow, WorkflowRunRow, WorkflowStepRow};
use crate::store::Store;

const COLS: &str =
    "id, name, session_id, state, args_json, started_at, finished_at, result_json, error";

fn map(r: &Row<'_>) -> rusqlite::Result<WorkflowRunRow> {
    Ok(WorkflowRunRow {
        id: r.get(0)?,
        name: r.get(1)?,
        session_id: r.get(2)?,
        state: r.get(3)?,
        args_json: r.get(4)?,
        started_at: get_ts(r, 5)?,
        finished_at: get_opt_ts(r, 6)?,
        result_json: r.get(7)?,
        error: r.get(8)?,
    })
}

impl Store {
    pub fn get_workflow_cache(&self, key_hash: &str) -> Result<Option<WorkflowCacheRow>> {
        let conn = self.conn();
        let mut st = conn.prepare(
            "SELECT key_hash, workflow, result_json, created_at FROM workflow_cache \
             WHERE key_hash = ?1",
        )?;
        let mut rows = st.query_map([key_hash], |r| {
            Ok(WorkflowCacheRow {
                key_hash: r.get(0)?,
                workflow: r.get(1)?,
                result_json: r.get(2)?,
                created_at: get_ts(r, 3)?,
            })
        })?;
        Ok(rows.next().transpose()?)
    }

    pub fn put_workflow_cache(&self, row: &WorkflowCacheRow) -> Result<()> {
        self.conn().execute(
            "INSERT OR REPLACE INTO workflow_cache (key_hash, workflow, result_json, created_at) \
             VALUES (?1,?2,?3,?4)",
            params![
                row.key_hash,
                row.workflow,
                row.result_json,
                ts_to_sql(&row.created_at)
            ],
        )?;
        Ok(())
    }

    /// Deletes cache entries created before `ts`; returns how many were removed.
    pub fn prune_workflow_cache_before(&self, ts: DateTime<Utc>) -> Result<usize> {
        Ok(self.conn().execute(
            "DELETE FROM workflow_cache WHERE created_at < ?1",
            [ts_to_sql(&ts)],
        )?)
    }

    pub fn insert_workflow_run(&self, w: &WorkflowRunRow) -> Result<()> {
        self.insert_workflow_run_owned(w, None)
    }

    /// Inserts the workflow and its initiating run together. None is a depth-0 owner.
    pub fn insert_workflow_run_owned(
        &self,
        w: &WorkflowRunRow,
        owner_run_id: Option<&str>,
    ) -> Result<()> {
        self.conn().execute(
            &format!("INSERT INTO workflow_runs ({COLS}, owner_run_id) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)"),
            params![
                w.id,
                w.name,
                w.session_id,
                w.state,
                w.args_json,
                ts_to_sql(&w.started_at),
                opt_ts_to_sql(&w.finished_at),
                w.result_json,
                w.error,
                owner_run_id,
            ],
        )?;
        Ok(())
    }

    /// The initiating run, absent for orchestrator and legacy workflows.
    pub fn get_workflow_owner(&self, id: &str) -> Result<Option<String>> {
        Ok(self.conn().query_row(
            "SELECT owner_run_id FROM workflow_runs WHERE id = ?1",
            [id],
            |r| r.get(0),
        )?)
    }

    /// Ends a run: the final state with its result (on success) or error, and the finish
    /// time, in one write so a reader never sees a finished run without its outcome.
    pub fn finish_workflow_run(
        &self,
        id: &str,
        state: &str,
        finished_at: DateTime<Utc>,
        result_json: Option<&str>,
        error: Option<&str>,
    ) -> Result<()> {
        let n = self.conn().execute(
            "UPDATE workflow_runs SET state = ?2, finished_at = ?3, result_json = ?4, \
             error = ?5 WHERE id = ?1",
            params![id, state, ts_to_sql(&finished_at), result_json, error],
        )?;
        if n == 0 {
            return Err(StoreError::NotFound {
                entity: "workflow run",
                id: id.into(),
            });
        }
        Ok(())
    }

    /// Fails a run that is still `running` (and cancels its open steps); a run that already
    /// ended is left alone. Returns whether it changed anything.
    pub fn fail_workflow_if_running(&self, id: &str, error: &str) -> Result<bool> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let ts = ts_to_sql(&chrono::Utc::now());
        let n = tx.execute(
            "UPDATE workflow_runs SET state = 'failed', finished_at = ?2, error = ?3 \
             WHERE id = ?1 AND state = 'running'",
            params![id, ts, error],
        )?;
        if n > 0 {
            tx.execute(
                "UPDATE workflow_steps SET state = 'cancelled', finished_at = ?2 \
                 WHERE workflow_run_id = ?1 AND state IN ('running','waiting','stalled')",
                params![id, ts],
            )?;
        }
        tx.commit()?;
        Ok(n > 0)
    }

    /// Fails the runs a previous supervisor left `running` (it died with them) and cancels
    /// their open steps. Returns how many runs were failed.
    pub fn fail_orphaned_workflows(&self, now: DateTime<Utc>) -> Result<usize> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let ts = ts_to_sql(&now);
        tx.execute(
            "UPDATE workflow_steps SET state = 'cancelled', finished_at = ?1 \
             WHERE state IN ('running','waiting','stalled') AND workflow_run_id IN \
             (SELECT id FROM workflow_runs WHERE state = 'running')",
            [&ts],
        )?;
        let n = tx.execute(
            "UPDATE workflow_runs SET state = 'failed', finished_at = ?1, \
             error = 'the supervisor stopped while the workflow was running; run it again to resume from its cached steps' \
             WHERE state = 'running'",
            [&ts],
        )?;
        tx.commit()?;
        Ok(n)
    }

    /// Newest first, after the `(started_at, id)` position `after` when given.
    pub fn list_workflow_runs_page(
        &self,
        after: Option<(DateTime<Utc>, &str)>,
        limit: usize,
    ) -> Result<Vec<WorkflowRunRow>> {
        let conn = self.conn();
        let rows = match after {
            Some((ts, id)) => {
                let mut st = conn.prepare(&format!(
                    "SELECT {COLS} FROM workflow_runs WHERE (started_at, id) < (?1, ?2) \
                     ORDER BY started_at DESC, id DESC LIMIT ?3"
                ))?;
                st.query_map(params![ts_to_sql(&ts), id, limit as i64], map)?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            }
            None => {
                let mut st = conn.prepare(&format!(
                    "SELECT {COLS} FROM workflow_runs ORDER BY started_at DESC, id DESC LIMIT ?1"
                ))?;
                st.query_map([limit as i64], map)?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            }
        };
        Ok(rows)
    }

    /// Whether `session_id` has a workflow that is still running.
    pub fn session_has_running_workflow(&self, session_id: &str) -> Result<bool> {
        Ok(self.conn().query_row(
            "SELECT EXISTS(SELECT 1 FROM workflow_runs WHERE session_id = ?1 AND state = 'running')",
            [session_id],
            |r| r.get(0),
        )?)
    }

    /// Finds the single workflow run whose id starts with `prefix`.
    pub fn get_workflow_run_by_prefix(&self, prefix: &str) -> Result<WorkflowRunRow> {
        let not_found = || StoreError::NotFound {
            entity: "workflow run",
            id: prefix.into(),
        };
        if prefix.is_empty() {
            return Err(not_found());
        }
        let esc = prefix
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let conn = self.conn();
        let mut st = conn.prepare(&format!(
            "SELECT {COLS} FROM workflow_runs WHERE id LIKE ?1 ESCAPE '\\' LIMIT 2"
        ))?;
        let mut found = st
            .query_map([format!("{esc}%")], map)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        match found.len() {
            0 => Err(not_found()),
            1 => Ok(found.remove(0)),
            _ => Err(StoreError::AmbiguousPrefix {
                entity: "workflow run",
                prefix: prefix.into(),
            }),
        }
    }

    /// Sets the state; a `Some` `finished_at` stamps the finish time.
    pub fn update_workflow_run_state(
        &self,
        id: &str,
        state: &str,
        finished_at: Option<DateTime<Utc>>,
    ) -> Result<()> {
        let fin = opt_ts_to_sql(&finished_at);
        let n = self.conn().execute(
            "UPDATE workflow_runs SET state = ?2, finished_at = COALESCE(?3, finished_at) \
             WHERE id = ?1",
            params![id, state, fin],
        )?;
        if n == 0 {
            return Err(StoreError::NotFound {
                entity: "workflow run",
                id: id.into(),
            });
        }
        Ok(())
    }

    /// Puts a finished run back to `running` for a re-run: new args, no finish time.
    pub fn reset_workflow_run(&self, id: &str, args_json: &str) -> Result<()> {
        let n = self.conn().execute(
            "UPDATE workflow_runs SET state = 'running', args_json = ?2, finished_at = NULL, \
             result_json = NULL, error = NULL WHERE id = ?1",
            params![id, args_json],
        )?;
        if n == 0 {
            return Err(StoreError::NotFound {
                entity: "workflow run",
                id: id.into(),
            });
        }
        Ok(())
    }

    pub fn get_workflow_run(&self, id: &str) -> Result<Option<WorkflowRunRow>> {
        let conn = self.conn();
        let mut st = conn.prepare(&format!("SELECT {COLS} FROM workflow_runs WHERE id = ?1"))?;
        let mut rows = st.query_map([id], map)?;
        Ok(rows.next().transpose()?)
    }

    /// Newest first.
    pub fn list_workflow_runs(&self, limit: usize) -> Result<Vec<WorkflowRunRow>> {
        let conn = self.conn();
        let mut st = conn.prepare(&format!(
            "SELECT {COLS} FROM workflow_runs ORDER BY started_at DESC, id DESC LIMIT ?1"
        ))?;
        let rows = st.query_map([limit as i64], map)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

const STEP_COLS: &str = "workflow_run_id, step_index, phase, prompt_hash, run_id, state, \
                         started_at, finished_at";

fn map_step(r: &Row<'_>) -> rusqlite::Result<WorkflowStepRow> {
    Ok(WorkflowStepRow {
        workflow_run_id: r.get(0)?,
        step_index: r.get(1)?,
        phase: r.get(2)?,
        prompt_hash: r.get(3)?,
        run_id: r.get(4)?,
        state: r.get(5)?,
        started_at: get_ts(r, 6)?,
        finished_at: get_opt_ts(r, 7)?,
    })
}

impl Store {
    /// Inserts the step, or replaces it when the run is being re-run.
    pub fn insert_workflow_step(&self, s: &WorkflowStepRow) -> Result<()> {
        self.conn().execute(
            &format!(
                "INSERT INTO workflow_steps ({STEP_COLS}) VALUES (?1,?2,?3,?4,?5,?6,?7,?8) \
                 ON CONFLICT (workflow_run_id, step_index) DO UPDATE SET \
                 phase = excluded.phase, prompt_hash = excluded.prompt_hash, \
                 run_id = excluded.run_id, state = excluded.state, \
                 started_at = excluded.started_at, finished_at = excluded.finished_at"
            ),
            params![
                s.workflow_run_id,
                s.step_index,
                s.phase,
                s.prompt_hash,
                s.run_id,
                s.state,
                ts_to_sql(&s.started_at),
                opt_ts_to_sql(&s.finished_at)
            ],
        )?;
        Ok(())
    }

    /// Sets the state; `Some` run id / finish time overwrite, `None` keeps the old value.
    pub fn update_workflow_step(
        &self,
        workflow_run_id: &str,
        step_index: i64,
        state: &str,
        run_id: Option<&str>,
        finished_at: Option<DateTime<Utc>>,
    ) -> Result<()> {
        let fin = opt_ts_to_sql(&finished_at);
        let n = self.conn().execute(
            "UPDATE workflow_steps SET state = ?3, run_id = COALESCE(?4, run_id), \
             finished_at = COALESCE(?5, finished_at) \
             WHERE workflow_run_id = ?1 AND step_index = ?2",
            params![workflow_run_id, step_index, state, run_id, fin],
        )?;
        if n == 0 {
            return Err(StoreError::NotFound {
                entity: "workflow step",
                id: format!("{workflow_run_id}#{step_index}"),
            });
        }
        Ok(())
    }

    /// Marks steps still `running`, `waiting` or `stalled` as `cancelled`; returns them
    /// (as they were before) so callers can report the change.
    pub fn cancel_open_workflow_steps(
        &self,
        workflow_run_id: &str,
        finished_at: DateTime<Utc>,
    ) -> Result<Vec<WorkflowStepRow>> {
        let open: Vec<WorkflowStepRow> = self
            .list_workflow_steps(workflow_run_id)?
            .into_iter()
            .filter(|s| matches!(s.state.as_str(), "running" | "waiting" | "stalled"))
            .collect();
        self.conn().execute(
            "UPDATE workflow_steps SET state = 'cancelled', finished_at = ?2 \
             WHERE workflow_run_id = ?1 AND state IN ('running','waiting','stalled')",
            params![workflow_run_id, ts_to_sql(&finished_at)],
        )?;
        Ok(open)
    }

    /// Removes steps with `step_index >= from` (leftovers of an earlier, longer run).
    pub fn delete_workflow_steps_from(&self, workflow_run_id: &str, from: i64) -> Result<usize> {
        Ok(self.conn().execute(
            "DELETE FROM workflow_steps WHERE workflow_run_id = ?1 AND step_index >= ?2",
            params![workflow_run_id, from],
        )?)
    }

    /// Ordered by step index.
    pub fn list_workflow_steps(&self, workflow_run_id: &str) -> Result<Vec<WorkflowStepRow>> {
        let conn = self.conn();
        let mut st = conn.prepare(&format!(
            "SELECT {STEP_COLS} FROM workflow_steps WHERE workflow_run_id = ?1 \
             ORDER BY step_index"
        ))?;
        let rows = st.query_map([workflow_run_id], map_step)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use so_core::new_id;

    #[test]
    fn cache_roundtrip() {
        let s = Store::open_in_memory().unwrap();
        assert!(s.get_workflow_cache("k").unwrap().is_none());
        let mut row = WorkflowCacheRow {
            key_hash: "k".into(),
            workflow: "wf".into(),
            result_json: "1".into(),
            created_at: crate::codec::now(),
        };
        s.put_workflow_cache(&row).unwrap();
        row.result_json = "2".into();
        s.put_workflow_cache(&row).unwrap();
        assert_eq!(s.get_workflow_cache("k").unwrap().unwrap(), row);
        let later = row.created_at + chrono::Duration::seconds(1);
        assert_eq!(s.prune_workflow_cache_before(row.created_at).unwrap(), 0);
        assert_eq!(s.prune_workflow_cache_before(later).unwrap(), 1);
        assert!(s.get_workflow_cache("k").unwrap().is_none());
    }

    #[test]
    fn workflow_runs() {
        let s = Store::open_in_memory().unwrap();
        let w = WorkflowRunRow {
            id: new_id(),
            name: "wf".into(),
            session_id: Some("s".into()),
            state: "running".into(),
            args_json: "{}".into(),
            started_at: crate::codec::now(),
            finished_at: None,
            result_json: None,
            error: None,
        };
        s.insert_workflow_run(&w).unwrap();
        assert_eq!(s.get_workflow_run(&w.id).unwrap().unwrap(), w);
        s.update_workflow_run_state(&w.id, "succeeded", Some(crate::codec::now()))
            .unwrap();
        let got = s.get_workflow_run(&w.id).unwrap().unwrap();
        assert_eq!(got.state, "succeeded");
        assert!(got.finished_at.is_some());
        assert_eq!(s.list_workflow_runs(5).unwrap().len(), 1);
        assert!(matches!(
            s.update_workflow_run_state("nope", "x", None),
            Err(StoreError::NotFound { .. })
        ));
    }

    #[test]
    fn workflow_steps() {
        let s = Store::open_in_memory().unwrap();
        let w = WorkflowRunRow {
            id: new_id(),
            name: "wf".into(),
            session_id: None,
            state: "running".into(),
            args_json: "null".into(),
            started_at: crate::codec::now(),
            finished_at: None,
            result_json: None,
            error: None,
        };
        s.insert_workflow_run(&w).unwrap();
        let mk = |i: i64| WorkflowStepRow {
            workflow_run_id: w.id.clone(),
            step_index: i,
            phase: (i == 0).then(|| "plan".to_string()),
            prompt_hash: format!("h{i}"),
            run_id: None,
            state: "running".into(),
            started_at: crate::codec::now(),
            finished_at: None,
        };
        s.insert_workflow_step(&mk(1)).unwrap();
        s.insert_workflow_step(&mk(0)).unwrap();
        s.update_workflow_step(&w.id, 0, "succeeded", Some("r1"), Some(crate::codec::now()))
            .unwrap();
        let steps = s.list_workflow_steps(&w.id).unwrap();
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].step_index, 0);
        assert_eq!(steps[0].state, "succeeded");
        assert_eq!(steps[0].run_id.as_deref(), Some("r1"));
        assert_eq!(steps[0].phase.as_deref(), Some("plan"));
        assert!(steps[0].finished_at.is_some());
        assert_eq!(steps[1].state, "running");
        s.insert_workflow_step(&mk(2)).unwrap();
        let open = s
            .cancel_open_workflow_steps(&w.id, crate::codec::now())
            .unwrap();
        assert_eq!(open.len(), 2);
        let steps = s.list_workflow_steps(&w.id).unwrap();
        assert_eq!(steps[0].state, "succeeded");
        assert!(
            steps[1..]
                .iter()
                .all(|x| x.state == "cancelled" && x.finished_at.is_some())
        );
        assert_eq!(s.delete_workflow_steps_from(&w.id, 1).unwrap(), 2);
        assert_eq!(s.list_workflow_steps(&w.id).unwrap().len(), 1);
        s.update_workflow_run_state(&w.id, "failed", Some(crate::codec::now()))
            .unwrap();
        s.reset_workflow_run(&w.id, "[1]").unwrap();
        let r = s.get_workflow_run(&w.id).unwrap().unwrap();
        assert_eq!(
            (r.state.as_str(), r.args_json.as_str(), r.finished_at),
            ("running", "[1]", None)
        );
        assert!(matches!(
            s.update_workflow_step(&w.id, 9, "x", None, None),
            Err(StoreError::NotFound { .. })
        ));
    }

    #[test]
    fn outcome_prefix_lookup_and_orphans() {
        let s = Store::open_in_memory().unwrap();
        let mk = |id: &str| WorkflowRunRow {
            id: id.into(),
            name: "wf".into(),
            session_id: None,
            state: "running".into(),
            args_json: "null".into(),
            started_at: crate::codec::now(),
            finished_at: None,
            result_json: None,
            error: None,
        };
        s.insert_workflow_run(&mk("abc-1")).unwrap();
        s.insert_workflow_run(&mk("abd-2")).unwrap();
        let step = WorkflowStepRow {
            workflow_run_id: "abc-1".into(),
            step_index: 0,
            phase: None,
            prompt_hash: "h".into(),
            run_id: None,
            state: "running".into(),
            started_at: crate::codec::now(),
            finished_at: None,
        };
        s.insert_workflow_step(&step).unwrap();
        s.finish_workflow_run("abd-2", "succeeded", crate::codec::now(), Some("[1]"), None)
            .unwrap();
        let done = s.get_workflow_run("abd-2").unwrap().unwrap();
        assert_eq!(done.result_json.as_deref(), Some("[1]"));
        assert_eq!(s.get_workflow_run_by_prefix("abc").unwrap().id, "abc-1");
        assert!(matches!(
            s.get_workflow_run_by_prefix("ab"),
            Err(StoreError::AmbiguousPrefix { .. })
        ));
        assert!(matches!(
            s.get_workflow_run_by_prefix("zz"),
            Err(StoreError::NotFound { .. })
        ));
        // Only the run that was still `running` is failed, and its open step cancelled.
        assert_eq!(s.fail_orphaned_workflows(crate::codec::now()).unwrap(), 1);
        let failed = s.get_workflow_run("abc-1").unwrap().unwrap();
        assert_eq!(failed.state, "failed");
        assert!(failed.error.unwrap().contains("supervisor stopped"));
        assert_eq!(
            s.list_workflow_steps("abc-1").unwrap()[0].state,
            "cancelled"
        );
        assert_eq!(
            s.get_workflow_run("abd-2").unwrap().unwrap().state,
            "succeeded"
        );
        s.reset_workflow_run("abd-2", "null").unwrap();
        assert!(
            s.get_workflow_run("abd-2")
                .unwrap()
                .unwrap()
                .result_json
                .is_none()
        );
    }

    #[test]
    fn paging_and_running_check() {
        let s = Store::open_in_memory().unwrap();
        let mk = |id: &str, age: i64, state: &str| WorkflowRunRow {
            id: id.into(),
            name: id.into(),
            session_id: Some("s1".into()),
            state: state.into(),
            args_json: "null".into(),
            started_at: crate::codec::now() - chrono::Duration::seconds(age),
            finished_at: None,
            result_json: None,
            error: None,
        };
        s.insert_workflow_run(&mk("a", 30, "succeeded")).unwrap();
        s.insert_workflow_run(&mk("b", 20, "succeeded")).unwrap();
        s.insert_workflow_run(&mk("c", 10, "running")).unwrap();
        let first = s.list_workflow_runs_page(None, 2).unwrap();
        assert_eq!(
            first.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            ["c", "b"]
        );
        let last = &first[1];
        let rest = s
            .list_workflow_runs_page(Some((last.started_at, &last.id)), 2)
            .unwrap();
        assert_eq!(rest.len(), 1);
        assert_eq!(rest[0].id, "a");
        assert!(s.session_has_running_workflow("s1").unwrap());
        assert!(!s.session_has_running_workflow("other").unwrap());
    }
}

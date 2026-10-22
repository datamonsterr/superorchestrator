//! Workflow cache and workflow run queries.

use chrono::{DateTime, Utc};
use rusqlite::{Row, params};

use crate::codec::*;
use crate::error::{Result, StoreError};
use crate::rows::{WorkflowCacheRow, WorkflowRunRow};
use crate::store::Store;

const COLS: &str = "id, name, session_id, state, args_json, started_at, finished_at";

fn map(r: &Row<'_>) -> rusqlite::Result<WorkflowRunRow> {
    Ok(WorkflowRunRow {
        id: r.get(0)?,
        name: r.get(1)?,
        session_id: r.get(2)?,
        state: r.get(3)?,
        args_json: r.get(4)?,
        started_at: get_ts(r, 5)?,
        finished_at: get_opt_ts(r, 6)?,
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
        self.conn().execute(
            &format!("INSERT INTO workflow_runs ({COLS}) VALUES (?1,?2,?3,?4,?5,?6,?7)"),
            params![
                w.id,
                w.name,
                w.session_id,
                w.state,
                w.args_json,
                ts_to_sql(&w.started_at),
                opt_ts_to_sql(&w.finished_at)
            ],
        )?;
        Ok(())
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
}

//! Run and attempt queries.

use chrono::Utc;
use rusqlite::{Row, params};
use so_core::{RunState, new_id};

use crate::codec::*;
use crate::error::{Result, StoreError};
use crate::rows::{AttemptRow, RunPatch, RunRow};
use crate::store::Store;

const COLS: &str = "id, session_id, parent_run_id, depth, name, agent, choice_id, harness, \
    account, model, effort, cwd, worktree_path, branch, native_session_id, state, review, \
    output_schema, result_json, error_kind, prompt, created_at, started_at, finished_at";

const ACTIVE: &str = "state NOT IN ('succeeded','failed','cancelled')";

fn map(r: &Row<'_>) -> rusqlite::Result<RunRow> {
    Ok(RunRow {
        id: r.get(0)?,
        session_id: r.get(1)?,
        parent_run_id: r.get(2)?,
        depth: r.get(3)?,
        name: r.get(4)?,
        agent: r.get(5)?,
        choice_id: r.get(6)?,
        harness: get_enum(r, 7)?,
        account: r.get(8)?,
        model: r.get(9)?,
        effort: r.get(10)?,
        cwd: r.get(11)?,
        worktree_path: r.get(12)?,
        branch: r.get(13)?,
        native_session_id: r.get(14)?,
        state: get_enum(r, 15)?,
        review: r.get(16)?,
        output_schema: r.get(17)?,
        result_json: r.get(18)?,
        error_kind: get_opt_enum(r, 19)?,
        prompt: r.get(20)?,
        created_at: get_ts(r, 21)?,
        started_at: get_opt_ts(r, 22)?,
        finished_at: get_opt_ts(r, 23)?,
    })
}

fn map_attempt(r: &Row<'_>) -> rusqlite::Result<AttemptRow> {
    Ok(AttemptRow {
        id: r.get(0)?,
        run_id: r.get(1)?,
        account: r.get(2)?,
        pool: r.get(3)?,
        started_at: get_ts(r, 4)?,
        ended_at: get_opt_ts(r, 5)?,
        outcome: r.get(6)?,
    })
}

impl Store {
    pub fn insert_run(&self, r: &RunRow) -> Result<()> {
        self.conn().execute(
            &format!(
                "INSERT INTO runs ({COLS}) VALUES \
                 (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24)"
            ),
            params![
                r.id,
                r.session_id,
                r.parent_run_id,
                r.depth,
                r.name,
                r.agent,
                r.choice_id,
                enum_to_sql(&r.harness),
                r.account,
                r.model,
                r.effort,
                r.cwd,
                r.worktree_path,
                r.branch,
                r.native_session_id,
                enum_to_sql(&r.state),
                r.review,
                r.output_schema,
                r.result_json,
                r.error_kind.as_ref().map(enum_to_sql),
                r.prompt,
                ts_to_sql(&r.created_at),
                opt_ts_to_sql(&r.started_at),
                opt_ts_to_sql(&r.finished_at),
            ],
        )?;
        Ok(())
    }

    pub fn get_run(&self, id: &str) -> Result<Option<RunRow>> {
        let conn = self.conn();
        let mut st = conn.prepare(&format!("SELECT {COLS} FROM runs WHERE id = ?1"))?;
        let mut rows = st.query_map([id], map)?;
        Ok(rows.next().transpose()?)
    }

    /// Finds the single run whose id starts with `prefix`.
    pub fn get_run_by_prefix(&self, prefix: &str) -> Result<RunRow> {
        let not_found = || StoreError::NotFound {
            entity: "run",
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
            "SELECT {COLS} FROM runs WHERE id LIKE ?1 ESCAPE '\\' LIMIT 2"
        ))?;
        let mut found = st
            .query_map([format!("{esc}%")], map)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        match found.len() {
            0 => Err(not_found()),
            1 => Ok(found.remove(0)),
            _ => Err(StoreError::AmbiguousPrefix {
                entity: "run",
                prefix: prefix.into(),
            }),
        }
    }

    /// Compare-and-set state change: succeeds only if the run is in `from` and
    /// `from -> to` is an allowed transition.
    pub fn update_run_state(&self, id: &str, from: RunState, to: RunState) -> Result<()> {
        if !from.can_transition(to) {
            return Err(StoreError::InvalidTransition { from, to });
        }
        let conn = self.conn();
        let n = conn.execute(
            "UPDATE runs SET state = ?3 WHERE id = ?1 AND state = ?2",
            params![id, enum_to_sql(&from), enum_to_sql(&to)],
        )?;
        if n == 1 {
            return Ok(());
        }
        match conn.query_row("SELECT state FROM runs WHERE id = ?1", [id], |r| {
            r.get::<_, String>(0)
        }) {
            Ok(actual) => Err(StoreError::StateConflict {
                id: id.into(),
                expected: from,
                actual,
            }),
            Err(rusqlite::Error::QueryReturnedNoRows) => Err(StoreError::NotFound {
                entity: "run",
                id: id.into(),
            }),
            Err(e) => Err(e.into()),
        }
    }

    /// Sets the `Some` fields of `patch`; other columns are untouched.
    pub fn patch_run(&self, id: &str, p: &RunPatch) -> Result<()> {
        let n = self.conn().execute(
            "UPDATE runs SET \
             choice_id = COALESCE(?2, choice_id), harness = COALESCE(?3, harness), \
             account = COALESCE(?4, account), model = COALESCE(?5, model), \
             effort = COALESCE(?6, effort), worktree_path = COALESCE(?7, worktree_path), \
             branch = COALESCE(?8, branch), native_session_id = COALESCE(?9, native_session_id), \
             result_json = COALESCE(?10, result_json), error_kind = COALESCE(?11, error_kind), \
             started_at = COALESCE(?12, started_at), finished_at = COALESCE(?13, finished_at) \
             WHERE id = ?1",
            params![
                id,
                p.choice_id,
                p.harness.as_ref().map(enum_to_sql),
                p.account,
                p.model,
                p.effort,
                p.worktree_path,
                p.branch,
                p.native_session_id,
                p.result_json,
                p.error_kind.as_ref().map(enum_to_sql),
                opt_ts_to_sql(&p.started_at),
                opt_ts_to_sql(&p.finished_at),
            ],
        )?;
        if n == 0 {
            return Err(StoreError::NotFound {
                entity: "run",
                id: id.into(),
            });
        }
        Ok(())
    }

    fn query_runs(&self, sql: &str, p: impl rusqlite::Params) -> Result<Vec<RunRow>> {
        let conn = self.conn();
        let mut st = conn.prepare(sql)?;
        let rows = st.query_map(p, map)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn list_runs_by_session(&self, session_id: &str) -> Result<Vec<RunRow>> {
        self.query_runs(
            &format!("SELECT {COLS} FROM runs WHERE session_id = ?1 ORDER BY created_at, id"),
            [session_id],
        )
    }

    pub fn list_child_runs(&self, parent_run_id: &str) -> Result<Vec<RunRow>> {
        self.query_runs(
            &format!("SELECT {COLS} FROM runs WHERE parent_run_id = ?1 ORDER BY created_at, id"),
            [parent_run_id],
        )
    }

    /// Runs not in a terminal state (queued counts as active).
    pub fn list_active_runs(&self) -> Result<Vec<RunRow>> {
        self.query_runs(
            &format!("SELECT {COLS} FROM runs WHERE {ACTIVE} ORDER BY created_at, id"),
            [],
        )
    }

    pub fn count_active_runs_by_account(&self, account: &str) -> Result<i64> {
        Ok(self.conn().query_row(
            &format!("SELECT COUNT(*) FROM runs WHERE account = ?1 AND {ACTIVE}"),
            [account],
            |r| r.get(0),
        )?)
    }

    pub fn count_active_runs_total(&self) -> Result<i64> {
        Ok(self.conn().query_row(
            &format!("SELECT COUNT(*) FROM runs WHERE {ACTIVE}"),
            [],
            |r| r.get(0),
        )?)
    }

    /// Records the start of an attempt on `account`; returns its id.
    pub fn start_attempt(&self, run_id: &str, account: &str, pool: Option<&str>) -> Result<String> {
        let id = new_id();
        self.conn().execute(
            "INSERT INTO attempts (id, run_id, account, pool, started_at) VALUES (?1,?2,?3,?4,?5)",
            params![id, run_id, account, pool, ts_to_sql(&Utc::now())],
        )?;
        Ok(id)
    }

    pub fn finish_attempt(&self, id: &str, outcome: &str) -> Result<()> {
        let n = self.conn().execute(
            "UPDATE attempts SET ended_at = ?2, outcome = ?3 WHERE id = ?1",
            params![id, ts_to_sql(&Utc::now()), outcome],
        )?;
        if n == 0 {
            return Err(StoreError::NotFound {
                entity: "attempt",
                id: id.into(),
            });
        }
        Ok(())
    }

    pub fn list_attempts(&self, run_id: &str) -> Result<Vec<AttemptRow>> {
        let conn = self.conn();
        let mut st = conn.prepare(
            "SELECT id, run_id, account, pool, started_at, ended_at, outcome FROM attempts \
             WHERE run_id = ?1 ORDER BY started_at, id",
        )?;
        let rows = st.query_map([run_id], map_attempt)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::sessions::tests::session;
    use so_core::{ErrorKind, Harness};

    pub fn run(session_id: &str, account: Option<&str>) -> RunRow {
        RunRow {
            id: new_id(),
            session_id: session_id.into(),
            parent_run_id: None,
            depth: 0,
            name: Some("w".into()),
            agent: None,
            choice_id: None,
            harness: Harness::Codex,
            account: account.map(Into::into),
            model: None,
            effort: None,
            cwd: None,
            worktree_path: None,
            branch: None,
            native_session_id: None,
            state: RunState::Queued,
            review: false,
            output_schema: None,
            result_json: None,
            error_kind: None,
            prompt: Some("do it".into()),
            created_at: crate::codec::now(),
            started_at: None,
            finished_at: None,
        }
    }

    fn setup() -> (Store, String) {
        let s = Store::open_in_memory().unwrap();
        let sess = session(None);
        s.insert_session(&sess).unwrap();
        (s, sess.id)
    }

    #[test]
    fn insert_get_list_and_fk() {
        let (s, sid) = setup();
        let mut r = run(&sid, Some("codex-acc1"));
        r.review = true;
        s.insert_run(&r).unwrap();
        assert_eq!(s.get_run(&r.id).unwrap().unwrap(), r);
        let mut child = run(&sid, None);
        child.parent_run_id = Some(r.id.clone());
        child.depth = 1;
        s.insert_run(&child).unwrap();
        assert_eq!(s.list_runs_by_session(&sid).unwrap().len(), 2);
        let kids = s.list_child_runs(&r.id).unwrap();
        assert_eq!(kids.len(), 1);
        assert_eq!(kids[0].id, child.id);
        assert!(s.insert_run(&run("missing-session", None)).is_err());
    }

    #[test]
    fn prefix_lookup() {
        let (s, sid) = setup();
        let r = run(&sid, None);
        s.insert_run(&r).unwrap();
        assert_eq!(s.get_run_by_prefix(&r.id[..12]).unwrap().id, r.id);
        assert!(matches!(
            s.get_run_by_prefix("zzzz"),
            Err(StoreError::NotFound { .. })
        ));
        assert!(matches!(
            s.get_run_by_prefix(""),
            Err(StoreError::NotFound { .. })
        ));
        let mut a = run(&sid, None);
        let mut b = run(&sid, None);
        a.id = "abc1".into();
        b.id = "abc2".into();
        s.insert_run(&a).unwrap();
        s.insert_run(&b).unwrap();
        assert!(matches!(
            s.get_run_by_prefix("abc"),
            Err(StoreError::AmbiguousPrefix { .. })
        ));
        assert_eq!(s.get_run_by_prefix("abc2").unwrap().id, "abc2");
        assert!(s.get_run_by_prefix("%").is_err());
    }

    #[test]
    fn state_cas() {
        let (s, sid) = setup();
        let r = run(&sid, None);
        s.insert_run(&r).unwrap();
        s.update_run_state(&r.id, RunState::Queued, RunState::Starting)
            .unwrap();
        // wrong from
        assert!(matches!(
            s.update_run_state(&r.id, RunState::Queued, RunState::Cancelled),
            Err(StoreError::StateConflict { .. })
        ));
        // forbidden transition
        assert!(matches!(
            s.update_run_state(&r.id, RunState::Starting, RunState::Succeeded),
            Err(StoreError::InvalidTransition { .. })
        ));
        assert!(matches!(
            s.update_run_state("nope", RunState::Queued, RunState::Starting),
            Err(StoreError::NotFound { .. })
        ));
        assert_eq!(s.get_run(&r.id).unwrap().unwrap().state, RunState::Starting);
    }

    #[test]
    fn patch_and_counts() {
        let (s, sid) = setup();
        let a = run(&sid, Some("a1"));
        let b = run(&sid, Some("a1"));
        let c = run(&sid, Some("a2"));
        for r in [&a, &b, &c] {
            s.insert_run(r).unwrap();
        }
        assert_eq!(s.count_active_runs_by_account("a1").unwrap(), 2);
        assert_eq!(s.count_active_runs_total().unwrap(), 3);
        let now = crate::codec::now();
        s.patch_run(
            &a.id,
            &RunPatch {
                native_session_id: Some("nat".into()),
                branch: Some("br".into()),
                worktree_path: Some("/wt".into()),
                error_kind: Some(ErrorKind::Quota),
                result_json: Some("{}".into()),
                started_at: Some(now),
                finished_at: Some(now),
                ..Default::default()
            },
        )
        .unwrap();
        let got = s.get_run(&a.id).unwrap().unwrap();
        assert_eq!(got.native_session_id.as_deref(), Some("nat"));
        assert_eq!(got.branch.as_deref(), Some("br"));
        assert_eq!(got.error_kind, Some(ErrorKind::Quota));
        assert_eq!(got.account.as_deref(), Some("a1"));
        assert!(got.finished_at.is_some());
        assert!(matches!(
            s.patch_run("nope", &RunPatch::default()),
            Err(StoreError::NotFound { .. })
        ));
        s.update_run_state(&a.id, RunState::Queued, RunState::Cancelled)
            .unwrap();
        assert_eq!(s.count_active_runs_by_account("a1").unwrap(), 1);
        assert_eq!(s.count_active_runs_total().unwrap(), 2);
        assert_eq!(s.list_active_runs().unwrap().len(), 2);
    }

    #[test]
    fn attempts() {
        let (s, sid) = setup();
        let r = run(&sid, None);
        s.insert_run(&r).unwrap();
        let id = s.start_attempt(&r.id, "acc", Some("pool")).unwrap();
        let v = s.list_attempts(&r.id).unwrap();
        assert_eq!(v.len(), 1);
        assert!(v[0].ended_at.is_none());
        s.finish_attempt(&id, "quota").unwrap();
        let v = s.list_attempts(&r.id).unwrap();
        assert_eq!(v[0].outcome.as_deref(), Some("quota"));
        assert!(v[0].ended_at.is_some());
        assert!(s.finish_attempt("nope", "x").is_err());
        assert!(s.start_attempt("nope", "a", None).is_err());
    }
}

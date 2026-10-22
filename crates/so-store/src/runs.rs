//! Run and attempt queries.

use std::sync::LazyLock;

use chrono::{DateTime, Utc};
use rusqlite::types::ToSql;
use rusqlite::{Connection, Row, TransactionBehavior, params};
use so_core::{RunState, new_id};

use crate::codec::*;
use crate::error::{Result, StoreError};
use crate::rows::{AttemptRow, FinishPatch, InsertRun, RunPatch, RunRow};
use crate::store::Store;

const COLS: &str = "id, session_id, parent_run_id, depth, name, agent, choice_id, harness, \
    account, model, effort, cwd, worktree_path, branch, native_session_id, state, review, \
    output_schema, result_json, error_kind, prompt, created_at, started_at, finished_at, \
    isolation, max_turns, worktree_base, worktree_repo, timeout_ms";

fn state_list(pred: impl Fn(RunState) -> bool) -> String {
    RunState::ALL
        .into_iter()
        .filter(|s| pred(*s))
        .map(|s| format!("'{}'", s.as_str()))
        .collect::<Vec<_>>()
        .join(",")
}

/// Non-terminal states, including `queued`.
static ACTIVE: LazyLock<String> =
    LazyLock::new(|| format!("state IN ({})", state_list(|s| s.is_active())));

/// States that hold a concurrency slot. `queued` waits for a slot and `pending_review` has
/// no harness process, so both are excluded (SPEC 7.7).
static SLOT_HOLDING: LazyLock<String> = LazyLock::new(|| {
    format!(
        "state IN ({})",
        state_list(|s| s.is_active() && s != RunState::Queued && s != RunState::PendingReview)
    )
});

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
        isolation: get_enum(r, 24)?,
        max_turns: r
            .get::<_, Option<i64>>(25)?
            .and_then(|n| u32::try_from(n).ok()),
        worktree_base: r.get(26)?,
        worktree_repo: r.get(27)?,
        timeout_ms: r
            .get::<_, Option<i64>>(28)?
            .and_then(|n| u64::try_from(n).ok()),
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
        insert_run_on(&self.conn(), r)
    }

    /// Inserts a run unless its parent already has `max_children` active children. The
    /// count and the insert share one transaction, so concurrent spawns cannot overshoot.
    pub fn insert_run_checked(&self, r: &RunRow, max_children: usize) -> Result<InsertRun> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(parent) = &r.parent_run_id {
            let active = &*ACTIVE;
            let n: i64 = tx.query_row(
                &format!("SELECT COUNT(*) FROM runs WHERE parent_run_id = ?1 AND {active}"),
                [parent],
                |row| row.get(0),
            )?;
            let n = usize::try_from(n).unwrap_or(usize::MAX);
            if n >= max_children {
                return Ok(InsertRun::ChildCap { active: n });
            }
        }
        insert_run_on(&tx, r)?;
        tx.commit()?;
        Ok(InsertRun::Inserted)
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
        cas_state(&self.conn(), id, from, to)
    }

    /// Brings a `succeeded` or `failed` run back to `running` (a message resumed its native
    /// session) and clears its result. Returns `false` when the run is in another state.
    pub fn reopen_run(&self, id: &str) -> Result<bool> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let n = tx.execute(
            "UPDATE runs SET state = 'running', finished_at = NULL, error_kind = NULL, \
             result_json = NULL WHERE id = ?1 AND state IN ('succeeded', 'failed')",
            [id],
        )?;
        if n == 1 {
            let sid: String =
                tx.query_row("SELECT session_id FROM runs WHERE id = ?1", [id], |r| {
                    r.get(0)
                })?;
            tx.execute(
                "UPDATE sessions SET status = 'active', ended_at = NULL WHERE id = ?1",
                [sid],
            )?;
        }
        tx.commit()?;
        Ok(n == 1)
    }

    /// Stores the review checks of a run (`ci`, `pr_threads`, `cmd:<shell>`).
    pub fn set_run_checks(&self, run_id: &str, checks: &[String]) -> Result<()> {
        let json = serde_json::to_string(checks).unwrap_or_else(|_| "[]".into());
        self.conn().execute(
            "INSERT INTO run_meta (run_id, checks_json) VALUES (?1, ?2) \
             ON CONFLICT(run_id) DO UPDATE SET checks_json = excluded.checks_json",
            params![run_id, json],
        )?;
        Ok(())
    }

    /// The review checks of a run; empty when none were set.
    pub fn get_run_checks(&self, run_id: &str) -> Result<Vec<String>> {
        let json: Option<String> = self
            .conn()
            .query_row(
                "SELECT checks_json FROM run_meta WHERE run_id = ?1",
                [run_id],
                |r| r.get(0),
            )
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                e => Err(e),
            })?;
        Ok(json
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_default())
    }

    /// Marks a run as a fork: it resumes a copy of this native session when it starts.
    pub fn set_run_fork(&self, run_id: &str, native_session_id: &str) -> Result<()> {
        self.conn().execute(
            "INSERT INTO run_meta (run_id, fork_from) VALUES (?1, ?2) \
             ON CONFLICT(run_id) DO UPDATE SET fork_from = excluded.fork_from",
            params![run_id, native_session_id],
        )?;
        Ok(())
    }

    /// The native session a fork run starts from.
    pub fn get_run_fork(&self, run_id: &str) -> Result<Option<String>> {
        let v: Option<Option<String>> = self
            .conn()
            .query_row(
                "SELECT fork_from FROM run_meta WHERE run_id = ?1",
                [run_id],
                |r| r.get(0),
            )
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                e => Err(e),
            })?;
        Ok(v.flatten())
    }

    /// Atomically claims a concurrency slot: moves the run `from -> to` and sets its account,
    /// only if the account (slot-holding runs plus open interactive sessions) is below
    /// `account_cap` and all slot-holding runs are below `global_cap`. Moving into
    /// `starting` also sets `started_at` to `now`. Returns `false`
    /// (changing nothing) when a cap is reached.
    #[allow(clippy::too_many_arguments)] // one atomic claim; a params struct would only add noise
    pub fn try_claim_slot(
        &self,
        run_id: &str,
        from: RunState,
        to: RunState,
        account: &str,
        account_cap: i64,
        global_cap: i64,
        now: DateTime<Utc>,
    ) -> Result<bool> {
        if !from.can_transition(to) {
            return Err(StoreError::InvalidTransition { from, to });
        }
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let actual = current_state(&tx, run_id)?;
        if actual != from {
            return Err(StoreError::StateConflict {
                id: run_id.into(),
                expected: from,
                actual,
            });
        }
        let slot = &*SLOT_HOLDING;
        let account_runs: i64 = tx.query_row(
            &format!("SELECT COUNT(*) FROM runs WHERE account = ?1 AND id != ?2 AND {slot}"),
            params![account, run_id],
            |r| r.get(0),
        )?;
        let interactive = crate::sessions::count_interactive(&tx, account)?;
        let total: i64 = tx.query_row(
            &format!("SELECT COUNT(*) FROM runs WHERE id != ?1 AND {slot}"),
            [run_id],
            |r| r.get(0),
        )?;
        if account_runs + interactive >= account_cap || total >= global_cap {
            return Ok(false);
        }
        tx.execute(
            "UPDATE runs SET state = ?2, account = ?3, started_at = COALESCE(?4, started_at) \
             WHERE id = ?1",
            params![
                run_id,
                enum_to_sql(&to),
                account,
                (to == RunState::Starting).then(|| ts_to_sql(&now))
            ],
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// Atomically moves a run `from -> to` and writes the final fields of `patch`.
    pub fn finish_run(
        &self,
        run_id: &str,
        from: RunState,
        to: RunState,
        patch: &FinishPatch,
    ) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        cas_state(&tx, run_id, from, to)?;
        tx.execute(
            "UPDATE runs SET result_json = COALESCE(?2, result_json), \
             error_kind = COALESCE(?3, error_kind), finished_at = COALESCE(?4, finished_at) \
             WHERE id = ?1",
            params![
                run_id,
                patch.result_json,
                patch.error_kind.as_ref().map(enum_to_sql),
                opt_ts_to_sql(&patch.finished_at),
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Sets the fields present in `p`; other columns are untouched.
    pub fn patch_run(&self, id: &str, p: &RunPatch) -> Result<()> {
        let mut sets: Vec<&str> = Vec::new();
        let mut vals: Vec<Box<dyn ToSql>> = Vec::new();
        macro_rules! set {
            ($col:literal, $v:expr) => {
                sets.push(concat!($col, " = ?"));
                vals.push(Box::new($v));
            };
        }
        if let Some(v) = &p.choice_id {
            set!("choice_id", v.clone());
        }
        if let Some(v) = &p.harness {
            set!("harness", enum_to_sql(v));
        }
        if let Some(v) = &p.model {
            set!("model", v.clone());
        }
        if let Some(v) = &p.effort {
            set!("effort", v.clone());
        }
        if let Some(v) = &p.account {
            set!("account", v.clone());
        }
        if let Some(v) = &p.worktree_path {
            set!("worktree_path", v.clone());
        }
        if let Some(v) = &p.branch {
            set!("branch", v.clone());
        }
        if let Some(v) = &p.worktree_base {
            set!("worktree_base", v.clone());
        }
        if let Some(v) = &p.worktree_repo {
            set!("worktree_repo", v.clone());
        }
        if let Some(v) = &p.native_session_id {
            set!("native_session_id", v.clone());
        }
        if let Some(v) = &p.result_json {
            set!("result_json", v.clone());
        }
        if let Some(v) = &p.error_kind {
            set!("error_kind", v.as_ref().map(enum_to_sql));
        }
        if let Some(v) = &p.started_at {
            set!("started_at", ts_to_sql(v));
        }
        if let Some(v) = &p.finished_at {
            set!("finished_at", opt_ts_to_sql(v));
        }
        if sets.is_empty() {
            sets.push("id = id");
        }
        let sql = format!("UPDATE runs SET {} WHERE id = ?", sets.join(", "));
        vals.push(Box::new(id.to_string()));
        let n = self
            .conn()
            .execute(&sql, rusqlite::params_from_iter(vals.iter().map(|v| &**v)))?;
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

    pub fn list_runs_by_session(&self, session_id: &str, limit: usize) -> Result<Vec<RunRow>> {
        self.query_runs(
            &format!(
                "SELECT {COLS} FROM runs WHERE session_id = ?1 ORDER BY created_at, id LIMIT ?2"
            ),
            params![session_id, limit as i64],
        )
    }

    pub fn list_child_runs(&self, parent_run_id: &str, limit: usize) -> Result<Vec<RunRow>> {
        self.query_runs(
            &format!(
                "SELECT {COLS} FROM runs WHERE parent_run_id = ?1 ORDER BY created_at, id LIMIT ?2"
            ),
            params![parent_run_id, limit as i64],
        )
    }

    /// Runs not in a terminal state, including `queued` (unlike the slot counts).
    pub fn list_active_runs(&self, limit: usize) -> Result<Vec<RunRow>> {
        let active = &*ACTIVE;
        self.query_runs(
            &format!("SELECT {COLS} FROM runs WHERE {active} ORDER BY created_at, id LIMIT ?1"),
            [limit as i64],
        )
    }

    /// Queued runs, oldest first (FIFO queue drain).
    pub fn list_queued_runs(&self, limit: usize) -> Result<Vec<RunRow>> {
        self.query_runs(
            &format!("SELECT {COLS} FROM runs WHERE state = ?1 ORDER BY created_at, id LIMIT ?2"),
            params![enum_to_sql(&RunState::Queued), limit as i64],
        )
    }

    /// Slot-holding runs (not queued, not terminal) on `account`.
    pub fn count_active_runs_by_account(&self, account: &str) -> Result<i64> {
        let slot = &*SLOT_HOLDING;
        Ok(self.conn().query_row(
            &format!("SELECT COUNT(*) FROM runs WHERE account = ?1 AND {slot}"),
            [account],
            |r| r.get(0),
        )?)
    }

    /// Slot-holding runs across all accounts.
    pub fn count_active_runs_total(&self) -> Result<i64> {
        let slot = &*SLOT_HOLDING;
        Ok(self.conn().query_row(
            &format!("SELECT COUNT(*) FROM runs WHERE {slot}"),
            [],
            |r| r.get(0),
        )?)
    }

    /// Records the start of an attempt on `account`; returns its id.
    pub fn start_attempt(
        &self,
        run_id: &str,
        account: &str,
        pool: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<String> {
        let id = new_id();
        self.conn().execute(
            "INSERT INTO attempts (id, run_id, account, pool, started_at) VALUES (?1,?2,?3,?4,?5)",
            params![id, run_id, account, pool, ts_to_sql(&now)],
        )?;
        Ok(id)
    }

    /// Start of the most recent attempt on `account`, for least-recently-used ranking.
    pub fn last_attempt_started(&self, account: &str) -> Result<Option<DateTime<Utc>>> {
        let conn = self.conn();
        let mut st = conn.prepare(
            "SELECT started_at FROM attempts WHERE account = ?1 ORDER BY started_at DESC LIMIT 1",
        )?;
        let mut rows = st.query_map([account], |r| get_ts(r, 0))?;
        Ok(rows.next().transpose()?)
    }

    pub fn finish_attempt(&self, id: &str, outcome: &str, now: DateTime<Utc>) -> Result<()> {
        let n = self.conn().execute(
            "UPDATE attempts SET ended_at = ?2, outcome = ?3 WHERE id = ?1",
            params![id, ts_to_sql(&now), outcome],
        )?;
        if n == 0 {
            return Err(StoreError::NotFound {
                entity: "attempt",
                id: id.into(),
            });
        }
        Ok(())
    }

    pub fn list_attempts(&self, run_id: &str, limit: usize) -> Result<Vec<AttemptRow>> {
        let conn = self.conn();
        let mut st = conn.prepare(
            "SELECT id, run_id, account, pool, started_at, ended_at, outcome FROM attempts \
             WHERE run_id = ?1 ORDER BY started_at, id LIMIT ?2",
        )?;
        let rows = st.query_map(params![run_id, limit as i64], map_attempt)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

fn insert_run_on(conn: &Connection, r: &RunRow) -> Result<()> {
    conn.execute(
        &format!(
            "INSERT INTO runs ({COLS}) VALUES \
             (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,\
              ?25,?26,?27,?28,?29)"
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
            enum_to_sql(&r.isolation),
            r.max_turns.map(i64::from),
            r.worktree_base,
            r.worktree_repo,
            r.timeout_ms.and_then(|n| i64::try_from(n).ok()),
        ],
    )?;
    Ok(())
}

fn current_state(conn: &Connection, id: &str) -> Result<RunState> {
    match conn.query_row("SELECT state FROM runs WHERE id = ?1", [id], |r| {
        get_enum::<RunState>(r, 0)
    }) {
        Ok(s) => Ok(s),
        Err(rusqlite::Error::QueryReturnedNoRows) => Err(StoreError::NotFound {
            entity: "run",
            id: id.into(),
        }),
        Err(e) => Err(e.into()),
    }
}

fn cas_state(conn: &Connection, id: &str, from: RunState, to: RunState) -> Result<()> {
    if !from.can_transition(to) {
        return Err(StoreError::InvalidTransition { from, to });
    }
    let n = conn.execute(
        "UPDATE runs SET state = ?3 WHERE id = ?1 AND state = ?2",
        params![id, enum_to_sql(&from), enum_to_sql(&to)],
    )?;
    if n == 1 {
        return Ok(());
    }
    Err(StoreError::StateConflict {
        id: id.into(),
        expected: from,
        actual: current_state(conn, id)?,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::rows::FinishPatch;
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
            isolation: so_core::Isolation::None,
            max_turns: None,
            worktree_base: None,
            worktree_repo: None,
            timeout_ms: None,
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
        assert_eq!(s.list_runs_by_session(&sid, 10).unwrap().len(), 2);
        let kids = s.list_child_runs(&r.id, 10).unwrap();
        assert_eq!(kids.len(), 1);
        assert_eq!(kids[0].id, child.id);
        assert!(matches!(
            s.insert_run(&run("missing-session", None)),
            Err(StoreError::Constraint(_))
        ));
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
        for id in ["abc1", "abc2", "a_c3", "a\\c4"] {
            let mut x = run(&sid, None);
            x.id = id.into();
            s.insert_run(&x).unwrap();
        }
        assert!(matches!(
            s.get_run_by_prefix("abc"),
            Err(StoreError::AmbiguousPrefix { .. })
        ));
        assert_eq!(s.get_run_by_prefix("abc2").unwrap().id, "abc2");
        // `_` and `\` are literals, not LIKE wildcards
        assert_eq!(s.get_run_by_prefix("a_").unwrap().id, "a_c3");
        assert_eq!(s.get_run_by_prefix("a\\").unwrap().id, "a\\c4");
        assert!(matches!(
            s.get_run_by_prefix("%"),
            Err(StoreError::NotFound { .. })
        ));
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
            Err(StoreError::StateConflict {
                actual: RunState::Starting,
                ..
            })
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
    fn slot_sql_sets_match_states() {
        for st in RunState::ALL {
            let slot = st.is_active() && st != RunState::Queued && st != RunState::PendingReview;
            let q = format!("'{}'", st.as_str());
            assert_eq!(SLOT_HOLDING.contains(&q), slot, "{st}");
            assert_eq!(ACTIVE.contains(&q), st.is_active(), "{st}");
        }
    }

    #[test]
    fn patch_set_and_clear() {
        let (s, sid) = setup();
        let a = run(&sid, Some("a1"));
        s.insert_run(&a).unwrap();
        let now = crate::codec::now();
        s.patch_run(
            &a.id,
            &RunPatch {
                native_session_id: Some(Some("nat".into())),
                branch: Some(Some("br".into())),
                worktree_path: Some(Some("/wt".into())),
                error_kind: Some(Some(ErrorKind::Quota)),
                result_json: Some(Some("{}".into())),
                started_at: Some(now),
                finished_at: Some(Some(now)),
                model: Some("m".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let got = s.get_run(&a.id).unwrap().unwrap();
        assert_eq!(got.native_session_id.as_deref(), Some("nat"));
        assert_eq!(got.branch.as_deref(), Some("br"));
        assert_eq!(got.error_kind, Some(ErrorKind::Quota));
        assert_eq!(got.model.as_deref(), Some("m"));
        assert_eq!(got.account.as_deref(), Some("a1"));
        assert_eq!(got.finished_at, Some(now));
        s.patch_run(
            &a.id,
            &RunPatch {
                error_kind: Some(None),
                finished_at: Some(None),
                account: Some(None),
                ..Default::default()
            },
        )
        .unwrap();
        let got = s.get_run(&a.id).unwrap().unwrap();
        assert_eq!(got.error_kind, None);
        assert_eq!(got.finished_at, None);
        assert_eq!(got.account, None);
        assert_eq!(got.branch.as_deref(), Some("br"));
        s.patch_run(&a.id, &RunPatch::default()).unwrap();
        assert!(matches!(
            s.patch_run("nope", &RunPatch::default()),
            Err(StoreError::NotFound { .. })
        ));
    }

    #[test]
    fn slot_counts_exclude_queued() {
        let (s, sid) = setup();
        let a = run(&sid, Some("a1"));
        let b = run(&sid, Some("a1"));
        let c = run(&sid, Some("a2"));
        for r in [&a, &b, &c] {
            s.insert_run(r).unwrap();
        }
        assert_eq!(s.count_active_runs_by_account("a1").unwrap(), 0);
        assert_eq!(s.count_active_runs_total().unwrap(), 0);
        assert_eq!(s.list_active_runs(10).unwrap().len(), 3);
        assert_eq!(s.list_queued_runs(2).unwrap().len(), 2);
        assert_eq!(s.list_queued_runs(10).unwrap()[0].id, a.id);
        for r in [&a, &b, &c] {
            s.update_run_state(&r.id, RunState::Queued, RunState::Starting)
                .unwrap();
        }
        assert!(s.list_queued_runs(10).unwrap().is_empty());
        assert_eq!(s.count_active_runs_by_account("a1").unwrap(), 2);
        assert_eq!(s.count_active_runs_total().unwrap(), 3);
        s.update_run_state(&a.id, RunState::Starting, RunState::Cancelled)
            .unwrap();
        assert_eq!(s.count_active_runs_by_account("a1").unwrap(), 1);
        assert_eq!(s.count_active_runs_total().unwrap(), 2);
        assert_eq!(s.list_active_runs(10).unwrap().len(), 2);
    }

    #[test]
    fn claim_slot_respects_caps() {
        let (s, sid) = setup();
        let runs: Vec<_> = (0..4).map(|_| run(&sid, None)).collect();
        for r in &runs {
            s.insert_run(r).unwrap();
        }
        let claim = |i: usize, acct_cap, global_cap| {
            s.try_claim_slot(
                &runs[i].id,
                RunState::Queued,
                RunState::Starting,
                "a1",
                acct_cap,
                global_cap,
                crate::codec::now(),
            )
            .unwrap()
        };
        assert!(claim(0, 2, 10));
        assert!(
            s.get_run(&runs[0].id)
                .unwrap()
                .unwrap()
                .started_at
                .is_some()
        );
        assert!(
            s.get_run(&runs[1].id)
                .unwrap()
                .unwrap()
                .started_at
                .is_none()
        );
        assert_eq!(
            s.get_run(&runs[0].id).unwrap().unwrap().account.as_deref(),
            Some("a1")
        );
        // an open interactive session on the account takes the second slot
        let mut inter = session(None);
        inter.kind = so_core::SessionKind::Interactive;
        inter.account = Some("a1".into());
        s.insert_session(&inter).unwrap();
        assert!(!claim(1, 2, 10));
        assert_eq!(
            s.get_run(&runs[1].id).unwrap().unwrap().state,
            RunState::Queued
        );
        s.end_session(&inter.id, "ended", crate::codec::now())
            .unwrap();
        assert!(claim(1, 2, 10));
        assert!(!claim(2, 2, 10)); // account cap
        assert!(!claim(2, 5, 2)); // global cap
        assert!(claim(2, 5, 3));
        // errors
        assert!(matches!(
            s.try_claim_slot(
                &runs[2].id,
                RunState::Queued,
                RunState::Starting,
                "a",
                9,
                9,
                crate::codec::now()
            ),
            Err(StoreError::StateConflict { .. })
        ));
        assert!(matches!(
            s.try_claim_slot(
                &runs[3].id,
                RunState::Queued,
                RunState::Running,
                "a",
                9,
                9,
                crate::codec::now()
            ),
            Err(StoreError::InvalidTransition { .. })
        ));
        assert!(matches!(
            s.try_claim_slot(
                "nope",
                RunState::Queued,
                RunState::Starting,
                "a",
                9,
                9,
                crate::codec::now()
            ),
            Err(StoreError::NotFound { .. })
        ));
    }

    #[test]
    fn claim_slot_race_has_one_winner() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let a = Store::open(&path).unwrap();
        let b = Store::open(&path).unwrap();
        let sess = session(None);
        a.insert_session(&sess).unwrap();
        let r1 = run(&sess.id, None);
        let r2 = run(&sess.id, None);
        a.insert_run(&r1).unwrap();
        a.insert_run(&r2).unwrap();
        let handles: Vec<_> = [(a.clone(), r1.id.clone()), (b, r2.id.clone())]
            .into_iter()
            .map(|(st, id)| {
                std::thread::spawn(move || {
                    st.try_claim_slot(
                        &id,
                        RunState::Queued,
                        RunState::Starting,
                        "acc",
                        1,
                        1,
                        crate::codec::now(),
                    )
                    .unwrap()
                })
            })
            .collect();
        let wins = handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .filter(|w| *w)
            .count();
        assert_eq!(wins, 1);
        assert_eq!(a.count_active_runs_total().unwrap(), 1);
    }

    #[test]
    fn reopen_and_checks() {
        let (s, sid) = setup();
        let r = run(&sid, None);
        s.insert_run(&r).unwrap();
        assert!(!s.reopen_run(&r.id).unwrap());
        s.set_run_checks(&r.id, &["ci".into(), "cmd:true".into()])
            .unwrap();
        assert_eq!(s.get_run_checks(&r.id).unwrap(), ["ci", "cmd:true"]);
        assert!(s.get_run_checks("none").unwrap().is_empty());
        assert_eq!(s.get_run_fork(&r.id).unwrap(), None);
        s.set_run_fork(&r.id, "native-1").unwrap();
        assert_eq!(s.get_run_fork(&r.id).unwrap().as_deref(), Some("native-1"));
        assert_eq!(s.get_run_checks(&r.id).unwrap(), ["ci", "cmd:true"]);
        let patch = FinishPatch {
            result_json: Some("1".into()),
            error_kind: None,
            finished_at: Some(crate::codec::now()),
        };
        s.finish_run(&r.id, RunState::Queued, RunState::Failed, &patch)
            .unwrap();
        assert!(s.reopen_run(&r.id).unwrap());
        let got = s.get_run(&r.id).unwrap().unwrap();
        assert_eq!(got.state, RunState::Running);
        assert!(got.finished_at.is_none() && got.result_json.is_none());
    }

    #[test]
    fn finish_run_is_atomic() {
        let (s, sid) = setup();
        let r = run(&sid, None);
        s.insert_run(&r).unwrap();
        let patch = FinishPatch {
            result_json: Some("{\"ok\":false}".into()),
            error_kind: Some(ErrorKind::Auth),
            finished_at: Some(crate::codec::now()),
        };
        // wrong from: nothing is written
        assert!(matches!(
            s.finish_run(&r.id, RunState::Running, RunState::Failed, &patch),
            Err(StoreError::StateConflict { .. })
        ));
        let got = s.get_run(&r.id).unwrap().unwrap();
        assert_eq!((got.state, got.error_kind), (RunState::Queued, None));
        s.finish_run(&r.id, RunState::Queued, RunState::Failed, &patch)
            .unwrap();
        let got = s.get_run(&r.id).unwrap().unwrap();
        assert_eq!(got.state, RunState::Failed);
        assert_eq!(got.error_kind, Some(ErrorKind::Auth));
        assert!(got.finished_at.is_some() && got.result_json.is_some());
        assert!(matches!(
            s.finish_run(&r.id, RunState::Failed, RunState::Running, &patch),
            Err(StoreError::InvalidTransition { .. })
        ));
    }

    #[test]
    fn attempts() {
        let (s, sid) = setup();
        let r = run(&sid, None);
        s.insert_run(&r).unwrap();
        let now = crate::codec::now();
        let id = s.start_attempt(&r.id, "acc", Some("pool"), now).unwrap();
        let v = s.list_attempts(&r.id, 10).unwrap();
        assert_eq!(v.len(), 1);
        assert!(v[0].ended_at.is_none());
        s.finish_attempt(&id, "quota", now).unwrap();
        let v = s.list_attempts(&r.id, 10).unwrap();
        assert_eq!(v[0].outcome.as_deref(), Some("quota"));
        assert_eq!(v[0].ended_at, Some(now));
        assert!(matches!(
            s.finish_attempt("nope", "x", now),
            Err(StoreError::NotFound { .. })
        ));
        assert!(matches!(
            s.start_attempt("nope", "a", None, now),
            Err(StoreError::Constraint(_))
        ));
    }

    #[test]
    fn insert_run_checked_caps_active_children() {
        let (s, sid) = setup();
        let parent = run(&sid, None);
        s.insert_run(&parent).unwrap();
        let child = |state| {
            let mut r = run(&sid, None);
            r.parent_run_id = Some(parent.id.clone());
            r.state = state;
            r
        };
        assert_eq!(
            s.insert_run_checked(&child(RunState::Queued), 2).unwrap(),
            InsertRun::Inserted
        );
        s.insert_run(&child(RunState::Succeeded)).unwrap();
        assert_eq!(
            s.insert_run_checked(&child(RunState::Running), 2).unwrap(),
            InsertRun::Inserted
        );
        assert_eq!(
            s.insert_run_checked(&child(RunState::Queued), 2).unwrap(),
            InsertRun::ChildCap { active: 2 }
        );
        assert_eq!(s.list_child_runs(&parent.id, 10).unwrap().len(), 3);
    }

    #[test]
    fn last_attempt_started_is_the_newest() {
        let (s, sid) = setup();
        let r = run(&sid, None);
        s.insert_run(&r).unwrap();
        let now = crate::codec::now();
        assert_eq!(s.last_attempt_started("acc").unwrap(), None);
        s.start_attempt(&r.id, "acc", None, now - chrono::Duration::hours(2))
            .unwrap();
        s.start_attempt(&r.id, "acc", None, now).unwrap();
        s.start_attempt(&r.id, "other", None, now + chrono::Duration::hours(1))
            .unwrap();
        assert_eq!(s.last_attempt_started("acc").unwrap(), Some(now));
    }
}

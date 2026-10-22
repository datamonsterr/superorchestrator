//! Session queries.

use chrono::{DateTime, Utc};
use rusqlite::{Connection, Row, params};
use so_core::SessionKind;

use crate::codec::*;
use crate::error::{Result, StoreError};
use crate::rows::{SessionFilter, SessionRow, SessionStats};
use crate::store::Store;

const COLS: &str = "id, kind, client, cwd, account, pid, started_at, ended_at, status";

fn map(r: &Row<'_>) -> rusqlite::Result<SessionRow> {
    Ok(SessionRow {
        id: r.get(0)?,
        kind: get_enum(r, 1)?,
        client: r.get(2)?,
        cwd: r.get(3)?,
        account: r.get(4)?,
        pid: r.get(5)?,
        started_at: get_ts(r, 6)?,
        ended_at: get_opt_ts(r, 7)?,
        status: r.get(8)?,
    })
}

/// Ends open interactive sessions whose process is gone. `exec` replaces the wrapper
/// process, so nothing else records the end. Runs inside every cap check.
pub(crate) fn reap_dead_interactive(conn: &Connection) -> Result<usize> {
    let open: Vec<(String, i64, Option<String>)> = {
        let mut st = conn.prepare(
            "SELECT id, pid, pid_start FROM sessions \
             WHERE kind = ?1 AND ended_at IS NULL AND pid IS NOT NULL",
        )?;
        st.query_map([enum_to_sql(&SessionKind::Interactive)], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?
        .collect::<rusqlite::Result<_>>()?
    };
    let now = ts_to_sql(&chrono::Utc::now());
    let mut ended = 0;
    for (id, pid, start) in open {
        if !so_core::procinfo::same_process_alive(pid, start.as_deref()) {
            ended += conn.execute(
                "UPDATE sessions SET ended_at = ?2, status = 'ended' \
                 WHERE id = ?1 AND ended_at IS NULL",
                params![id, now],
            )?;
        }
    }
    Ok(ended)
}

pub(crate) fn count_interactive(conn: &Connection, account: &str) -> Result<i64> {
    reap_dead_interactive(conn)?;
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM sessions WHERE kind = ?1 AND account = ?2 AND ended_at IS NULL",
        params![enum_to_sql(&SessionKind::Interactive), account],
        |r| r.get(0),
    )?)
}

impl Store {
    pub fn insert_session(&self, s: &SessionRow) -> Result<()> {
        self.conn().execute(
            &format!("INSERT INTO sessions ({COLS}) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)"),
            params![
                s.id,
                enum_to_sql(&s.kind),
                s.client,
                s.cwd,
                s.account,
                s.pid,
                ts_to_sql(&s.started_at),
                opt_ts_to_sql(&s.ended_at),
                s.status
            ],
        )?;
        Ok(())
    }

    pub fn get_session(&self, id: &str) -> Result<Option<SessionRow>> {
        let conn = self.conn();
        let mut st = conn.prepare(&format!("SELECT {COLS} FROM sessions WHERE id = ?1"))?;
        let mut rows = st.query_map([id], map)?;
        Ok(rows.next().transpose()?)
    }

    /// Newest first.
    pub fn list_sessions(&self, filter: SessionFilter, limit: usize) -> Result<Vec<SessionRow>> {
        let where_ = match filter {
            SessionFilter::Active => "WHERE ended_at IS NULL",
            SessionFilter::All => "",
        };
        let conn = self.conn();
        let mut st = conn.prepare(&format!(
            "SELECT {COLS} FROM sessions {where_} ORDER BY started_at DESC, id DESC LIMIT ?1"
        ))?;
        let rows = st.query_map([limit as i64], map)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Run counts and ledger cost of a session.
    pub fn session_stats(&self, session_id: &str) -> Result<SessionStats> {
        let conn = self.conn();
        let (runs, escalated, failed): (i64, i64, i64) = conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(state = 'escalated'),0), COALESCE(SUM(state = 'failed'),0) \
             FROM runs WHERE session_id = ?1",
            [session_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        let cost: f64 = conn.query_row(
            "SELECT COALESCE(SUM(cost),0.0) FROM ledger WHERE run_id IN \
             (SELECT id FROM runs WHERE session_id = ?1)",
            [session_id],
            |r| r.get(0),
        )?;
        Ok(SessionStats {
            runs,
            escalated,
            failed,
            cost,
        })
    }

    /// Marks a session ended with a final status.
    pub fn end_session(&self, id: &str, status: &str, now: DateTime<Utc>) -> Result<()> {
        let n = self.conn().execute(
            "UPDATE sessions SET ended_at = ?2, status = ?3 WHERE id = ?1",
            params![id, ts_to_sql(&now), status],
        )?;
        if n == 0 {
            return Err(StoreError::NotFound {
                entity: "session",
                id: id.into(),
            });
        }
        Ok(())
    }

    /// Ends open interactive sessions whose process is gone; returns how many ended.
    pub fn reap_dead_interactive_sessions(&self) -> Result<usize> {
        reap_dead_interactive(&self.conn())
    }

    /// Records when the session's process started (see `so_core::procinfo::process_start`).
    pub fn set_session_pid_start(&self, id: &str, start: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE sessions SET pid_start = ?2 WHERE id = ?1",
            params![id, start],
        )?;
        Ok(())
    }

    /// Open interactive sessions on `account` (they count toward its cap, SPEC 5.1).
    pub fn count_active_interactive_sessions_by_account(&self, account: &str) -> Result<i64> {
        count_interactive(&self.conn(), account)
    }

    pub fn find_active_session_by_pid(&self, pid: i64) -> Result<Option<SessionRow>> {
        let conn = self.conn();
        let mut st = conn.prepare(&format!(
            "SELECT {COLS} FROM sessions WHERE pid = ?1 AND ended_at IS NULL \
             ORDER BY started_at DESC LIMIT 1"
        ))?;
        let mut rows = st.query_map([pid], map)?;
        Ok(rows.next().transpose()?)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use so_core::{SessionKind, new_id};

    pub fn session(pid: Option<i64>) -> SessionRow {
        SessionRow {
            id: new_id(),
            kind: SessionKind::Orchestrator,
            client: "claude-code".into(),
            cwd: Some("/w".into()),
            account: None,
            pid,
            started_at: crate::codec::now(),
            ended_at: None,
            status: "active".into(),
        }
    }

    fn interactive(pid: i64, account: &str) -> SessionRow {
        let mut s = session(Some(pid));
        s.kind = SessionKind::Interactive;
        s.account = Some(account.into());
        s
    }

    #[test]
    fn cap_checks_reap_dead_interactive_sessions() {
        let s = Store::open_in_memory().unwrap();
        let me = i64::from(std::process::id());
        let live = interactive(me, "a");
        let dead = interactive(i64::from(i32::MAX), "a");
        let unknown = {
            let mut u = interactive(0, "a");
            u.pid = None;
            u
        };
        for x in [&live, &dead, &unknown] {
            s.insert_session(x).unwrap();
        }
        // The count itself frees the dead session's slot; no other call needed.
        assert_eq!(
            s.count_active_interactive_sessions_by_account("a").unwrap(),
            2
        );
        let ended = s.get_session(&dead.id).unwrap().unwrap();
        assert!(ended.ended_at.is_some());
        assert_eq!(ended.status, "ended");
        assert!(s.get_session(&live.id).unwrap().unwrap().ended_at.is_none());
        assert!(
            s.get_session(&unknown.id)
                .unwrap()
                .unwrap()
                .ended_at
                .is_none()
        );
        assert_eq!(s.reap_dead_interactive_sessions().unwrap(), 0);
    }

    #[test]
    fn a_reused_pid_is_reaped_by_its_start_time() {
        let s = Store::open_in_memory().unwrap();
        let me = i64::from(std::process::id());
        let reused = interactive(me, "a");
        s.insert_session(&reused).unwrap();
        s.set_session_pid_start(&reused.id, "not-my-start").unwrap();
        let mine = interactive(me, "a");
        s.insert_session(&mine).unwrap();
        s.set_session_pid_start(&mine.id, &so_core::procinfo::process_start(me).unwrap())
            .unwrap();
        assert_eq!(s.reap_dead_interactive_sessions().unwrap(), 1);
        assert!(
            s.get_session(&reused.id)
                .unwrap()
                .unwrap()
                .ended_at
                .is_some()
        );
        assert!(s.get_session(&mine.id).unwrap().unwrap().ended_at.is_none());
    }

    #[test]
    fn session_crud() {
        let s = Store::open_in_memory().unwrap();
        let a = session(Some(42));
        let b = session(None);
        s.insert_session(&a).unwrap();
        s.insert_session(&b).unwrap();
        assert_eq!(s.get_session(&a.id).unwrap().unwrap(), a);
        assert!(s.get_session("nope").unwrap().is_none());
        assert_eq!(s.find_active_session_by_pid(42).unwrap().unwrap().id, a.id);
        assert_eq!(
            s.list_sessions(SessionFilter::Active, 100).unwrap().len(),
            2
        );
        assert_eq!(s.list_sessions(SessionFilter::All, 1).unwrap().len(), 1);
        s.end_session(&a.id, "ended", crate::codec::now()).unwrap();
        let ended = s.get_session(&a.id).unwrap().unwrap();
        assert!(ended.ended_at.is_some());
        assert_eq!(ended.status, "ended");
        assert!(s.find_active_session_by_pid(42).unwrap().is_none());
        assert_eq!(
            s.list_sessions(SessionFilter::Active, 100).unwrap().len(),
            1
        );
        assert_eq!(s.list_sessions(SessionFilter::All, 100).unwrap().len(), 2);
        let mut i = session(None);
        i.kind = SessionKind::Interactive;
        i.account = Some("acc".into());
        s.insert_session(&i).unwrap();
        assert_eq!(
            s.count_active_interactive_sessions_by_account("acc")
                .unwrap(),
            1
        );
        assert_eq!(
            s.count_active_interactive_sessions_by_account("x").unwrap(),
            0
        );
        s.end_session(&i.id, "ended", crate::codec::now()).unwrap();
        assert_eq!(
            s.count_active_interactive_sessions_by_account("acc")
                .unwrap(),
            0
        );
        assert!(matches!(
            s.end_session("nope", "x", crate::codec::now()),
            Err(StoreError::NotFound { .. })
        ));
    }
}

//! Session queries.

use chrono::{DateTime, Utc};
use rusqlite::{Connection, Row, params};
use so_core::SessionKind;

use crate::codec::*;
use crate::error::{Result, StoreError};
use crate::rows::{SessionFilter, SessionRow};
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

pub(crate) fn count_interactive(conn: &Connection, account: &str) -> Result<i64> {
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

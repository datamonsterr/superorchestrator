//! Message queries.

use chrono::{DateTime, Utc};
use rusqlite::{Row, params};

use crate::codec::*;
use crate::error::Result;
use crate::rows::MessageRow;
use crate::store::Store;

const COLS: &str = "id, session_id, from_run, to_run, kind, body, created_at, read_at";

fn map(r: &Row<'_>) -> rusqlite::Result<MessageRow> {
    Ok(MessageRow {
        id: r.get(0)?,
        session_id: r.get(1)?,
        from_run: r.get(2)?,
        to_run: r.get(3)?,
        kind: get_enum(r, 4)?,
        body: r.get(5)?,
        created_at: get_ts(r, 6)?,
        read_at: get_opt_ts(r, 7)?,
    })
}

impl Store {
    pub fn insert_message(&self, m: &MessageRow) -> Result<()> {
        self.conn().execute(
            &format!("INSERT INTO messages ({COLS}) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)"),
            params![
                m.id,
                m.session_id,
                m.from_run,
                m.to_run,
                enum_to_sql(&m.kind),
                m.body,
                ts_to_sql(&m.created_at),
                opt_ts_to_sql(&m.read_at)
            ],
        )?;
        Ok(())
    }

    /// Unread messages of a session addressed to `to_run` (`None` = the orchestrator).
    pub fn unread_messages(
        &self,
        session_id: &str,
        to_run: Option<&str>,
        limit: usize,
    ) -> Result<Vec<MessageRow>> {
        let conn = self.conn();
        let mut st = conn.prepare(&format!(
            "SELECT {COLS} FROM messages WHERE session_id = ?1 AND to_run IS ?2 \
             AND read_at IS NULL ORDER BY created_at, id LIMIT ?3"
        ))?;
        let rows = st.query_map(params![session_id, to_run, limit as i64], map)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Messages sent to or from `run_id`, oldest first.
    pub fn list_messages_by_run(&self, run_id: &str, limit: usize) -> Result<Vec<MessageRow>> {
        let conn = self.conn();
        let mut st = conn.prepare(&format!(
            "SELECT {COLS} FROM messages WHERE to_run = ?1 OR from_run = ?1 \
             ORDER BY created_at, id LIMIT ?2"
        ))?;
        let rows = st.query_map(params![run_id, limit as i64], map)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Marks the given messages read and returns the ids this call actually changed. Two
    /// callers racing over the same ids get disjoint sets, so a message is handed to
    /// exactly one of them.
    pub fn claim_messages(&self, ids: &[String], now: DateTime<Utc>) -> Result<Vec<String>> {
        let now = ts_to_sql(&now);
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let mut claimed = Vec::new();
        {
            let mut st = tx.prepare(
                "UPDATE messages SET read_at = ?2 WHERE id = ?1 AND read_at IS NULL RETURNING id",
            )?;
            for id in ids {
                let mut rows = st.query(params![id, now])?;
                if let Some(r) = rows.next()? {
                    claimed.push(r.get::<_, String>(0)?);
                }
            }
        }
        tx.commit()?;
        Ok(claimed)
    }

    /// Puts messages back to unread (a delivery that failed after they were claimed).
    pub fn mark_messages_unread(&self, ids: &[String]) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        {
            let mut st = tx.prepare("UPDATE messages SET read_at = NULL WHERE id = ?1")?;
            for id in ids {
                st.execute(params![id])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Marks messages read now; returns how many were newly marked.
    pub fn mark_messages_read(&self, ids: &[String], now: DateTime<Utc>) -> Result<usize> {
        let now = ts_to_sql(&now);
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let mut n = 0;
        {
            let mut st =
                tx.prepare("UPDATE messages SET read_at = ?2 WHERE id = ?1 AND read_at IS NULL")?;
            for id in ids {
                n += st.execute(params![id, now])?;
            }
        }
        tx.commit()?;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use so_core::{MessageKind, new_id};

    fn msg(session: &str, to: Option<&str>) -> MessageRow {
        MessageRow {
            id: new_id(),
            session_id: session.into(),
            from_run: Some("child".into()),
            to_run: to.map(Into::into),
            kind: MessageKind::Question,
            body: "hi".into(),
            created_at: crate::codec::now(),
            read_at: None,
        }
    }

    #[test]
    fn claim_returns_only_what_it_flipped_and_unread_restores() {
        let s = Store::open_in_memory().unwrap();
        let a = msg("s", Some("r"));
        let b = msg("s", Some("r"));
        for m in [&a, &b] {
            s.insert_message(m).unwrap();
        }
        let ids = vec![a.id.clone(), b.id.clone()];
        let now = crate::codec::now();
        assert_eq!(s.claim_messages(&ids[..1], now).unwrap(), ids[..1]);
        // The second claimer gets only what the first left.
        assert_eq!(s.claim_messages(&ids, now).unwrap(), ids[1..]);
        assert!(s.claim_messages(&ids, now).unwrap().is_empty());
        s.mark_messages_unread(&ids[..1]).unwrap();
        assert_eq!(s.unread_messages("s", Some("r"), 10).unwrap(), vec![a]);
    }

    #[test]
    fn unread_and_mark() {
        let s = Store::open_in_memory().unwrap();
        let a = msg("s", None);
        let b = msg("s", Some("r1"));
        let c = msg("other", None);
        for m in [&a, &b, &c] {
            s.insert_message(m).unwrap();
        }
        assert_eq!(s.unread_messages("s", None, 10).unwrap(), vec![a.clone()]);
        assert_eq!(
            s.unread_messages("s", Some("r1"), 10).unwrap(),
            vec![b.clone()]
        );
        assert_eq!(
            s.mark_messages_read(std::slice::from_ref(&a.id), crate::codec::now())
                .unwrap(),
            1
        );
        assert_eq!(
            s.mark_messages_read(std::slice::from_ref(&a.id), crate::codec::now())
                .unwrap(),
            0
        );
        assert!(s.unread_messages("s", None, 10).unwrap().is_empty());
        assert_eq!(s.unread_messages("s", Some("r1"), 10).unwrap().len(), 1);
    }
}

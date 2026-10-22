//! Message queries.

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
    ) -> Result<Vec<MessageRow>> {
        let conn = self.conn();
        let mut st = conn.prepare(&format!(
            "SELECT {COLS} FROM messages WHERE session_id = ?1 AND to_run IS ?2 \
             AND read_at IS NULL ORDER BY created_at, id"
        ))?;
        let rows = st.query_map(params![session_id, to_run], map)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Marks messages read now; returns how many were newly marked.
    pub fn mark_messages_read(&self, ids: &[String]) -> Result<usize> {
        let now = ts_to_sql(&chrono::Utc::now());
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
    fn unread_and_mark() {
        let s = Store::open_in_memory().unwrap();
        let a = msg("s", None);
        let b = msg("s", Some("r1"));
        let c = msg("other", None);
        for m in [&a, &b, &c] {
            s.insert_message(m).unwrap();
        }
        assert_eq!(s.unread_messages("s", None).unwrap(), vec![a.clone()]);
        assert_eq!(s.unread_messages("s", Some("r1")).unwrap(), vec![b.clone()]);
        assert_eq!(
            s.mark_messages_read(std::slice::from_ref(&a.id)).unwrap(),
            1
        );
        assert_eq!(
            s.mark_messages_read(std::slice::from_ref(&a.id)).unwrap(),
            0
        );
        assert!(s.unread_messages("s", None).unwrap().is_empty());
        assert_eq!(s.unread_messages("s", Some("r1")).unwrap().len(), 1);
    }
}

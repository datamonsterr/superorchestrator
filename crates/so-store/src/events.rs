//! Event append and timeline queries.

use chrono::{DateTime, Utc};
use rusqlite::{Row, params};
use so_core::Event;

use crate::codec::*;
use crate::error::Result;
use crate::store::Store;

fn map(r: &Row<'_>) -> rusqlite::Result<Event> {
    let data: String = r.get(5)?;
    Ok(Event {
        id: r.get(0)?,
        session_id: r.get(1)?,
        run_id: r.get(2)?,
        ts: get_ts(r, 3)?,
        event_type: get_enum(r, 4)?,
        data: serde_json::from_str(&data).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(5, rusqlite::types::Type::Text, Box::new(e))
        })?,
    })
}

const COLS: &str = "id, session_id, run_id, ts, type, data_json";

impl Store {
    /// Appends an event and returns its id.
    pub fn append_event(&self, e: &Event) -> Result<String> {
        let data = serde_json::to_string(&e.data)?;
        self.conn().execute(
            &format!("INSERT INTO events ({COLS}) VALUES (?1,?2,?3,?4,?5,?6)"),
            params![
                e.id,
                e.session_id,
                e.run_id,
                ts_to_sql(&e.ts),
                enum_to_sql(&e.event_type),
                data
            ],
        )?;
        Ok(e.id.clone())
    }

    /// Events of a session in time order; `after_ts` is exclusive.
    pub fn list_events_by_session(
        &self,
        session_id: &str,
        after_ts: Option<DateTime<Utc>>,
        limit: usize,
    ) -> Result<Vec<Event>> {
        let after = after_ts.as_ref().map(ts_to_sql);
        let conn = self.conn();
        let mut st = conn.prepare_cached(&format!(
            "SELECT {COLS} FROM events WHERE session_id = ?1 AND (?2 IS NULL OR ts > ?2) \
             ORDER BY ts, id LIMIT ?3"
        ))?;
        let rows = st.query_map(params![session_id, after, limit as i64], map)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn list_events_by_run(&self, run_id: &str, limit: usize) -> Result<Vec<Event>> {
        let conn = self.conn();
        let mut st = conn.prepare_cached(&format!(
            "SELECT {COLS} FROM events WHERE run_id = ?1 ORDER BY ts, id LIMIT ?2"
        ))?;
        let rows = st.query_map(params![run_id, limit as i64], map)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use so_core::{EventType, new_id};

    fn ev(session: &str, run: Option<&str>, ts: DateTime<Utc>) -> Event {
        Event {
            id: new_id(),
            session_id: session.into(),
            run_id: run.map(Into::into),
            ts,
            event_type: EventType::Progress,
            data: serde_json::json!({"step": 1}),
        }
    }

    #[test]
    fn append_and_list() {
        let s = Store::open_in_memory().unwrap();
        let t0 = crate::codec::now();
        let e1 = ev("s1", Some("r1"), t0);
        let e2 = ev("s1", None, t0 + Duration::seconds(1));
        let e3 = ev("s2", Some("r1"), t0 + Duration::seconds(2));
        for e in [&e3, &e2, &e1] {
            assert_eq!(s.append_event(e).unwrap(), e.id);
        }
        let all = s.list_events_by_session("s1", None, 10).unwrap();
        assert_eq!(all, vec![e1.clone(), e2.clone()]);
        let after = s.list_events_by_session("s1", Some(e1.ts), 10).unwrap();
        assert_eq!(after, vec![e2]);
        assert_eq!(s.list_events_by_session("s1", None, 1).unwrap().len(), 1);
        let by_run = s.list_events_by_run("r1", 10).unwrap();
        assert_eq!(by_run, vec![e1, e3]);
    }
}

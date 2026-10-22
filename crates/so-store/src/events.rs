//! Event append and timeline queries.

use chrono::{DateTime, Utc};
use rusqlite::{Row, params};
use so_core::Event;

use crate::codec::*;
use crate::error::Result;
use crate::rows::EventQuery;
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

    /// Events of a session in `(ts, id)` order. `after` is an exclusive cursor, normally the
    /// `(ts, id)` of the last event already seen.
    pub fn list_events_by_session(
        &self,
        session_id: &str,
        after: Option<(DateTime<Utc>, &str)>,
        limit: usize,
    ) -> Result<Vec<Event>> {
        let conn = self.conn();
        let rows = match after {
            None => {
                let mut st = conn.prepare_cached(&format!(
                    "SELECT {COLS} FROM events WHERE session_id = ?1 ORDER BY ts, id LIMIT ?2"
                ))?;
                st.query_map(params![session_id, limit as i64], map)?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            }
            Some((ts, id)) => {
                let mut st = conn.prepare_cached(&format!(
                    "SELECT {COLS} FROM events WHERE session_id = ?1 \
                     AND (ts > ?2 OR (ts = ?2 AND id > ?3)) ORDER BY ts, id LIMIT ?4"
                ))?;
                st.query_map(params![session_id, ts_to_sql(&ts), id, limit as i64], map)?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            }
        };
        Ok(rows)
    }

    /// The newest event of `run_id` with the given type.
    pub fn latest_event_of_type(
        &self,
        run_id: &str,
        event_type: so_core::EventType,
    ) -> Result<Option<Event>> {
        let conn = self.conn();
        let mut st = conn.prepare_cached(&format!(
            "SELECT {COLS} FROM events WHERE run_id = ?1 AND type = ?2 \
             ORDER BY ts DESC, id DESC LIMIT 1"
        ))?;
        let mut rows = st.query_map(params![run_id, enum_to_sql(&event_type)], map)?;
        Ok(rows.next().transpose()?)
    }

    /// Deletes events older than `ts`; returns how many were removed.
    pub fn prune_events_before(&self, ts: DateTime<Utc>) -> Result<usize> {
        Ok(self
            .conn()
            .execute("DELETE FROM events WHERE ts < ?1", [ts_to_sql(&ts)])?)
    }

    /// Events matching `q`; the cursor `after` is exclusive in the chosen direction.
    pub fn query_events(&self, q: &EventQuery) -> Result<Vec<Event>> {
        use rusqlite::types::ToSql;
        let mut conds: Vec<String> = Vec::new();
        let mut args: Vec<Box<dyn ToSql>> = Vec::new();
        let mut add = |cond: &str, v: Box<dyn ToSql>| {
            args.push(v);
            conds.push(cond.replace('?', &format!("?{}", args.len())));
        };
        if let Some(s) = &q.session_id {
            add("session_id = ?", Box::new(s.clone()));
        }
        if let Some(r) = &q.run_id {
            add("run_id = ?", Box::new(r.clone()));
        }
        if let Some(t) = &q.event_type {
            add("type = ?", Box::new(enum_to_sql(t)));
        }
        if let Some((ts, id)) = &q.after {
            let (ts, id) = (ts_to_sql(ts), id.clone());
            let op = if q.newest_first { "<" } else { ">" };
            args.push(Box::new(ts));
            let n = args.len();
            args.push(Box::new(id));
            conds.push(format!(
                "(ts {op} ?{n} OR (ts = ?{n} AND id {op} ?{}))",
                n + 1
            ));
        }
        let where_ = if conds.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conds.join(" AND "))
        };
        let dir = if q.newest_first { "DESC" } else { "ASC" };
        args.push(Box::new(q.limit as i64));
        let sql = format!(
            "SELECT {COLS} FROM events {where_} ORDER BY ts {dir}, id {dir} LIMIT ?{}",
            args.len()
        );
        let conn = self.conn();
        let mut st = conn.prepare(&sql)?;
        let rows = st.query_map(
            rusqlite::params_from_iter(args.iter().map(|b| b.as_ref())),
            map,
        )?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The oldest event of `event_type` for a run, if any.
    pub fn first_event_of_type(
        &self,
        run_id: &str,
        event_type: &so_core::EventType,
    ) -> Result<Option<Event>> {
        let conn = self.conn();
        let mut st = conn.prepare_cached(&format!(
            "SELECT {COLS} FROM events WHERE run_id = ?1 AND type = ?2 ORDER BY ts, id LIMIT 1"
        ))?;
        let mut rows = st.query_map(params![run_id, enum_to_sql(event_type)], map)?;
        Ok(rows.next().transpose()?)
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
    fn query_events_filters_and_pages_both_ways() {
        let s = Store::open_in_memory().unwrap();
        let t0 = Utc::now();
        let mut all = Vec::new();
        for i in 0..5 {
            let mut e = ev(
                "s1",
                Some(if i % 2 == 0 { "r1" } else { "r2" }),
                t0 + Duration::seconds(i),
            );
            if i == 3 {
                e.event_type = EventType::Escalated;
            }
            s.append_event(&e).unwrap();
            all.push(e);
        }
        s.append_event(&ev("s2", Some("r9"), t0)).unwrap();
        let q = |f: &dyn Fn(&mut EventQuery)| {
            let mut q = EventQuery {
                limit: 100,
                ..Default::default()
            };
            f(&mut q);
            s.query_events(&q).unwrap()
        };
        assert_eq!(q(&|q| q.session_id = Some("s1".into())).len(), 5);
        assert_eq!(q(&|q| q.run_id = Some("r1".into())).len(), 3);
        let esc = q(&|q| q.event_type = Some(EventType::Escalated));
        assert_eq!(esc.len(), 1);
        assert_eq!(esc[0].id, all[3].id);
        let page = q(&|q| {
            q.session_id = Some("s1".into());
            q.after = Some((all[1].ts, all[1].id.clone()));
            q.limit = 2;
        });
        assert_eq!(
            page.iter().map(|e| &e.id).collect::<Vec<_>>(),
            [&all[2].id, &all[3].id]
        );
        let back = q(&|q| {
            q.session_id = Some("s1".into());
            q.newest_first = true;
            q.after = Some((all[3].ts, all[3].id.clone()));
            q.limit = 1;
        });
        assert_eq!(back[0].id, all[2].id);
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
        let after = s
            .list_events_by_session("s1", Some((e1.ts, e1.id.as_str())), 10)
            .unwrap();
        assert_eq!(after, vec![e2.clone()]);
        // same-timestamp events page by id
        let mut e4 = ev("s1", None, e1.ts);
        e4.id = format!("{}z", e1.id);
        s.append_event(&e4).unwrap();
        let after = s
            .list_events_by_session("s1", Some((e1.ts, e1.id.as_str())), 10)
            .unwrap();
        assert_eq!(after, vec![e4, e2.clone()]);
        assert_eq!(s.list_events_by_session("s1", None, 1).unwrap().len(), 1);
        let by_run = s.list_events_by_run("r1", 10).unwrap();
        assert_eq!(by_run, vec![e1, e3.clone()]);
        assert_eq!(s.prune_events_before(e2.ts).unwrap(), 2);
        assert_eq!(s.list_events_by_session("s1", None, 10).unwrap(), vec![e2]);
        assert_eq!(s.list_events_by_session("s2", None, 10).unwrap(), vec![e3]);
    }
}

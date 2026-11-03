//! Read-only routing history for insight reports.
use crate::codec::{get_enum, get_opt_enum, get_opt_ts, get_ts};
use crate::{Result, Store};
use chrono::{DateTime, Utc};
use serde::Serialize;
use so_core::{ErrorKind, RunState};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HistoryRow {
    pub run_id: String,
    pub prompt: Option<String>,
    pub state: RunState,
    pub error_kind: Option<ErrorKind>,
    pub choice_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub complexity: Option<f64>,
    pub level: Option<String>,
    pub source: Option<String>,
    pub reviews_accepted: usize,
    pub reviews_rejected: usize,
    pub failovers: usize,
    pub escalations: usize,
}

impl Store {
    /// Returns routed runs newest first, with their latest decision and event totals.
    pub fn routing_history(&self, limit: usize) -> Result<Vec<HistoryRow>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            r#"
            WITH selected AS MATERIALIZED (
                SELECT r.*, (SELECT e.data_json FROM events e
                    WHERE e.run_id = r.id AND e.type = 'route_decision'
                    ORDER BY e.ts DESC, e.id DESC LIMIT 1) AS decision_json
                FROM runs r
                WHERE EXISTS (SELECT 1 FROM events e
                    WHERE e.run_id = r.id AND e.type = 'route_decision')
                ORDER BY r.created_at DESC, r.id DESC LIMIT ?1
            )
            SELECT r.id, r.prompt, r.state, r.error_kind, r.choice_id, r.created_at, r.started_at,
                r.finished_at, r.decision_json,
                COALESCE(SUM(e.type = 'review_accepted'), 0),
                COALESCE(SUM(e.type = 'review_rejected'), 0),
                COALESCE(SUM(e.type = 'failover'), 0),
                COALESCE(SUM(e.type = 'escalated'), 0)
            FROM selected r LEFT JOIN events e ON e.run_id = r.id
                AND e.type IN ('review_accepted', 'review_rejected', 'failover', 'escalated')
            GROUP BY r.id
            ORDER BY r.created_at DESC, r.id DESC
        "#,
        )?;
        let rows = stmt.query_map([i64::try_from(limit).unwrap_or(i64::MAX)], |r| {
            let data: String = r.get(8)?;
            let value: serde_json::Value = serde_json::from_str(&data).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    8,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?;
            let decision = if value.get("complexity").is_some() {
                &value
            } else {
                value.get("result").unwrap_or(&value)
            };
            Ok(HistoryRow {
                run_id: r.get(0)?,
                prompt: r.get(1)?,
                state: get_enum(r, 2)?,
                error_kind: get_opt_enum(r, 3)?,
                choice_id: r.get(4)?,
                created_at: get_ts(r, 5)?,
                started_at: get_opt_ts(r, 6)?,
                finished_at: get_opt_ts(r, 7)?,
                complexity: decision.get("complexity").and_then(|v| v.as_f64()),
                level: decision
                    .get("level")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned),
                source: decision
                    .get("source")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned),
                reviews_accepted: r.get::<_, u32>(9)? as usize,
                reviews_rejected: r.get::<_, u32>(10)? as usize,
                failovers: r.get::<_, u32>(11)? as usize,
                escalations: r.get::<_, u32>(12)? as usize,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use so_core::{Event, EventType, new_id};

    #[test]
    fn history_uses_latest_route_counts_and_run_order() {
        let store = Store::open_in_memory().unwrap();
        let session = crate::sessions::tests::session(None);
        store.insert_session(&session).unwrap();
        let old = crate::runs::tests::run(&session.id, None);
        let mut new = crate::runs::tests::run(&session.id, None);
        new.created_at = old.created_at + Duration::seconds(1);
        let mut unrouted = crate::runs::tests::run(&session.id, None);
        unrouted.created_at = new.created_at + Duration::seconds(1);
        for run in [&old, &new, &unrouted] {
            store.insert_run(run).unwrap();
        }
        let append = |run: &str, ty, seconds, data| {
            store
                .append_event(&Event {
                    id: new_id(),
                    session_id: session.id.clone(),
                    run_id: Some(run.into()),
                    ts: old.created_at + Duration::seconds(seconds),
                    event_type: ty,
                    data,
                })
                .unwrap();
        };
        append(
            &old.id,
            EventType::RouteDecision,
            0,
            serde_json::json!({"complexity": 12}),
        );
        append(
            &new.id,
            EventType::RouteDecision,
            0,
            serde_json::json!({"complexity": 20}),
        );
        append(
            &new.id,
            EventType::RouteDecision,
            1,
            serde_json::json!({"result": {"complexity": 50, "level": "mid", "source": "jev"}}),
        );
        for ty in [
            EventType::ReviewAccepted,
            EventType::ReviewRejected,
            EventType::ReviewRejected,
            EventType::Failover,
            EventType::Escalated,
        ] {
            append(&new.id, ty, 2, serde_json::json!({}));
        }
        let rows = store.routing_history(1).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].run_id, new.id);
        assert_eq!(rows[0].complexity, Some(50.0));
        assert_eq!(rows[0].level.as_deref(), Some("mid"));
        assert_eq!(
            (
                rows[0].reviews_accepted,
                rows[0].reviews_rejected,
                rows[0].failovers,
                rows[0].escalations
            ),
            (1, 2, 1, 1)
        );
        let all = store.routing_history(10).unwrap();
        assert_eq!(all[1].run_id, old.id);
        assert_eq!(
            (
                all[1].reviews_accepted,
                all[1].reviews_rejected,
                all[1].failovers,
                all[1].escalations
            ),
            (0, 0, 0, 0)
        );
        assert!(store.routing_history(0).unwrap().is_empty());
    }
    #[test]
    fn malformed_decision_json_returns_error() {
        let store = Store::open_in_memory().unwrap();
        let session = crate::sessions::tests::session(None);
        store.insert_session(&session).unwrap();
        let run = crate::runs::tests::run(&session.id, None);
        store.insert_run(&run).unwrap();
        store
            .append_event(&Event {
                id: new_id(),
                session_id: session.id,
                run_id: Some(run.id),
                ts: run.created_at,
                event_type: EventType::RouteDecision,
                data: serde_json::json!({}),
            })
            .unwrap();
        store
            .conn()
            .execute(
                "UPDATE events SET data_json = 'invalid' WHERE type = 'route_decision'",
                [],
            )
            .unwrap();
        assert!(matches!(
            store.routing_history(10),
            Err(crate::StoreError::Sqlite(
                rusqlite::Error::FromSqlConversionFailure(8, _, _)
            ))
        ));
    }
}

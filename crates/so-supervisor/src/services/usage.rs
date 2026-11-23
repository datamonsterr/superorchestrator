//! Keep shared headroom current even when no runs are producing usage events.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;

use crate::SupervisorState;

pub(crate) async fn refresh_loop(state: Arc<SupervisorState>, mut shutdown: watch::Receiver<bool>) {
    let Some(host) = state.host.clone() else {
        return;
    };
    let refresher = so_usage::active_refresher(state.store.clone(), state.config.clone(), host);
    // Discover newly added accounts promptly; the refresher applies the per-account TTL.
    let period = state
        .config
        .usage
        .probe_ttl
        .as_duration()
        .clamp(Duration::from_secs(1), Duration::from_secs(30));
    let mut ticks = tokio::time::interval(period);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            _ = shutdown.changed() => return,
            _ = ticks.tick() => {}
        }
        tokio::select! {
            biased;
            _ = shutdown.changed() => return,
            errors = refresher.refresh(&state.paths) => {
                for error in errors {
                    tracing::warn!("{error}");
                }
            }
        }
    }
}

/// Aggregated ledger usage for one UTC day.
#[derive(Debug, Clone, PartialEq)]
pub struct DailyUsage {
    pub day: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost: f64,
    pub by_account: std::collections::BTreeMap<String, f64>,
}

/// Read and aggregate daily ledger totals since the given instant.
pub fn daily(
    store: &so_store::Store,
    since: chrono::DateTime<chrono::Utc>,
) -> so_store::Result<Vec<DailyUsage>> {
    let mut out: Vec<DailyUsage> = Vec::new();
    for r in store.ledger_daily(since)? {
        if out.last().is_none_or(|d| d.day != r.day) {
            out.push(DailyUsage {
                day: r.day.clone(),
                input_tokens: 0,
                output_tokens: 0,
                cost: 0.0,
                by_account: Default::default(),
            });
        }
        if let Some(d) = out.last_mut() {
            d.input_tokens += u64::try_from(r.input_tokens).unwrap_or(0);
            d.output_tokens += u64::try_from(r.output_tokens).unwrap_or(0);
            d.cost += r.cost;
            *d.by_account.entry(r.account).or_default() += r.cost;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone};
    use so_store::LedgerEntry;

    #[test]
    fn daily_groups_accounts_and_days_and_respects_since() {
        let store = so_store::Store::open_in_memory().unwrap();
        let now = chrono::Utc
            .with_ymd_and_hms(2026, 10, 23, 12, 0, 0)
            .unwrap();
        for (account, ts, input, output, cost) in [
            ("a", now - Duration::days(2), 100, 20, 1.0),
            ("a", now, 10, 2, 0.25),
            ("b", now, 30, 3, 0.5),
            ("a", now + Duration::days(1), 40, 4, 0.75),
        ] {
            store
                .insert_ledger(&LedgerEntry {
                    account: account.into(),
                    pool: "default".into(),
                    run_id: None,
                    ts,
                    input_tokens: input,
                    output_tokens: output,
                    cost,
                })
                .unwrap();
        }
        let rows = daily(&store, now - Duration::days(1)).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(
            (
                rows[0].day.as_str(),
                rows[0].input_tokens,
                rows[0].output_tokens,
                rows[0].cost
            ),
            ("2026-10-23", 40, 5, 0.75)
        );
        assert_eq!(rows[0].by_account["a"], 0.25);
        assert_eq!(rows[0].by_account["b"], 0.5);
        assert_eq!(rows[1].day, "2026-10-24");
    }
}

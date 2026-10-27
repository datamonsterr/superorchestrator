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

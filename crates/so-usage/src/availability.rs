//! What the router needs: is (account, pool) usable now, and how much room does it have.

use chrono::{DateTime, Utc};
use so_core::HeadroomStatus;
use so_store::Store;

use crate::error::Result;
use crate::headroom::{DEFAULT_LOW_PCT, effective_with, headroom_score};

#[derive(Debug, Clone, PartialEq)]
pub struct Availability {
    pub available: bool,
    pub status: HeadroomStatus,
    /// Smallest known remaining percentage; `None` when unknown.
    pub score: Option<f64>,
    pub reason: Option<String>,
}

/// Combines the flag and the effective headroom. `Exhausted` is unavailable; `Low` is not.
pub fn availability(
    store: &Store,
    account: &str,
    pool: &str,
    now: DateTime<Utc>,
) -> Result<Availability> {
    let flag = store.get_flag(account, pool)?.filter(|f| f.until > now);
    let record = store.get_headroom(account, pool)?;
    let eff = record
        .as_ref()
        .map(|h| effective_with(h, DEFAULT_LOW_PCT, flag.is_some(), now));
    let score = record.as_ref().and_then(|h| headroom_score(h, now));
    let status = match (&eff, &flag) {
        (Some(e), _) => e.status,
        (None, Some(_)) => HeadroomStatus::Exhausted,
        (None, None) => HeadroomStatus::Unknown,
    };
    let reason = match (&flag, status) {
        (Some(f), _) => Some(f.reason.clone()),
        (None, HeadroomStatus::Exhausted) => Some("window exhausted until reset".into()),
        _ => None,
    };
    Ok(Availability {
        available: status != HeadroomStatus::Exhausted,
        status,
        score,
        reason,
    })
}

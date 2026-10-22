//! Marking and checking exhaustion (SPEC 6.3).

use std::time::Duration as StdDuration;

use chrono::{DateTime, Duration, Utc};
use so_core::{Headroom, HeadroomSource, HeadroomStatus};
use so_store::{FlagRow, Store};

use crate::availability::availability;
use crate::error::Result;

/// Flags (account, pool) until `reset` (when in the future) or `now + default_cooldown`.
/// The flag is the source of truth; the stored status is set to `Exhausted` for display only
/// and `observed_at` is left alone so probe staleness is unaffected. Returns the expiry.
pub fn mark_exhausted(
    store: &Store,
    account: &str,
    pool: &str,
    reset: Option<DateTime<Utc>>,
    default_cooldown: StdDuration,
    reason: &str,
    now: DateTime<Utc>,
) -> Result<DateTime<Utc>> {
    let cooldown = Duration::from_std(default_cooldown).unwrap_or(Duration::hours(5));
    let until = reset.filter(|r| *r > now).unwrap_or(now + cooldown);
    store.set_flag(&FlagRow {
        account: account.into(),
        pool: pool.into(),
        until,
        reason: reason.into(),
    })?;
    let mut h = store.get_headroom(account, pool)?.unwrap_or(Headroom {
        account: account.into(),
        pool: pool.into(),
        five_hour_remaining_pct: None,
        five_hour_resets_at: None,
        weekly_remaining_pct: None,
        weekly_resets_at: None,
        status: HeadroomStatus::Unknown,
        source: HeadroomSource::Ledger,
        // Epoch: a record made here carries no observation, so it is always stale.
        observed_at: DateTime::UNIX_EPOCH,
    });
    h.status = HeadroomStatus::Exhausted;
    store.upsert_headroom(&h)?;
    Ok(until)
}

/// True when no flag is active and the effective headroom is not exhausted.
pub fn is_available(store: &Store, account: &str, pool: &str, now: DateTime<Utc>) -> Result<bool> {
    Ok(availability(store, account, pool, now)?.available)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::availability::Availability;

    const COOL: StdDuration = StdDuration::from_secs(5 * 3600);

    #[test]
    fn default_cooldown_then_expiry() {
        let s = Store::open_in_memory().unwrap();
        let now = Utc::now();
        assert!(is_available(&s, "a", "default", now).unwrap());
        let until = mark_exhausted(&s, "a", "default", None, COOL, "quota", now).unwrap();
        assert_eq!(until, now + Duration::hours(5));
        assert!(!is_available(&s, "a", "default", now).unwrap());
        assert!(!is_available(&s, "a", "default", now + Duration::hours(4)).unwrap());
        let later = now + Duration::hours(5) + Duration::seconds(1);
        assert!(is_available(&s, "a", "default", later).unwrap());
        let a = availability(&s, "a", "default", later).unwrap();
        assert_eq!(a.status, HeadroomStatus::Unknown);
    }

    #[test]
    fn does_not_bump_observed_at_or_persist_exhaustion() {
        let s = Store::open_in_memory().unwrap();
        let now = Utc::now();
        let seen = now - Duration::minutes(2);
        let mut h = Headroom {
            account: "a".into(),
            pool: "p".into(),
            five_hour_remaining_pct: Some(60.0),
            five_hour_resets_at: Some(now + Duration::hours(1)),
            weekly_remaining_pct: Some(40.0),
            weekly_resets_at: Some(now + Duration::days(2)),
            status: HeadroomStatus::Ok,
            source: HeadroomSource::Probe,
            observed_at: seen,
        };
        h.observed_at = DateTime::from_timestamp(seen.timestamp(), 0).unwrap();
        s.upsert_headroom(&h).unwrap();
        mark_exhausted(&s, "a", "p", None, COOL, "quota", now).unwrap();
        assert_eq!(
            s.get_headroom("a", "p").unwrap().unwrap().observed_at,
            h.observed_at
        );
        let a = availability(&s, "a", "p", now).unwrap();
        assert_eq!(
            a,
            Availability {
                available: false,
                status: HeadroomStatus::Exhausted,
                score: Some(40.0),
                reason: Some("quota".into())
            }
        );
        // Expired flag, healthy windows: available again though the stored status says Exhausted.
        let later = now + Duration::hours(6);
        let a = availability(&s, "a", "p", later).unwrap();
        assert!(a.available);
        assert_ne!(a.status, HeadroomStatus::Exhausted);
        // A fresh record made by mark_exhausted is stale for TTL purposes.
        mark_exhausted(&s, "n", "p", None, COOL, "q", now).unwrap();
        assert_eq!(
            s.get_headroom("n", "p").unwrap().unwrap().observed_at,
            DateTime::UNIX_EPOCH
        );
    }

    #[test]
    fn reset_time_used_and_past_reset_ignored() {
        let s = Store::open_in_memory().unwrap();
        let now = Utc::now();
        let reset = now + Duration::hours(2);
        assert_eq!(
            mark_exhausted(&s, "a", "p", Some(reset), COOL, "q", now).unwrap(),
            reset
        );
        assert!(!is_available(&s, "a", "p", now + Duration::hours(1)).unwrap());
        assert!(is_available(&s, "a", "p", reset + Duration::seconds(1)).unwrap());
        let until =
            mark_exhausted(&s, "b", "p", Some(now - Duration::hours(1)), COOL, "q", now).unwrap();
        assert_eq!(until, now + Duration::hours(5));
    }

    #[test]
    fn probed_zero_window_blocks_until_reset_and_low_stays_available() {
        let s = Store::open_in_memory().unwrap();
        let now = Utc::now();
        let mut h = Headroom {
            account: "a".into(),
            pool: "p".into(),
            five_hour_remaining_pct: None,
            five_hour_resets_at: None,
            weekly_remaining_pct: Some(0.0),
            weekly_resets_at: Some(now + Duration::hours(3)),
            status: HeadroomStatus::Exhausted,
            source: HeadroomSource::Probe,
            observed_at: now,
        };
        s.upsert_headroom(&h).unwrap();
        assert!(!is_available(&s, "a", "p", now).unwrap());
        assert!(is_available(&s, "a", "p", now + Duration::hours(4)).unwrap());
        h.weekly_remaining_pct = Some(5.0);
        h.status = HeadroomStatus::Low;
        s.upsert_headroom(&h).unwrap();
        let a = availability(&s, "a", "p", now).unwrap();
        assert_eq!(
            (a.available, a.status, a.score),
            (true, HeadroomStatus::Low, Some(5.0))
        );
    }
}

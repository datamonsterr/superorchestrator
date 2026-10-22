//! Marking and checking exhaustion (SPEC 6.3).

use std::time::Duration as StdDuration;

use chrono::{DateTime, Duration, Utc};
use so_core::{Headroom, HeadroomSource, HeadroomStatus};
use so_store::{FlagRow, Store};

use crate::error::Result;

/// Flags (account, pool) until `reset` (when in the future) or `now + default_cooldown`, and
/// records headroom status `Exhausted`. Returns the flag's expiry.
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
        observed_at: now,
    });
    h.status = HeadroomStatus::Exhausted;
    h.observed_at = now;
    store.upsert_headroom(&h)?;
    Ok(until)
}

/// True when no flag is active and the recorded exhaustion (if any) has passed its reset.
pub fn is_available(store: &Store, account: &str, pool: &str, now: DateTime<Utc>) -> Result<bool> {
    if store
        .get_flag(account, pool)?
        .is_some_and(|f| f.until > now)
    {
        return Ok(false);
    }
    let Some(h) = store.get_headroom(account, pool)? else {
        return Ok(true);
    };
    if h.status != HeadroomStatus::Exhausted {
        return Ok(true);
    }
    // Exhausted without an active flag: blocked only by a depleted window that has not reset.
    let blocked = |pct: Option<f64>, reset: Option<DateTime<Utc>>| {
        pct.is_some_and(|p| p <= 0.0) && reset.is_none_or(|r| r > now)
    };
    Ok(!(blocked(h.five_hour_remaining_pct, h.five_hour_resets_at)
        || blocked(h.weekly_remaining_pct, h.weekly_resets_at)))
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(
            is_available(
                &s,
                "a",
                "default",
                now + Duration::hours(5) + Duration::seconds(1)
            )
            .unwrap()
        );
        assert_eq!(
            s.get_headroom("a", "default").unwrap().unwrap().status,
            HeadroomStatus::Exhausted
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
    fn probed_zero_window_blocks_until_reset() {
        let s = Store::open_in_memory().unwrap();
        let now = Utc::now();
        s.upsert_headroom(&Headroom {
            account: "a".into(),
            pool: "p".into(),
            five_hour_remaining_pct: None,
            five_hour_resets_at: None,
            weekly_remaining_pct: Some(0.0),
            weekly_resets_at: Some(now + Duration::hours(3)),
            status: HeadroomStatus::Exhausted,
            source: HeadroomSource::Probe,
            observed_at: now,
        })
        .unwrap();
        assert!(!is_available(&s, "a", "p", now).unwrap());
        assert!(is_available(&s, "a", "p", now + Duration::hours(4)).unwrap());
    }
}

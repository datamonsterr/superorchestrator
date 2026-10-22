//! Status computation and record merging (SPEC 6.2).

use chrono::{DateTime, Utc};
use so_core::{Headroom, HeadroomSource, HeadroomStatus};

use crate::parse::{Window, WindowReading};

/// Exhausted when any known window has nothing left, the (account, pool) is `flagged`, or
/// `exhausted_until` is still in the future; low when any window is under `low_pct`;
/// unknown without data; otherwise ok. Disabled windows (`None`) are ignored.
pub fn compute_status(
    readings: &[WindowReading],
    low_pct: f64,
    flagged: bool,
    exhausted_until: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> HeadroomStatus {
    if flagged || exhausted_until.is_some_and(|u| u > now) {
        return HeadroomStatus::Exhausted;
    }
    let known: Vec<f64> = readings.iter().filter_map(|r| r.remaining_pct).collect();
    if known.is_empty() {
        HeadroomStatus::Unknown
    } else if known.iter().any(|p| *p <= 0.0) {
        HeadroomStatus::Exhausted
    } else if known.iter().any(|p| *p < low_pct) {
        HeadroomStatus::Low
    } else {
        HeadroomStatus::Ok
    }
}

/// Builds the record of one (account, pool) from readings (other pools are ignored). The
/// status ignores flags; callers apply them with [`compute_status`] or `is_available`.
pub fn merge_into_record(
    account: &str,
    pool: &str,
    readings: &[WindowReading],
    low_pct: f64,
    source: HeadroomSource,
    observed_at: DateTime<Utc>,
) -> Headroom {
    let mine: Vec<WindowReading> = readings
        .iter()
        .filter(|r| r.pool == pool)
        .cloned()
        .collect();
    let last = |w: Window| mine.iter().rev().find(|r| r.window == w);
    Headroom {
        account: account.into(),
        pool: pool.into(),
        five_hour_remaining_pct: last(Window::FiveHour).and_then(|r| r.remaining_pct),
        five_hour_resets_at: last(Window::FiveHour).and_then(|r| r.resets_at),
        weekly_remaining_pct: last(Window::Weekly).and_then(|r| r.remaining_pct),
        weekly_resets_at: last(Window::Weekly).and_then(|r| r.resets_at),
        status: compute_status(&mine, low_pct, false, None, observed_at),
        source,
        observed_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn r(w: Window, p: Option<f64>) -> WindowReading {
        WindowReading {
            pool: "default".into(),
            window: w,
            remaining_pct: p,
            resets_at: None,
        }
    }

    #[test]
    fn thresholds() {
        let now = Utc::now();
        let st = |rs: &[WindowReading], f, u| compute_status(rs, 15.0, f, u, now);
        let ok = [
            r(Window::FiveHour, Some(80.0)),
            r(Window::Weekly, Some(50.0)),
        ];
        assert_eq!(st(&ok, false, None), HeadroomStatus::Ok);
        let low = [
            r(Window::FiveHour, Some(80.0)),
            r(Window::Weekly, Some(14.9)),
        ];
        assert_eq!(st(&low, false, None), HeadroomStatus::Low);
        let ex = [
            r(Window::FiveHour, Some(0.0)),
            r(Window::Weekly, Some(50.0)),
        ];
        assert_eq!(st(&ex, false, None), HeadroomStatus::Exhausted);
        assert_eq!(st(&[], false, None), HeadroomStatus::Unknown);
    }

    #[test]
    fn flag_forces_exhausted_and_disabled_ignored() {
        let now = Utc::now();
        let ok = [r(Window::FiveHour, Some(80.0))];
        assert_eq!(
            compute_status(&ok, 15.0, true, None, now),
            HeadroomStatus::Exhausted
        );
        let future = Some(now + Duration::hours(1));
        assert_eq!(
            compute_status(&ok, 15.0, false, future, now),
            HeadroomStatus::Exhausted
        );
        let past = Some(now - Duration::hours(1));
        assert_eq!(
            compute_status(&ok, 15.0, false, past, now),
            HeadroomStatus::Ok
        );
        let dis = [r(Window::FiveHour, None), r(Window::Weekly, Some(60.0))];
        assert_eq!(
            compute_status(&dis, 15.0, false, None, now),
            HeadroomStatus::Ok
        );
        let all_dis = [r(Window::FiveHour, None)];
        assert_eq!(
            compute_status(&all_dis, 15.0, false, None, now),
            HeadroomStatus::Unknown
        );
    }

    #[test]
    fn merge_filters_pool() {
        let now = Utc::now();
        let mut g = r(Window::Weekly, Some(92.0));
        g.pool = "gemini".into();
        let mut c = r(Window::Weekly, Some(1.0));
        c.pool = "claude".into();
        let h = merge_into_record("a", "gemini", &[g, c], 15.0, HeadroomSource::Probe, now);
        assert_eq!(h.weekly_remaining_pct, Some(92.0));
        assert_eq!(h.five_hour_remaining_pct, None);
        assert_eq!(h.status, HeadroomStatus::Ok);
        assert_eq!(h.source, HeadroomSource::Probe);
    }
}

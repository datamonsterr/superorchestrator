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

/// Default `usage.low_pct`, used where no config is at hand.
pub const DEFAULT_LOW_PCT: f64 = 15.0;

/// Partial update (passive events): windows present in `readings` replace the stored ones,
/// the other window is kept. Status is derived from the merged windows.
pub fn merge_partial(
    existing: Option<&Headroom>,
    account: &str,
    pool: &str,
    readings: &[WindowReading],
    low_pct: f64,
    source: HeadroomSource,
    now: DateTime<Utc>,
) -> Headroom {
    let mut h = existing.cloned().unwrap_or(Headroom {
        account: account.into(),
        pool: pool.into(),
        five_hour_remaining_pct: None,
        five_hour_resets_at: None,
        weekly_remaining_pct: None,
        weekly_resets_at: None,
        status: HeadroomStatus::Unknown,
        source,
        observed_at: now,
    });
    for r in readings.iter().filter(|r| r.pool == pool) {
        match r.window {
            Window::FiveHour => {
                h.five_hour_remaining_pct = r.remaining_pct;
                h.five_hour_resets_at = r.resets_at;
            }
            Window::Weekly => {
                h.weekly_remaining_pct = r.remaining_pct;
                h.weekly_resets_at = r.resets_at;
            }
        }
    }
    h.source = source;
    h.observed_at = now;
    h.status = effective_with(&h, low_pct, false, now).status;
    h
}

/// The record as it reads at `now`: a window whose reset time has passed counts as fully reset
/// (100%, no reset time), and the status is derived from the windows, not trusted from storage.
/// A stored `Exhausted` with no window data (set by `mark_exhausted`) reads as unknown; flags
/// decide that case (see `availability`).
pub fn effective(h: &Headroom, now: DateTime<Utc>) -> Headroom {
    effective_with(h, DEFAULT_LOW_PCT, false, now)
}

/// [`effective`] with an explicit `low_pct` and flag state (`flagged` forces `Exhausted`).
pub fn effective_with(h: &Headroom, low_pct: f64, flagged: bool, now: DateTime<Utc>) -> Headroom {
    let mut e = h.clone();
    let reset = |pct: &mut Option<f64>, at: &mut Option<DateTime<Utc>>| {
        if at.is_some_and(|a| a <= now) {
            if pct.is_some() {
                *pct = Some(100.0);
            }
            *at = None;
        }
    };
    reset(&mut e.five_hour_remaining_pct, &mut e.five_hour_resets_at);
    reset(&mut e.weekly_remaining_pct, &mut e.weekly_resets_at);
    let readings: Vec<WindowReading> = [
        (
            Window::FiveHour,
            e.five_hour_remaining_pct,
            e.five_hour_resets_at,
        ),
        (Window::Weekly, e.weekly_remaining_pct, e.weekly_resets_at),
    ]
    .into_iter()
    .map(|(window, remaining_pct, resets_at)| WindowReading {
        pool: e.pool.clone(),
        window,
        remaining_pct,
        resets_at,
    })
    .collect();
    e.status = if flagged || readings.iter().any(|r| r.remaining_pct.is_some()) {
        compute_status(&readings, low_pct, flagged, None, now)
    } else if h.status == HeadroomStatus::Exhausted {
        HeadroomStatus::Unknown
    } else {
        h.status
    };
    e
}

/// Smallest known remaining percentage of the record at `now`, for ranking.
pub fn headroom_score(h: &Headroom, now: DateTime<Utc>) -> Option<f64> {
    let e = effective(h, now);
    [e.five_hour_remaining_pct, e.weekly_remaining_pct]
        .into_iter()
        .flatten()
        .min_by(f64::total_cmp)
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

    fn rec(five: Option<f64>, week: Option<f64>) -> Headroom {
        Headroom {
            account: "a".into(),
            pool: "p".into(),
            five_hour_remaining_pct: five,
            five_hour_resets_at: None,
            weekly_remaining_pct: week,
            weekly_resets_at: None,
            status: HeadroomStatus::Ok,
            source: HeadroomSource::Probe,
            observed_at: Utc::now(),
        }
    }

    #[test]
    fn partial_merge_keeps_other_window() {
        let now = Utc::now();
        let old = rec(Some(80.0), Some(50.0));
        let ev = [WindowReading {
            pool: "p".into(),
            window: Window::FiveHour,
            remaining_pct: Some(10.0),
            resets_at: None,
        }];
        let h = merge_partial(Some(&old), "a", "p", &ev, 15.0, HeadroomSource::Event, now);
        assert_eq!(h.five_hour_remaining_pct, Some(10.0));
        assert_eq!(h.weekly_remaining_pct, Some(50.0));
        assert_eq!(
            (h.status, h.source),
            (HeadroomStatus::Low, HeadroomSource::Event)
        );
    }

    #[test]
    fn past_reset_reads_as_full() {
        let now = Utc::now();
        let mut h = rec(Some(0.0), Some(50.0));
        h.five_hour_resets_at = Some(now - Duration::minutes(1));
        h.status = HeadroomStatus::Exhausted;
        let e = effective(&h, now);
        assert_eq!(
            (e.five_hour_remaining_pct, e.five_hour_resets_at),
            (Some(100.0), None)
        );
        assert_eq!(e.status, HeadroomStatus::Ok);
        h.five_hour_resets_at = Some(now + Duration::minutes(1));
        assert_eq!(effective(&h, now).status, HeadroomStatus::Exhausted);
        assert_eq!(headroom_score(&h, now), Some(0.0));
        assert_eq!(headroom_score(&rec(None, None), now), None);
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

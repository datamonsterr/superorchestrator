//! Local ledger windows (SPEC 6.1). Limits are unknowable here, so percentages stay `None`.

use chrono::{DateTime, Duration, Utc};
use so_core::{Headroom, HeadroomSource, HeadroomStatus};
use so_store::{LedgerSums, Store};

use crate::error::Result;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct LedgerWindows {
    pub five_hour: LedgerSums,
    pub weekly: LedgerSums,
}

impl LedgerWindows {
    pub fn load(store: &Store, account: &str, pool: &str, now: DateTime<Utc>) -> Result<Self> {
        Ok(Self {
            five_hour: store.ledger_sums(account, pool, now - Duration::hours(5))?,
            weekly: store.ledger_sums(account, pool, now - Duration::days(7))?,
        })
    }
}

/// Headroom derived from the ledger alone: exhausted while flagged, ok when the account ran in
/// the last 7 days without a flag, unknown with no history. No percentages.
pub fn ledger_headroom(
    store: &Store,
    account: &str,
    pool: &str,
    now: DateTime<Utc>,
) -> Result<Headroom> {
    let windows = LedgerWindows::load(store, account, pool, now)?;
    let flagged = store
        .get_flag(account, pool)?
        .is_some_and(|f| f.until > now);
    let status = if flagged {
        HeadroomStatus::Exhausted
    } else if windows.weekly.runs > 0 {
        HeadroomStatus::Ok
    } else {
        HeadroomStatus::Unknown
    };
    Ok(Headroom {
        account: account.into(),
        pool: pool.into(),
        five_hour_remaining_pct: None,
        five_hour_resets_at: None,
        weekly_remaining_pct: None,
        weekly_resets_at: None,
        status,
        source: HeadroomSource::Ledger,
        observed_at: now,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use so_store::{FlagRow, LedgerEntry};

    fn entry(ts: DateTime<Utc>, run: &str) -> LedgerEntry {
        LedgerEntry {
            account: "a".into(),
            pool: "default".into(),
            run_id: Some(run.into()),
            ts,
            input_tokens: 10,
            output_tokens: 5,
            cost: 0.1,
        }
    }

    #[test]
    fn windows_and_headroom() {
        let s = Store::open_in_memory().unwrap();
        let now = Utc::now();
        assert_eq!(
            ledger_headroom(&s, "a", "default", now).unwrap().status,
            HeadroomStatus::Unknown
        );
        s.insert_ledger(&entry(now - Duration::hours(6), "r1"))
            .unwrap();
        s.insert_ledger(&entry(now - Duration::hours(1), "r2"))
            .unwrap();
        let w = LedgerWindows::load(&s, "a", "default", now).unwrap();
        assert_eq!(w.five_hour.runs, 1);
        assert_eq!(w.weekly.runs, 2);
        let h = ledger_headroom(&s, "a", "default", now).unwrap();
        assert_eq!(
            (h.status, h.source, h.weekly_remaining_pct),
            (HeadroomStatus::Ok, HeadroomSource::Ledger, None)
        );
        s.set_flag(&FlagRow {
            account: "a".into(),
            pool: "default".into(),
            until: now + Duration::hours(1),
            reason: "q".into(),
        })
        .unwrap();
        assert_eq!(
            ledger_headroom(&s, "a", "default", now).unwrap().status,
            HeadroomStatus::Exhausted
        );
    }
}

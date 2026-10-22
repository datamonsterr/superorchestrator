//! Headroom, ledger and flag queries.

use chrono::{DateTime, Utc};
use rusqlite::{Row, params};
use so_core::Headroom;

use crate::codec::*;
use crate::error::Result;
use crate::rows::{FlagRow, LedgerEntry, LedgerSums};
use crate::store::Store;

const H_COLS: &str = "account, pool, five_hour_remaining_pct, five_hour_resets_at, \
    weekly_remaining_pct, weekly_resets_at, status, source, observed_at";

fn map_headroom(r: &Row<'_>) -> rusqlite::Result<Headroom> {
    Ok(Headroom {
        account: r.get(0)?,
        pool: r.get(1)?,
        five_hour_remaining_pct: r.get(2)?,
        five_hour_resets_at: get_opt_ts(r, 3)?,
        weekly_remaining_pct: r.get(4)?,
        weekly_resets_at: get_opt_ts(r, 5)?,
        status: get_enum(r, 6)?,
        source: get_enum(r, 7)?,
        observed_at: get_ts(r, 8)?,
    })
}

fn map_flag(r: &Row<'_>) -> rusqlite::Result<FlagRow> {
    Ok(FlagRow {
        account: r.get(0)?,
        pool: r.get(1)?,
        until: get_ts(r, 2)?,
        reason: r.get(3)?,
    })
}

impl Store {
    pub fn upsert_headroom(&self, h: &Headroom) -> Result<()> {
        self.conn().execute(
            &format!(
                "INSERT OR REPLACE INTO headroom ({H_COLS}) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)"
            ),
            params![
                h.account,
                h.pool,
                h.five_hour_remaining_pct,
                opt_ts_to_sql(&h.five_hour_resets_at),
                h.weekly_remaining_pct,
                opt_ts_to_sql(&h.weekly_resets_at),
                enum_to_sql(&h.status),
                enum_to_sql(&h.source),
                ts_to_sql(&h.observed_at),
            ],
        )?;
        Ok(())
    }

    pub fn get_headroom(&self, account: &str, pool: &str) -> Result<Option<Headroom>> {
        let conn = self.conn();
        let mut st = conn.prepare(&format!(
            "SELECT {H_COLS} FROM headroom WHERE account = ?1 AND pool = ?2"
        ))?;
        let mut rows = st.query_map([account, pool], map_headroom)?;
        Ok(rows.next().transpose()?)
    }

    pub fn list_headroom(&self, limit: usize) -> Result<Vec<Headroom>> {
        let conn = self.conn();
        let mut st = conn.prepare(&format!(
            "SELECT {H_COLS} FROM headroom ORDER BY account, pool LIMIT ?1"
        ))?;
        let rows = st.query_map([limit as i64], map_headroom)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Inserts a ledger row; returns its rowid.
    pub fn insert_ledger(&self, e: &LedgerEntry) -> Result<i64> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO ledger (account, pool, run_id, ts, input_tokens, output_tokens, cost) \
             VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![
                e.account,
                e.pool,
                e.run_id,
                ts_to_sql(&e.ts),
                e.input_tokens,
                e.output_tokens,
                e.cost
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Usage of (account, pool) at or after `since`.
    pub fn ledger_sums(
        &self,
        account: &str,
        pool: &str,
        since: DateTime<Utc>,
    ) -> Result<LedgerSums> {
        Ok(self.conn().query_row(
            "SELECT COUNT(DISTINCT run_id), COALESCE(SUM(input_tokens),0), \
             COALESCE(SUM(output_tokens),0), COALESCE(SUM(cost),0.0) \
             FROM ledger WHERE account = ?1 AND pool = ?2 AND ts >= ?3",
            params![account, pool, ts_to_sql(&since)],
            |r| {
                Ok(LedgerSums {
                    runs: r.get(0)?,
                    input_tokens: r.get(1)?,
                    output_tokens: r.get(2)?,
                    cost: r.get(3)?,
                })
            },
        )?)
    }

    /// Sets (or replaces) the cooldown flag of (account, pool).
    pub fn set_flag(&self, f: &FlagRow) -> Result<()> {
        self.conn().execute(
            "INSERT OR REPLACE INTO flags (account, pool, until, reason) VALUES (?1,?2,?3,?4)",
            params![f.account, f.pool, ts_to_sql(&f.until), f.reason],
        )?;
        Ok(())
    }

    /// Removes a flag; returns whether one existed.
    pub fn clear_flag(&self, account: &str, pool: &str) -> Result<bool> {
        let n = self.conn().execute(
            "DELETE FROM flags WHERE account = ?1 AND pool = ?2",
            [account, pool],
        )?;
        Ok(n > 0)
    }

    pub fn get_flag(&self, account: &str, pool: &str) -> Result<Option<FlagRow>> {
        let conn = self.conn();
        let mut st = conn.prepare(
            "SELECT account, pool, until, reason FROM flags WHERE account = ?1 AND pool = ?2",
        )?;
        let mut rows = st.query_map([account, pool], map_flag)?;
        Ok(rows.next().transpose()?)
    }

    /// Flags whose `until` is after `now`.
    pub fn active_flags(&self, now: DateTime<Utc>, limit: usize) -> Result<Vec<FlagRow>> {
        let conn = self.conn();
        let mut st = conn.prepare(
            "SELECT account, pool, until, reason FROM flags WHERE until > ?1 \
             ORDER BY account, pool LIMIT ?2",
        )?;
        let rows = st.query_map(params![ts_to_sql(&now), limit as i64], map_flag)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use so_core::{HeadroomSource, HeadroomStatus};

    fn headroom(pct: f64) -> Headroom {
        Headroom {
            account: "a".into(),
            pool: "p".into(),
            five_hour_remaining_pct: Some(pct),
            five_hour_resets_at: Some(crate::codec::now()),
            weekly_remaining_pct: None,
            weekly_resets_at: None,
            status: HeadroomStatus::Ok,
            source: HeadroomSource::Probe,
            observed_at: crate::codec::now(),
        }
    }

    #[test]
    fn headroom_upsert() {
        let s = Store::open_in_memory().unwrap();
        assert!(s.get_headroom("a", "p").unwrap().is_none());
        let h = headroom(80.0);
        s.upsert_headroom(&h).unwrap();
        assert_eq!(s.get_headroom("a", "p").unwrap().unwrap(), h);
        let h2 = headroom(10.0);
        s.upsert_headroom(&h2).unwrap();
        let mut other = headroom(5.0);
        other.account = "b".into();
        s.upsert_headroom(&other).unwrap();
        let all = s.list_headroom(100).unwrap();
        assert_eq!(all, vec![h2, other]);
    }

    #[test]
    fn ledger_sums() {
        let s = Store::open_in_memory().unwrap();
        let t0 = crate::codec::now();
        let mk = |run: &str, ts, i, o, c| LedgerEntry {
            account: "a".into(),
            pool: "p".into(),
            run_id: Some(run.into()),
            ts,
            input_tokens: i,
            output_tokens: o,
            cost: c,
        };
        s.insert_ledger(&mk("r1", t0 - Duration::hours(10), 100, 100, 1.0))
            .unwrap();
        s.insert_ledger(&mk("r1", t0, 10, 20, 0.5)).unwrap();
        s.insert_ledger(&mk("r2", t0, 1, 2, 0.25)).unwrap();
        let id = s.insert_ledger(&mk("r2", t0, 0, 0, 0.0)).unwrap();
        assert!(id > 0);
        let all = s.ledger_sums("a", "p", t0 - Duration::days(1)).unwrap();
        assert_eq!(
            (all.runs, all.input_tokens, all.output_tokens),
            (2, 111, 122)
        );
        assert!((all.cost - 1.75).abs() < 1e-9);
        let recent = s.ledger_sums("a", "p", t0 - Duration::hours(1)).unwrap();
        assert_eq!((recent.runs, recent.input_tokens), (2, 11));
        let none = s.ledger_sums("zz", "p", t0).unwrap();
        assert_eq!(none, LedgerSums::default());
    }

    #[test]
    fn flags() {
        let s = Store::open_in_memory().unwrap();
        let now = crate::codec::now();
        let live = FlagRow {
            account: "a".into(),
            pool: "p".into(),
            until: now + Duration::hours(1),
            reason: "quota".into(),
        };
        let expired = FlagRow {
            account: "b".into(),
            pool: "p".into(),
            until: now - Duration::hours(1),
            reason: "old".into(),
        };
        s.set_flag(&live).unwrap();
        s.set_flag(&expired).unwrap();
        assert_eq!(s.get_flag("a", "p").unwrap().unwrap(), live);
        assert_eq!(s.active_flags(now, 100).unwrap(), vec![live.clone()]);
        assert!(s.clear_flag("a", "p").unwrap());
        assert!(!s.clear_flag("a", "p").unwrap());
        assert!(s.get_flag("a", "p").unwrap().is_none());
        assert!(s.active_flags(now, 100).unwrap().is_empty());
    }
}

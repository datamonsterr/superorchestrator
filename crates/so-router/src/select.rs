//! Account selection (SPEC 8.3 step 5).

use std::cmp::Ordering;

use crate::eligible::AccountState;

/// Headroom assumed for a pool nothing has measured yet.
pub const UNKNOWN_HEADROOM: f64 = 50.0;

/// Remaining percentage of `pool` on `a` for ranking.
pub fn headroom_of(a: &AccountState, pool: &str) -> f64 {
    a.pools
        .get(pool)
        .and_then(|p| p.score)
        .unwrap_or(UNKNOWN_HEADROOM)
}

fn cmp(a: &AccountState, b: &AccountState, pool: &str) -> Ordering {
    headroom_of(b, pool)
        .total_cmp(&headroom_of(a, pool))
        .then(a.active.cmp(&b.active))
        .then(b.file.priority.cmp(&a.file.priority))
        // Never used sorts first, then the oldest attempt.
        .then(a.last_attempt.cmp(&b.last_attempt))
        .then(a.file.name.cmp(&b.file.name))
}

/// Accounts best first: most headroom for the pool, then fewest active runs, then higher
/// `priority`, then least recently used.
pub fn rank_accounts<'a>(accounts: &[&'a AccountState], pool: &str) -> Vec<&'a AccountState> {
    let mut v = accounts.to_vec();
    v.sort_by(|a, b| cmp(a, b, pool));
    v
}

pub fn select_account<'a>(accounts: &[&'a AccountState], pool: &str) -> Option<&'a AccountState> {
    rank_accounts(accounts, pool).into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eligible::tests::account;
    use chrono::{Duration, Utc};
    use so_core::Harness;

    fn acct(name: &str, headroom: Option<f64>, active: u32, prio: i32) -> AccountState {
        let mut a = account(name, Harness::Codex, 9, active);
        a.file.priority = prio;
        a.pools.get_mut("default").unwrap().score = headroom;
        a
    }

    fn pick(v: &[AccountState]) -> Vec<String> {
        let refs: Vec<&AccountState> = v.iter().collect();
        rank_accounts(&refs, "default")
            .iter()
            .map(|a| a.file.name.clone())
            .collect()
    }

    #[test]
    fn headroom_first_then_load_then_priority_then_lru() {
        let mut v = vec![
            acct("lowroom", Some(10.0), 0, 9),
            acct("busy", Some(80.0), 3, 9),
            acct("idle", Some(80.0), 1, 0),
        ];
        assert_eq!(pick(&v), vec!["idle", "busy", "lowroom"]);

        v = vec![acct("aa", Some(50.0), 1, 0), acct("bb", Some(50.0), 1, 5)];
        assert_eq!(pick(&v), vec!["bb", "aa"]);

        let now = Utc::now();
        let mut old = acct("old", Some(50.0), 1, 0);
        old.last_attempt = Some(now - Duration::hours(3));
        let mut recent = acct("recent", Some(50.0), 1, 0);
        recent.last_attempt = Some(now);
        let never = acct("never", Some(50.0), 1, 0);
        assert_eq!(pick(&[recent, old, never]), vec!["never", "old", "recent"]);
    }

    #[test]
    fn unknown_headroom_ranks_between_low_and_high() {
        let v = vec![
            acct("known-low", Some(10.0), 0, 0),
            acct("unknown", None, 0, 0),
            acct("known-high", Some(90.0), 0, 0),
        ];
        assert_eq!(pick(&v), vec!["known-high", "unknown", "known-low"]);
    }
}

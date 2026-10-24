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

/// The account order. [`winner_with_reason`] reads its reason off the same criteria in the
/// same order, so the two must change together.
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

/// The winning account and the first criterion that separated it from the runner-up:
/// `most headroom (80%)`, `fewest active runs (0)`, `higher priority`, `least recently used`
/// or, when every criterion ties, `name order`. `the only usable account` when there is one.
pub fn select_account_with_reason<'a>(
    accounts: &[&'a AccountState],
    pool: &str,
) -> Option<(&'a AccountState, String)> {
    winner_with_reason(&rank_accounts(accounts, pool), pool)
}

/// [`select_account_with_reason`] over accounts already ranked by [`rank_accounts`].
pub fn winner_with_reason<'a>(
    ranked: &[&'a AccountState],
    pool: &str,
) -> Option<(&'a AccountState, String)> {
    let (win, rest) = ranked.split_first()?;
    let Some(next) = rest.first() else {
        return Some((*win, "the only usable account".into()));
    };
    let reason = if headroom_of(win, pool).total_cmp(&headroom_of(next, pool)) != Ordering::Equal {
        format!("most headroom ({:.0}%)", headroom_of(win, pool))
    } else if win.active != next.active {
        format!("fewest active runs ({})", win.active)
    } else if win.file.priority != next.file.priority {
        "higher priority".into()
    } else if win.last_attempt != next.last_attempt {
        "least recently used".into()
    } else {
        "name order".into()
    };
    Some((*win, reason))
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

    fn why(v: &[AccountState]) -> (String, String) {
        let refs: Vec<&AccountState> = v.iter().collect();
        let (a, r) = select_account_with_reason(&refs, "default").unwrap();
        (a.file.name.clone(), r)
    }

    #[test]
    fn the_reason_is_the_first_criterion_that_separated_the_winner() {
        let (w, r) = why(&[acct("solo", Some(30.0), 2, 0)]);
        assert_eq!(
            (w.as_str(), r.as_str()),
            ("solo", "the only usable account")
        );

        let (w, r) = why(&[
            acct("low", Some(20.0), 0, 9),
            acct("high", Some(80.0), 5, 0),
        ]);
        assert_eq!((w.as_str(), r.as_str()), ("high", "most headroom (80%)"));

        let (w, r) = why(&[
            acct("busy", Some(50.0), 3, 9),
            acct("idle", Some(50.0), 0, 0),
        ]);
        assert_eq!((w.as_str(), r.as_str()), ("idle", "fewest active runs (0)"));

        let (w, r) = why(&[acct("aa", Some(50.0), 1, 0), acct("bb", Some(50.0), 1, 5)]);
        assert_eq!((w.as_str(), r.as_str()), ("bb", "higher priority"));

        let now = Utc::now();
        let mut old = acct("old", Some(50.0), 1, 0);
        old.last_attempt = Some(now - Duration::hours(3));
        let mut recent = acct("recent", Some(50.0), 1, 0);
        recent.last_attempt = Some(now);
        let (w, r) = why(&[recent, old]);
        assert_eq!((w.as_str(), r.as_str()), ("old", "least recently used"));

        let (w, r) = why(&[acct("zz", Some(50.0), 1, 0), acct("aa", Some(50.0), 1, 0)]);
        assert_eq!((w.as_str(), r.as_str()), ("aa", "name order"));
    }

    #[test]
    fn unknown_headroom_counts_as_the_assumed_value_in_the_reason() {
        let (w, r) = why(&[acct("known", Some(10.0), 0, 0), acct("unknown", None, 0, 0)]);
        assert_eq!((w.as_str(), r.as_str()), ("unknown", "most headroom (50%)"));
    }

    #[test]
    fn agy_gemini_and_claude_pools_rank_separately() {
        // One account is rich in the gemini pool and poor in the claude pool, the other the
        // reverse: each pool picks its own winner.
        let mut a = account("aa", Harness::Agy, 2, 0);
        a.pools.get_mut("gemini").unwrap().score = Some(90.0);
        a.pools.get_mut("claude").unwrap().score = Some(5.0);
        let mut b = account("bb", Harness::Agy, 2, 0);
        b.pools.get_mut("gemini").unwrap().score = Some(10.0);
        b.pools.get_mut("claude").unwrap().score = Some(70.0);
        let refs = vec![&a, &b];
        let (w, r) = select_account_with_reason(&refs, "gemini").unwrap();
        assert_eq!(
            (w.file.name.as_str(), r.as_str()),
            ("aa", "most headroom (90%)")
        );
        let (w, r) = select_account_with_reason(&refs, "claude").unwrap();
        assert_eq!(
            (w.file.name.as_str(), r.as_str()),
            ("bb", "most headroom (70%)")
        );
    }

    #[test]
    fn no_accounts_gives_no_winner() {
        assert!(select_account_with_reason(&[], "default").is_none());
    }
}

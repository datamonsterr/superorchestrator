//! Eligible choices (SPEC 8.3 step 1) over a snapshot of account load and headroom.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use so_core::{AccountFile, Choice, Harness, HeadroomStatus, Slot};
use so_store::Store;

use crate::catalog::{Catalog, Cost, Tier};
use crate::error::Result;

/// Headroom, availability and recent attempts of one (account, pool).
#[derive(Debug, Clone, PartialEq)]
pub struct PoolState {
    pub available: bool,
    pub status: HeadroomStatus,
    /// Smallest known remaining percentage; `None` when unknown.
    pub score: Option<f64>,
    pub five_hour_pct: Option<f64>,
    pub weekly_pct: Option<f64>,
    /// Runs recorded in the ledger over the trailing 5 h.
    pub attempts_5h: i64,
}

impl PoolState {
    pub fn unknown() -> Self {
        Self {
            available: true,
            status: HeadroomStatus::Unknown,
            score: None,
            five_hour_pct: None,
            weekly_pct: None,
            attempts_5h: 0,
        }
    }
}

/// One account with its load. `active` counts runs and interactive sessions against the cap.
#[derive(Debug, Clone)]
pub struct AccountState {
    pub file: AccountFile,
    pub active: u32,
    pub last_attempt: Option<DateTime<Utc>>,
    pub pools: BTreeMap<String, PoolState>,
}

#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub accounts: Vec<AccountState>,
}

impl Snapshot {
    /// Reads load and headroom for every enabled account. Blocks on SQLite.
    pub fn collect(
        store: &Store,
        accounts: &[AccountFile],
        low_pct: f64,
        now: DateTime<Utc>,
    ) -> Result<Self> {
        let mut out = Vec::new();
        for file in accounts.iter().filter(|a| a.enabled) {
            let runs = store.count_active_runs_by_account(&file.name)?;
            let sessions = store.count_active_interactive_sessions_by_account(&file.name)?;
            let mut pools = BTreeMap::new();
            for pool in &file.pools {
                let av = so_usage::availability(store, &file.name, pool, low_pct, now)?;
                let rec = store
                    .get_headroom(&file.name, pool)?
                    .map(|h| so_usage::effective(&h, now));
                let attempts = store
                    .ledger_sums(&file.name, pool, now - chrono::Duration::hours(5))?
                    .runs;
                pools.insert(
                    pool.clone(),
                    PoolState {
                        available: av.available,
                        status: av.status,
                        score: av.score,
                        five_hour_pct: rec.as_ref().and_then(|h| h.five_hour_remaining_pct),
                        weekly_pct: rec.as_ref().and_then(|h| h.weekly_remaining_pct),
                        attempts_5h: attempts,
                    },
                );
            }
            out.push(AccountState {
                file: file.clone(),
                active: u32::try_from(runs + sessions).unwrap_or(u32::MAX),
                last_attempt: store.last_attempt_started(&file.name)?,
                pools,
            });
        }
        Ok(Self { accounts: out })
    }
}

/// A choice that can run now, with the accounts that could take it.
#[derive(Debug, Clone)]
pub struct Candidate<'a> {
    pub choice: Choice,
    pub pool: String,
    pub tier: Tier,
    pub cost: Cost,
    pub accounts: Vec<&'a AccountState>,
}

/// Whether the account can serve this model under the endpoint routing rule.
pub fn serves(account: &AccountFile, harness: Harness, model: &str, catalog: &Catalog) -> bool {
    if account.harness != harness {
        return false;
    }
    if harness != Harness::Claude {
        return true;
    }
    match &account.endpoint {
        None => catalog.endpoint(harness, model).is_none(),
        Some(endpoint) if !endpoint.models.is_empty() => endpoint.models.iter().any(|m| m == model),
        Some(endpoint) => catalog.endpoint(harness, model) == Some(endpoint.preset),
    }
}

/// The model a pinned account runs when the caller pinned none: an endpoint account's opus
/// slot, or the first of its `models` when a custom list leaves that slot out. `None` when a
/// model is pinned or the account is not an endpoint account.
pub fn pinned_account_model(account: &AccountFile, pinned_model: Option<&str>) -> Option<String> {
    if pinned_model.is_some() {
        return None;
    }
    let endpoint = account.endpoint.as_ref()?;
    let opus = endpoint.slot(Slot::Opus);
    if endpoint.models.is_empty() || endpoint.models.iter().any(|m| m == opus) {
        Some(opus.to_string())
    } else {
        endpoint.models.first().cloned()
    }
}

/// Accounts that can serve the model with an available pool and capacity.
pub fn usable_accounts<'a>(
    snapshot: &'a Snapshot,
    harness: Harness,
    pool: &str,
    model: &str,
    catalog: &Catalog,
) -> Vec<&'a AccountState> {
    snapshot
        .accounts
        .iter()
        .filter(|a| {
            a.file.enabled
                && serves(&a.file, harness, model, catalog)
                && a.active < a.file.cap
                && a.pools.get(pool).is_some_and(|p| p.available)
        })
        .collect()
}

/// Builds a candidate for `choice`, or `None` when no account can take it.
pub fn candidate<'a>(
    choice: &Choice,
    catalog: &Catalog,
    snapshot: &'a Snapshot,
) -> Option<Candidate<'a>> {
    let pool = catalog.pool_for(choice.harness, &choice.model);
    let accounts = usable_accounts(snapshot, choice.harness, &pool, &choice.model, catalog);
    if accounts.is_empty() {
        return None;
    }
    Some(Candidate {
        tier: catalog.tier(choice.harness, &choice.model),
        cost: catalog.cost(choice.harness, &choice.model),
        choice: choice.clone(),
        pool,
        accounts,
    })
}

/// Choices that are not excluded and have at least one usable account. Task text plays no
/// part in this. `choices` are the distinct refs of `Config::choices()`.
pub fn eligible<'a>(
    choices: &[Choice],
    catalog: &Catalog,
    snapshot: &'a Snapshot,
    exclude: &[String],
) -> Vec<Candidate<'a>> {
    choices
        .iter()
        .filter(|c| !exclude.contains(&c.id()))
        .filter_map(|c| candidate(c, catalog, snapshot))
        .collect()
}

/// Claude rank (SPEC 8.4): Claude choices stay in only when no other choice is eligible.
/// An explicit call pins instead and never reaches this.
pub fn apply_claude_rank<'a>(cands: Vec<Candidate<'a>>, catalog: &Catalog) -> Vec<Candidate<'a>> {
    let plain = |c: &Candidate<'_>| {
        c.choice.harness == Harness::Claude
            && catalog
                .endpoint(c.choice.harness, &c.choice.model)
                .is_none()
    };
    if cands.iter().all(plain) {
        return cands;
    }
    cands.into_iter().filter(|c| !plain(c)).collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn choice(text: &str) -> Choice {
        text.parse().unwrap_or_else(|e| panic!("{text}: {e}"))
    }

    pub fn account(name: &str, h: Harness, cap: u32, active: u32) -> AccountState {
        let mut file = AccountFile::new(name, h, "/h", "bin").unwrap();
        file.cap = cap;
        let pools = file
            .pools
            .iter()
            .map(|p| (p.clone(), PoolState::unknown()))
            .collect();
        AccountState {
            file,
            active,
            last_attempt: None,
            pools,
        }
    }

    fn ids(v: &[Candidate<'_>]) -> Vec<String> {
        v.iter().map(|c| c.choice.id()).collect()
    }

    #[test]
    fn filters_exclude_cap_and_exhausted_and_picks_the_agy_pool() {
        let cat = Catalog::default_catalog();
        let choices = vec![
            choice("codex/gpt-6.1-sol/high"),
            choice("codex/gpt-6.1-sol/medium"),
            choice("opencode/opencode/big-pickle"),
            choice("agy/gemini-3.8-flash-low/low"),
            choice("agy/claude-sonnet-4-6/thinking"),
            choice("claude/claude-haiku-5-5/medium"),
        ];
        let mut agy = account("agy1", Harness::Agy, 2, 0);
        agy.pools.get_mut("claude").unwrap().available = false;
        agy.pools.get_mut("claude").unwrap().status = HeadroomStatus::Exhausted;
        let snap = Snapshot {
            accounts: vec![
                account("cx", Harness::Codex, 2, 1),
                account("oc", Harness::Opencode, 1, 1),
                agy,
            ],
        };
        let got = eligible(&choices, &cat, &snap, &["codex/gpt-6.1-sol/medium".into()]);
        assert_eq!(
            ids(&got),
            ["codex/gpt-6.1-sol/high", "agy/gemini-3.8-flash-low/low"]
        );
        assert_eq!(got[1].pool, "gemini");
    }

    #[test]
    fn disabled_account_is_not_usable() {
        let cat = Catalog::default_catalog();
        let mut a = account("cx", Harness::Codex, 2, 0);
        a.file.enabled = false;
        let snap = Snapshot { accounts: vec![a] };
        let c = [choice("codex/gpt-6.1-sol/high")];
        assert!(eligible(&c, &cat, &snap, &[]).is_empty());
    }

    #[test]
    fn claude_ranks_last() {
        let cat = Catalog::default_catalog();
        let choices = vec![
            choice("claude/claude-haiku-5-5/medium"),
            choice("codex/gpt-6.1-sol/high"),
        ];
        let snap = Snapshot {
            accounts: vec![
                account("c1", Harness::Claude, 2, 0),
                account("x1", Harness::Codex, 2, 0),
            ],
        };
        let all = eligible(&choices, &cat, &snap, &[]);
        assert_eq!(
            ids(&all),
            ["claude/claude-haiku-5-5/medium", "codex/gpt-6.1-sol/high"]
        );
        assert_eq!(
            ids(&apply_claude_rank(all, &cat)),
            ["codex/gpt-6.1-sol/high"]
        );
        let only = eligible(&choices, &cat, &snap, &["codex/gpt-6.1-sol/high".into()]);
        assert_eq!(
            ids(&apply_claude_rank(only, &cat)),
            ["claude/claude-haiku-5-5/medium"]
        );
    }

    #[test]
    fn pinned_account_model_uses_opus_slot_or_first_listed_model() {
        let mk = |v: serde_json::Value| {
            let mut a = account("claude-e", Harness::Claude, 1, 0).file;
            a.endpoint = Some(serde_json::from_value(v).unwrap());
            a
        };
        let plain = account("claude-p", Harness::Claude, 1, 0).file;
        assert_eq!(pinned_account_model(&plain, None), None);
        let zai = mk(serde_json::json!({"preset": "zai", "token_env": "T"}));
        assert_eq!(pinned_account_model(&zai, None).as_deref(), Some("glm-5.3"));
        assert_eq!(pinned_account_model(&zai, Some("glm-5.3")), None);
        let custom =
            mk(serde_json::json!({"preset": "zai", "token_env": "T", "models": ["x-1", "x-2"]}));
        assert_eq!(pinned_account_model(&custom, None).as_deref(), Some("x-1"));
        let with_opus = mk(
            serde_json::json!({"preset": "zai", "token_env": "T", "models": ["x-1", "glm-5.3"]}),
        );
        assert_eq!(
            pinned_account_model(&with_opus, None).as_deref(),
            Some("glm-5.3")
        );
    }
}

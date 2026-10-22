//! Account cooldown flags (`account flag`, `POST /accounts/:name/flag`).

use chrono::{DateTime, Utc};
use so_core::AccountFile;
use so_store::{FlagRow, Store};

/// What to do with the flags of the chosen pools.
#[derive(Debug, Clone, Copy)]
pub enum FlagAction {
    Clear,
    Until(DateTime<Utc>),
}

/// The pools a flag request covers: the named pool, or all pools of the account.
pub fn flag_pools(account: &AccountFile, pool: Option<&str>) -> Vec<String> {
    match pool {
        Some(p) => vec![p.to_string()],
        None => account.pools.clone(),
    }
}

/// Sets (reason `manual` unless given) or clears the cooldown of `pools`.
pub fn apply_flag(
    store: &Store,
    account: &str,
    pools: &[String],
    action: FlagAction,
    reason: Option<&str>,
) -> so_store::Result<()> {
    for p in pools {
        match action {
            FlagAction::Clear => {
                store.clear_flag(account, p)?;
            }
            FlagAction::Until(until) => store.set_flag(&FlagRow {
                account: account.to_string(),
                pool: p.clone(),
                until,
                reason: reason.unwrap_or("manual").to_string(),
            })?,
        }
    }
    Ok(())
}

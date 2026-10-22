//! `list_harnesses`: accounts with sign-in state, headroom and load, plus the choices.

use std::sync::Arc;

use chrono::Utc;
use so_accounts::{HostEnv, SignIn, sign_in_state};
use so_core::proto::{AccountView, HarnessesView, METHOD_LIST_HARNESSES};
use so_core::{AccountFile, Headroom, HeadroomSource, HeadroomStatus};

use crate::dispatch::internal;
use crate::server::SupervisorState;

const HEADROOM_LIMIT: usize = 10_000;

pub fn register_harnesses(state: &Arc<SupervisorState>) {
    let state = state.clone();
    state
        .clone()
        .router
        .register(METHOD_LIST_HARNESSES, move |_ctx, _params| {
            let state = state.clone();
            async move {
                let view = build_harnesses_view(&state).map_err(internal)?;
                serde_json::to_value(view).map_err(internal)
            }
        });
}

fn signed_in_text(s: SignIn) -> &'static str {
    match s {
        SignIn::SignedIn => "signed_in",
        SignIn::SignedOut => "signed_out",
        SignIn::Unknown => "unknown",
    }
}

/// Builds the view from the account files on disk (read fresh on each call), the store
/// and the supervisor's config.
pub fn build_harnesses_view(state: &SupervisorState) -> Result<HarnessesView, String> {
    let host = HostEnv::detect().map_err(|e| e.to_string())?;
    let accounts = AccountFile::list(&state.paths).map_err(|e| e.to_string())?;
    let headroom = state
        .store
        .list_headroom(HEADROOM_LIMIT)
        .map_err(|e| e.to_string())?;
    let mut out = Vec::with_capacity(accounts.len());
    for account in accounts {
        let signed_in = sign_in_state(&account, &host).map_err(|e| e.to_string())?;
        let runs = state
            .store
            .count_active_runs_by_account(&account.name)
            .map_err(|e| e.to_string())?;
        let sessions = state
            .store
            .count_active_interactive_sessions_by_account(&account.name)
            .map_err(|e| e.to_string())?;
        let rows = account
            .pools
            .iter()
            .map(|pool| {
                headroom
                    .iter()
                    .find(|h| h.account == account.name && &h.pool == pool)
                    .cloned()
                    .unwrap_or_else(|| unknown_headroom(&account.name, pool))
            })
            .collect();
        out.push(AccountView {
            name: account.name.clone(),
            harness: account.harness,
            signed_in: signed_in_text(signed_in).to_string(),
            enabled: account.enabled,
            cap: account.cap,
            active_runs: u32::try_from(runs + sessions).unwrap_or(u32::MAX),
            headroom: rows,
        });
    }
    Ok(HarnessesView {
        accounts: out,
        choices: state.config.choices.clone(),
    })
}

fn unknown_headroom(account: &str, pool: &str) -> Headroom {
    Headroom {
        account: account.to_string(),
        pool: pool.to_string(),
        five_hour_remaining_pct: None,
        five_hour_resets_at: None,
        weekly_remaining_pct: None,
        weekly_resets_at: None,
        status: HeadroomStatus::Unknown,
        source: HeadroomSource::Ledger,
        observed_at: Utc::now(),
    }
}

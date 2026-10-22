//! `list_harnesses`: accounts with sign-in state, headroom and load, plus the choices.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use so_accounts::{HostEnv, SignIn, sign_in_state};
use so_core::proto::{AccountView, HarnessesView, METHOD_LIST_HARNESSES, SignInState};
use so_core::{AccountFile, Harness, Headroom, HeadroomSource, HeadroomStatus};

use crate::dispatch::internal;
use crate::server::SupervisorState;

const HEADROOM_LIMIT: usize = 10_000;

pub(super) fn register_harnesses(state: &Arc<SupervisorState>) {
    let state = state.clone();
    state
        .clone()
        .router
        .register(METHOD_LIST_HARNESSES, move |_ctx, _params| {
            let state = state.clone();
            async move {
                // Sign-in checks, directory reads and sqlite calls all block.
                let view = tokio::task::spawn_blocking(move || build_harnesses_view(&state))
                    .await
                    .map_err(internal)?
                    .map_err(internal)?;
                serde_json::to_value(view).map_err(internal)
            }
        });
}

fn sign_in_state_of(s: SignIn) -> SignInState {
    match s {
        SignIn::SignedIn => SignInState::SignedIn,
        SignIn::SignedOut => SignInState::SignedOut,
        SignIn::Unknown => SignInState::Unknown,
    }
}

/// Builds the view from the account files on disk (read fresh on each call), the store
/// and the supervisor's config. A broken account yields an entry with `error` set; only
/// failures that affect every account fail the call.
fn build_harnesses_view(state: &SupervisorState) -> Result<HarnessesView, String> {
    let host = state
        .host
        .as_ref()
        .ok_or("the host environment (HOME) is not available")?;
    let headroom = state
        .store
        .list_headroom(HEADROOM_LIMIT)
        .map_err(|e| e.to_string())?;
    let mut index: HashMap<(&str, &str), &Headroom> = HashMap::new();
    for h in &headroom {
        index.insert((h.account.as_str(), h.pool.as_str()), h);
    }

    let dir = state.paths.accounts_dir();
    let mut files: Vec<_> = match std::fs::read_dir(&dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "toml"))
            .collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(format!("could not read {}: {e}", dir.display())),
    };
    files.sort();

    let mut accounts = Vec::with_capacity(files.len());
    for path in files {
        match AccountFile::load(&path) {
            Ok(account) => accounts.push(account_view(state, host, &index, &account)),
            Err(e) => accounts.push(broken_account(&path, &e.to_string())),
        }
    }
    accounts.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(HarnessesView {
        accounts,
        choices: state.config.choices.clone(),
    })
}

fn account_view(
    state: &SupervisorState,
    host: &HostEnv,
    index: &HashMap<(&str, &str), &Headroom>,
    account: &AccountFile,
) -> AccountView {
    let mut errors = Vec::new();
    let signed_in = match sign_in_state(account, host) {
        Ok(s) => sign_in_state_of(s),
        Err(e) => {
            errors.push(format!("sign-in check failed: {e}"));
            SignInState::Unknown
        }
    };
    let mut count = |what: &str, r: so_store::Result<i64>| match r {
        Ok(n) => n,
        Err(e) => {
            errors.push(format!("could not count {what}: {e}"));
            0
        }
    };
    let runs = count(
        "runs",
        state.store.count_active_runs_by_account(&account.name),
    );
    let sessions = count(
        "sessions",
        state
            .store
            .count_active_interactive_sessions_by_account(&account.name),
    );
    let rows = account
        .pools
        .iter()
        .map(|pool| {
            index
                .get(&(account.name.as_str(), pool.as_str()))
                .map_or_else(|| unknown_headroom(&account.name, pool), |h| (*h).clone())
        })
        .collect();
    AccountView {
        name: account.name.clone(),
        harness: account.harness,
        signed_in,
        enabled: account.enabled,
        cap: account.cap,
        active_runs: u32::try_from(runs + sessions).unwrap_or(u32::MAX),
        headroom: rows,
        error: (!errors.is_empty()).then(|| errors.join("; ")),
    }
}

/// Entry for an account file that could not be loaded. The harness is unknown, so the
/// first one stands in; `error` is what tells a reader not to trust the other fields.
fn broken_account(path: &std::path::Path, error: &str) -> AccountView {
    AccountView {
        name: path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
        harness: Harness::Codex,
        signed_in: SignInState::Unknown,
        enabled: false,
        cap: 0,
        active_runs: 0,
        headroom: Vec::new(),
        error: Some(error.to_string()),
    }
}

/// Placeholder for a pool nothing has measured yet: status `unknown`, `observed_at` at
/// the Unix epoch (never observed), source `ledger` (the weakest source; the enum has no
/// "none").
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
        observed_at: DateTime::<Utc>::UNIX_EPOCH,
    }
}

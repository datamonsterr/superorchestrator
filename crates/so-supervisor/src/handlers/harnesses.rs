//! `list_harnesses`: accounts with sign-in state, headroom and load, plus the choices.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{Map, Value, json};
use so_accounts::{HostEnv, SignIn, sign_in_state};
use so_core::proto::{AccountFlag, AccountView, HarnessesView, METHOD_LIST_HARNESSES, SignInState};
use so_core::{AccountFile, Harness, Headroom, HeadroomSource, HeadroomStatus};
use so_router::{Catalog, Snapshot, eligible::candidate};

use crate::dispatch::internal;
use crate::server::SupervisorState;

const HEADROOM_LIMIT: usize = 10_000;

pub(super) fn register_harnesses(state: &Arc<SupervisorState>) {
    let state = state.clone();
    state
        .clone()
        .router
        .register(METHOD_LIST_HARNESSES, move |_ctx, params| {
            let state = state.clone();
            async move {
                let brief = params.get("brief").and_then(Value::as_bool) == Some(true);
                // Sign-in checks, directory reads and sqlite calls all block.
                tokio::task::spawn_blocking(move || {
                    if brief {
                        harnesses_brief(&state)
                    } else {
                        harnesses_view(&state)
                            .and_then(|v| serde_json::to_value(v).map_err(|e| e.to_string()))
                    }
                })
                .await
                .map_err(internal)?
                .map_err(internal)
            }
        });
}

/// The catalog, re-read only when `models.toml` changes on disk.
fn cached_catalog(paths: &so_core::Paths) -> Result<Arc<Catalog>, String> {
    type Slot = Option<(
        std::path::PathBuf,
        Option<std::time::SystemTime>,
        Arc<Catalog>,
    )>;
    static CACHE: std::sync::Mutex<Slot> = std::sync::Mutex::new(None);
    let file = paths.models_toml();
    let mtime = std::fs::metadata(&file).and_then(|m| m.modified()).ok();
    let mut slot = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((path, seen, catalog)) = slot.as_ref()
        && *path == file
        && *seen == mtime
    {
        return Ok(catalog.clone());
    }
    let catalog = Arc::new(Catalog::load(paths).map_err(|e| e.to_string())?);
    *slot = Some((file, mtime, catalog.clone()));
    Ok(catalog)
}

fn seconds(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// "ok 5h80 wk60": status and remaining percentages, one per pool (`pool:` prefix unless
/// it is `default`). `None` when nothing is known about any pool.
fn headroom_short(rows: &[Headroom]) -> Option<String> {
    let one = |h: &Headroom| {
        let mut out = String::new();
        if h.pool != "default" {
            out.push_str(&format!("{}:", h.pool));
        }
        out.push_str(h.status.as_str());
        if let Some(p) = h.five_hour_remaining_pct {
            out.push_str(&format!(" 5h{p:.0}"));
        }
        if let Some(p) = h.weekly_remaining_pct {
            out.push_str(&format!(" wk{p:.0}"));
        }
        out
    };
    let known = |h: &Headroom| {
        h.status != HeadroomStatus::Unknown
            || h.five_hour_remaining_pct.is_some()
            || h.weekly_remaining_pct.is_some()
    };
    if !rows.iter().any(known) {
        return None;
    }
    Some(rows.iter().map(one).collect::<Vec<_>>().join("; "))
}

/// `list_harnesses` for an orchestrator, with `{"brief": true}`: one short row per account
/// and per choice, plus the free models. Omitted keys mean false or empty; `signed_in` and
/// `headroom` are left out when unknown. A choice is `eligible` (the key is left out then) when the router
/// would consider it: it is enabled and an account of its harness can take a run; otherwise
/// `eligible: false` and `why` say what is missing.
pub fn harnesses_brief(state: &SupervisorState) -> Result<Value, String> {
    let view = harnesses_view(state)?;
    let accounts = AccountFile::list(&state.paths).map_err(|e| e.to_string())?;
    let catalog = cached_catalog(&state.paths)?;
    let snapshot = Snapshot::collect(
        &state.store,
        &accounts,
        f64::from(state.config.usage.low_pct),
        Utc::now(),
    )
    .map_err(|e| e.to_string())?;

    let rows: Vec<Value> = view
        .accounts
        .iter()
        .map(|a| {
            let mut row = Map::new();
            row.insert("name".into(), json!(a.name));
            row.insert("harness".into(), json!(a.harness));
            match a.signed_in {
                SignInState::SignedIn => row.insert("signed_in".into(), json!(true)),
                SignInState::SignedOut => row.insert("signed_in".into(), json!(false)),
                SignInState::Unknown => None,
            };
            if !a.enabled {
                row.insert("disabled".into(), json!(true));
            }
            row.insert("load".into(), json!(format!("{}/{}", a.active_runs, a.cap)));
            if let Some(h) = headroom_short(&a.headroom) {
                row.insert("headroom".into(), json!(h));
            }
            let until = a.flags.iter().flatten().map(|f| f.until).max();
            if let Some(until) = until {
                row.insert("cooldown".into(), json!(seconds(until)));
            }
            if let Some(e) = &a.error {
                row.insert("error".into(), json!(e));
            }
            Value::Object(row)
        })
        .collect();

    let choices: Vec<Value> = view
        .choices
        .iter()
        .map(|c| {
            let mut row = Map::new();
            row.insert("id".into(), json!(c.id));
            row.insert("harness".into(), json!(c.harness));
            row.insert("model".into(), json!(c.model));
            if let Some(e) = &c.effort {
                row.insert("effort".into(), json!(e));
            }
            if c.roles != ["worker"] {
                row.insert("roles".into(), json!(c.roles));
            }
            if !c.description.is_empty() {
                row.insert("description".into(), json!(c.description));
            }
            if catalog.is_free(c.harness, &c.model) {
                row.insert("free".into(), json!(true));
            }
            let why = if !c.enabled {
                Some("disabled")
            } else if candidate(c, &catalog, &snapshot).is_some() {
                None
            } else {
                let same: Vec<_> = view
                    .accounts
                    .iter()
                    .filter(|a| a.harness == c.harness)
                    .collect();
                let open: Vec<_> = same
                    .iter()
                    .filter(|a| a.enabled && a.active_runs < a.cap)
                    .collect();
                Some(if same.is_empty() {
                    "no account"
                } else if same.iter().all(|a| !a.enabled) {
                    "account disabled"
                } else if open.is_empty() {
                    "at cap"
                } else if open
                    .iter()
                    .all(|a| a.flags.as_ref().is_some_and(|f| !f.is_empty()))
                {
                    "cooldown"
                } else if open.iter().any(|a| {
                    snapshot
                        .accounts
                        .iter()
                        .find(|s| s.file.name == a.name)
                        .is_some_and(|s| s.pools.values().any(|p| !p.available))
                }) {
                    "no headroom"
                } else {
                    // The router found no usable account for a reason this view cannot name.
                    "not eligible"
                })
            };
            if let Some(why) = why {
                row.insert("eligible".into(), json!(false));
                row.insert("why".into(), json!(why));
            }
            Value::Object(row)
        })
        .collect();

    let mut out = Map::new();
    out.insert("accounts".into(), Value::Array(rows));
    out.insert("choices".into(), Value::Array(choices));
    // Free models of opencode, once (the catalog is per harness, not per account).
    let models = catalog.free_models(Harness::Opencode);
    let has_account = view
        .accounts
        .iter()
        .any(|a| a.harness == Harness::Opencode && a.enabled);
    if has_account && !models.is_empty() {
        out.insert("free_models".into(), json!({ "opencode": models }));
    }
    Ok(Value::Object(out))
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
pub fn harnesses_view(state: &SupervisorState) -> Result<HarnessesView, String> {
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

    let flags = state
        .store
        .active_flags(Utc::now(), HEADROOM_LIMIT)
        .map_err(|e| e.to_string())?;

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
            Ok(account) => accounts.push(account_view(state, host, &index, &flags, &account)),
            Err(e) => accounts.push(broken_account(&path, first_line(&e.to_string()))),
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
    flags: &[so_store::FlagRow],
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
        priority: Some(account.priority),
        pools: Some(account.pools.clone()),
        flags: Some(
            flags
                .iter()
                .filter(|f| f.account == account.name)
                .map(|f| AccountFlag {
                    pool: Some(f.pool.clone()),
                    until: f.until,
                    reason: Some(f.reason.clone()).filter(|r| !r.is_empty()),
                })
                .collect(),
        ),
    }
}

/// First line only: parse errors quote the offending file contents on later lines.
fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or_default()
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
        priority: None,
        pools: None,
        flags: None,
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

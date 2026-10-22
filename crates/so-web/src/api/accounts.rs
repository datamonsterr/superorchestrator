//! Accounts: the same operations as `account add|import|remove|enable|flag|login`.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use chrono::{Duration, Utc};
use serde::Deserialize;
use so_accounts::{HostEnv, manage};
use so_core::proto::{AccountView, AckResult, SignInState};
use so_core::{AccountFile, Config, validate_account_name};
use so_supervisor::harnesses_view;
use so_supervisor::services::accounts::{FlagAction, apply_flag, flag_pools};

use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiJson, ApiQuery, OptionalJson};
use crate::state::AppState;
use crate::stream::publish_account;
use crate::types::{
    AddAccountRequest, FlagRequest, ImportAccountRequest, ImportCandidate, PatchAccountRequest,
    SignInMode, SignInProgress, SignInStart, SignInStatus,
};

fn host(st: &AppState) -> ApiResult<HostEnv> {
    st.sup
        .host
        .clone()
        .ok_or_else(|| ApiError::unavailable("the host environment (HOME) is not available"))
}

/// Rejects names that are not valid account names (path separators, `..`, ...) before they
/// reach a file path.
fn valid_name(name: &str) -> ApiResult<()> {
    validate_account_name(name).map_err(|e| ApiError::invalid(e.to_string()))
}

fn load(st: &AppState, name: &str) -> ApiResult<AccountFile> {
    valid_name(name)?;
    let file = st.sup.paths.account_file(name);
    if !file.exists() {
        return Err(ApiError::not_found(format!("account `{name}` not found")));
    }
    AccountFile::load(&file)
        .map_err(|e| ApiError::internal(format!("account `{name}` could not be loaded: {e}")))
}

fn view_of(st: &AppState, name: &str) -> ApiResult<AccountView> {
    valid_name(name)?;
    harnesses_view(&st.sup)
        .map_err(ApiError::unavailable)?
        .accounts
        .into_iter()
        .find(|a| a.name == name)
        .ok_or_else(|| ApiError::not_found(format!("account `{name}` not found")))
}

/// Fresh view of `name`, also pushed to SSE clients.
fn publish_view(st: &AppState, name: &str) -> ApiResult<AccountView> {
    let v = view_of(st, name)?;
    publish_account(&st.hub, &v);
    Ok(v)
}

pub async fn list(State(st): State<AppState>) -> ApiResult<Json<Vec<AccountView>>> {
    let s = st.clone();
    s.blocking(move || {
        Ok(Json(
            harnesses_view(&st.sup)
                .map_err(ApiError::unavailable)?
                .accounts,
        ))
    })
    .await
}

pub async fn add(
    State(st): State<AppState>,
    ApiJson(b): ApiJson<AddAccountRequest>,
) -> ApiResult<(StatusCode, Json<AccountView>)> {
    let s = st.clone();
    s.blocking(move || {
        valid_name(&b.name)?;
        let cfg = Config::load(&st.sup.paths)?;
        let host = host(&st)?;
        manage::add(
            &st.sup.paths,
            &cfg,
            &host,
            b.harness,
            &b.name,
            b.home.as_deref(),
        )?;
        st.refresh_skill();
        Ok((StatusCode::CREATED, Json(publish_view(&st, &b.name)?)))
    })
    .await
}

pub async fn import(
    State(st): State<AppState>,
    ApiJson(b): ApiJson<ImportAccountRequest>,
) -> ApiResult<(StatusCode, Json<AccountView>)> {
    let s = st.clone();
    s.blocking(move || {
        valid_name(&b.name)?;
        let cfg = Config::load(&st.sup.paths)?;
        let host = host(&st)?;
        manage::import(&st.sup.paths, &cfg, &host, &b.name, b.harness, &b.home)?;
        st.refresh_skill();
        Ok((StatusCode::CREATED, Json(publish_view(&st, &b.name)?)))
    })
    .await
}

pub async fn scan(State(st): State<AppState>) -> ApiResult<Json<Vec<ImportCandidate>>> {
    let s = st.clone();
    s.blocking(move || {
        let host = host(&st)?;
        let found = so_accounts::scan_new(&st.sup.paths, &host)?;
        Ok(Json(
            found
                .into_iter()
                .map(|c| ImportCandidate {
                    home: host.tildify(&c.home),
                    name: c.name,
                    harness: c.harness,
                })
                .collect(),
        ))
    })
    .await
}

pub async fn patch(
    State(st): State<AppState>,
    Path(name): Path<String>,
    ApiJson(b): ApiJson<PatchAccountRequest>,
) -> ApiResult<Json<AccountView>> {
    let s = st.clone();
    s.blocking(move || {
        let mut a = load(&st, &name)?;
        if let Some(cap) = b.cap {
            a.cap = cap;
        }
        if let Some(p) = b.priority {
            a.priority = p;
        }
        if let Some(e) = b.enabled {
            a.enabled = e;
        }
        if let Some(pools) = b.pools {
            if pools.iter().any(|p| p.trim().is_empty()) {
                return Err(ApiError::invalid("pool names must not be empty"));
            }
            a.pools = pools;
        }
        a.save(&st.sup.paths)?;
        st.refresh_skill();
        Ok(Json(publish_view(&st, &name)?))
    })
    .await
}

#[derive(Debug, Deserialize)]
pub struct RemoveQuery {
    #[serde(default)]
    purge: bool,
}

pub async fn remove(
    State(st): State<AppState>,
    Path(name): Path<String>,
    ApiQuery(q): ApiQuery<RemoveQuery>,
) -> ApiResult<Json<AckResult>> {
    let s = st.clone();
    s.blocking(move || {
        valid_name(&name)?;
        let cfg = Config::load(&st.sup.paths)?;
        manage::remove(&st.sup.paths, &cfg, &host(&st)?, &name, q.purge)?;
        st.refresh_skill();
        Ok(Json(AckResult { ok: true }))
    })
    .await
}

pub async fn flag(
    State(st): State<AppState>,
    Path(name): Path<String>,
    ApiJson(b): ApiJson<FlagRequest>,
) -> ApiResult<Json<AccountView>> {
    let s = st.clone();
    s.blocking(move || {
        let account = load(&st, &name)?;
        if let Some(p) = &b.pool
            && !account.pools.contains(p)
        {
            return Err(ApiError::invalid(format!(
                "account `{name}` has no pool `{p}`"
            )));
        }
        let pools = flag_pools(&account, b.pool.as_deref());
        let action = if b.clear {
            FlagAction::Clear
        } else {
            match (b.until, b.hours) {
                (Some(t), _) => FlagAction::Until(t),
                (None, Some(h)) if h.is_finite() && h > 0.0 && h <= 24.0 * 365.0 => {
                    FlagAction::Until(Utc::now() + Duration::seconds((h * 3600.0) as i64))
                }
                _ => {
                    return Err(ApiError::invalid(
                        "give `until`, a positive `hours`, or `clear`",
                    ));
                }
            }
        };
        apply_flag(&st.sup.store, &name, &pools, action, b.reason.as_deref())
            .map_err(ApiError::from)?;
        Ok(Json(publish_view(&st, &name)?))
    })
    .await
}

/// `POST /accounts/:name/login`: the command to run in a terminal. Every harness uses the
/// manual mode today; `device_auth` is reserved for a supervisor-driven codex flow.
pub async fn login_start(
    State(st): State<AppState>,
    Path(name): Path<String>,
    _body: OptionalJson<serde_json::Value>,
) -> ApiResult<Json<SignInStart>> {
    let s = st.clone();
    s.blocking(move || {
        let account = load(&st, &name)?;
        let plan = manage::login_command(&account, &host(&st)?, false)?;
        let env: Vec<String> = plan.env.iter().map(|(k, v)| format!("{k}={v}")).collect();
        let command = if env.is_empty() {
            plan.argv.join(" ")
        } else {
            format!("{} {}", env.join(" "), plan.argv.join(" "))
        };
        Ok(Json(SignInStart::Manual {
            command,
            cwd: None,
            note: plan.note,
        }))
    })
    .await
}

/// `GET /accounts/:name/login`: sign-in is only an auth-file existence check (SPEC 5.4).
pub async fn login_status(
    State(st): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<Json<SignInStatus>> {
    let s = st.clone();
    s.blocking(move || {
        let view = view_of(&st, &name)?;
        let (state, message) = match view.signed_in {
            SignInState::SignedIn => (SignInProgress::Done, None),
            SignInState::SignedOut => (SignInProgress::Pending, None),
            SignInState::Unknown => (
                SignInProgress::Pending,
                Some("the sign-in state cannot be detected for this account".to_string()),
            ),
        };
        Ok(Json(SignInStatus {
            signed_in: view.signed_in,
            mode: SignInMode::Manual,
            state,
            message,
        }))
    })
    .await
}

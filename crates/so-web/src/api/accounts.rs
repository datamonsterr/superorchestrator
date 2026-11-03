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
use so_supervisor::runs::display_argv;
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
        validate_account_name(&b.name).map_err(|e| {
            let message = e.to_string();
            ApiError::invalid(&message).with_data(serde_json::json!({"name": message}))
        })?;
        let cfg = Config::load(&st.sup.paths)?;
        let host = host(&st)?;
        if let Some(ep) = b.endpoint {
            let field = |key: &str, msg: &str| {
                ApiError::invalid(msg).with_data(serde_json::json!({key: msg}))
            };
            if b.harness.is_some_and(|h| h != so_core::Harness::Claude) {
                return Err(field(
                    "harness",
                    "Endpoint accounts require the claude harness",
                ));
            }
            if b.token.is_some() == ep.token_env.is_some()
                || b.token.as_deref().is_some_and(|s| s.trim().is_empty())
            {
                return Err(field(
                    "token",
                    "Provide exactly one non-empty token or token environment variable",
                ));
            }
            let preset = ep
                .preset
                .parse()
                .map_err(|_| field("endpoint.preset", "Unknown endpoint preset"))?;
            if preset == so_core::EndpointPreset::Custom
                && ep.base_url.as_deref().is_none_or(str::is_empty)
            {
                return Err(field(
                    "endpoint.base_url",
                    "Custom endpoints require a base URL",
                ));
            }
            let endpoint = so_core::Endpoint {
                preset,
                base_url: ep.base_url,
                token_env: ep.token_env,
                token_file: None,
                token_var: None,
                models: ep.models,
                slots: so_core::Slots {
                    opus: ep.slots.opus,
                    sonnet: ep.slots.sonnet,
                    haiku: ep.slots.haiku,
                },
            };
            manage::add_endpoint(
                &st.sup.paths,
                &cfg,
                &host,
                &b.name,
                endpoint,
                b.token.as_deref(),
            )
            .map_err(|e| match e {
                so_accounts::AccountsError::Core(so_core::CoreError::InvalidValue { .. }) => {
                    field("endpoint", &e.to_string())
                }
                other => other.into(),
            })?;
        } else {
            if b.token.is_some() {
                return Err(ApiError::invalid("Token requires an endpoint")
                    .with_data(serde_json::json!({"token": "Token requires an endpoint"})));
            }
            let harness = b.harness.ok_or_else(|| {
                ApiError::invalid("harness is required")
                    .with_data(serde_json::json!({"harness": "harness is required"}))
            })?;
            manage::add(
                &st.sup.paths,
                &cfg,
                &host,
                harness,
                &b.name,
                b.home.as_deref(),
            )?;
        }
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
            if e && !a.enabled {
                manage::check_single_agy(&st.sup.paths, &host(&st)?, &a.name, a.harness)?;
            }
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
async fn endpoint_status(account: &AccountFile, host: &HostEnv) -> SignInStatus {
    let endpoint = account.endpoint.as_ref().expect("endpoint checked");
    let result = match so_accounts::resolve_endpoint_token(account, host) {
        Ok(token) => so_harness::endpoint::validate(
            endpoint.base_url(),
            endpoint.token_var(),
            &token,
            endpoint.slot(so_core::Slot::Haiku),
        )
        .await
        .map_err(|e| (e.kind.to_string(), e.detail)),
        Err(e) => Err(("auth".into(), e.to_string())),
    };
    match result {
        Ok(()) => SignInStatus {
            signed_in: SignInState::SignedIn,
            mode: SignInMode::Endpoint,
            state: SignInProgress::Done,
            message: None,
            kind: None,
        },
        Err((kind, message)) => SignInStatus {
            signed_in: SignInState::SignedOut,
            mode: SignInMode::Endpoint,
            state: SignInProgress::Failed,
            message: Some(message),
            kind: Some(kind),
        },
    }
}

pub async fn login_start(
    State(st): State<AppState>,
    Path(name): Path<String>,
    _body: OptionalJson<serde_json::Value>,
) -> ApiResult<Json<SignInStart>> {
    let account = load(&st, &name)?;
    let host = host(&st)?;
    if account.endpoint.is_some() {
        let status = endpoint_status(&account, &host).await;
        return Ok(Json(SignInStart::Endpoint {
            signed_in: status.signed_in,
            state: status.state,
            message: status.message,
            kind: status.kind,
        }));
    }
    let plan = manage::login_command(&account, &host, false)?;
    let mut argv = Vec::new();
    if !plan.env.is_empty() {
        argv.push("env".to_string());
        argv.extend(plan.env.iter().map(|(k, v)| format!("{k}={v}")));
    }
    argv.extend(plan.argv);
    Ok(Json(SignInStart::Manual {
        command: display_argv(&argv),
        cwd: None,
        note: plan.note,
    }))
}

/// `GET /accounts/:name/login`: sign-in is only an auth-file existence check (SPEC 5.4).
pub async fn login_status(
    State(st): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<Json<SignInStatus>> {
    let account = load(&st, &name)?;
    if account.endpoint.is_some() {
        return Ok(Json(endpoint_status(&account, &host(&st)?).await));
    }
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
            kind: None,
        }))
    })
    .await
}

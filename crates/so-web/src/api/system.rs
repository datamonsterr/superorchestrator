//! Status, dashboard, usage, doctor, harnesses, routing log, config, choices and worktrees.

use std::collections::BTreeMap;
use std::path::Path;

use axum::Json;
use axum::extract::State;
use chrono::{Duration, Utc};
use serde::Deserialize;
use serde_json::Value;
use so_core::proto::{AccountView, HarnessesView};
use so_core::{Config, EventType, Platform};
use so_store::EventQuery;
use so_supervisor::doctor::{DoctorReport, run_checks};
use so_supervisor::harnesses_view;
use so_supervisor::services::worktrees as wt;

use super::{redact_event, redact_run};
use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiJson, ApiQuery, OptionalJson};
use crate::page::{clamp_limit, decode_cursor, encode_cursor};
use crate::state::AppState;
use crate::types::{
    Caps, ChoicesView, ConfigView, DailyUsage, DashboardView, Page, PruneRequest, PruneResult,
    RouteRecord, RunViewPublic, SecretOrigin, SecretSource, StatusView, WorktreeView,
};

fn caps(st: &AppState) -> Caps {
    Caps {
        global: st.sup.config.limits.global_cap,
    }
}

async fn status_view(st: &AppState) -> ApiResult<StatusView> {
    Ok(StatusView::new(
        so_supervisor::status_result(&st.sup).await?,
        caps(st),
    ))
}

pub async fn status(State(st): State<AppState>) -> ApiResult<Json<StatusView>> {
    Ok(Json(status_view(&st).await?))
}

async fn accounts_of(st: &AppState) -> ApiResult<Vec<AccountView>> {
    let s = st.clone();
    st.blocking(move || {
        Ok(harnesses_view(&s.sup)
            .map_err(ApiError::unavailable)?
            .accounts)
    })
    .await
}

async fn daily(st: &AppState, days: i64) -> ApiResult<Vec<DailyUsage>> {
    let since = Utc::now() - Duration::days(days);
    let rows = st.db(move |s| s.ledger_daily(since)).await?;
    let mut out: Vec<DailyUsage> = Vec::new();
    for r in rows {
        if out.last().is_none_or(|d| d.day != r.day) {
            out.push(DailyUsage {
                day: r.day.clone(),
                input_tokens: 0,
                output_tokens: 0,
                cost: 0.0,
                by_account: BTreeMap::new(),
            });
        }
        if let Some(d) = out.last_mut() {
            d.input_tokens += u64::try_from(r.input_tokens).unwrap_or(0);
            d.output_tokens += u64::try_from(r.output_tokens).unwrap_or(0);
            d.cost += r.cost;
            *d.by_account.entry(r.account).or_default() += r.cost;
        }
    }
    Ok(out)
}

#[derive(Debug, Deserialize)]
pub struct DaysQuery {
    days: Option<i64>,
}

pub async fn usage_daily(
    State(st): State<AppState>,
    ApiQuery(q): ApiQuery<DaysQuery>,
) -> ApiResult<Json<Vec<DailyUsage>>> {
    Ok(Json(daily(&st, q.days.unwrap_or(30).clamp(1, 366)).await?))
}

pub async fn dashboard(State(st): State<AppState>) -> ApiResult<Json<DashboardView>> {
    let status = status_view(&st).await?;
    let accounts = accounts_of(&st).await?;
    let rows = st.db(|s| s.list_active_runs(1000)).await?;
    let mut live_runs = Vec::with_capacity(rows.len());
    for row in rows {
        let mut v = RunViewPublic::from(st.sup.runs.run_view(row).await?);
        redact_run(&mut v);
        live_runs.push(v);
    }
    let q = EventQuery {
        event_type: Some(EventType::Escalated),
        newest_first: true,
        limit: 20,
        ..Default::default()
    };
    let mut escalations = st.db(move |s| s.query_events(&q)).await?;
    escalations.iter_mut().for_each(redact_event);
    Ok(Json(DashboardView {
        status,
        accounts,
        live_runs,
        escalations,
        daily: daily(&st, 30).await?,
    }))
}

pub async fn doctor(State(st): State<AppState>) -> ApiResult<Json<DoctorReport>> {
    let s = st.clone();
    st.blocking(move || {
        let host =
            s.sup.host.clone().ok_or_else(|| {
                ApiError::unavailable("the host environment (HOME) is not available")
            })?;
        Ok(Json(run_checks(&s.sup.paths, &host)))
    })
    .await
}

pub async fn harnesses(State(st): State<AppState>) -> ApiResult<Json<HarnessesView>> {
    let s = st.clone();
    st.blocking(move || Ok(Json(harnesses_view(&s.sup).map_err(ApiError::unavailable)?)))
        .await
}

#[derive(Debug, Deserialize)]
pub struct CursorQuery {
    limit: Option<usize>,
    cursor: Option<String>,
}

/// The Route log, newest first, from `route_decision` events.
pub async fn routing_records(
    State(st): State<AppState>,
    ApiQuery(q): ApiQuery<CursorQuery>,
) -> ApiResult<Json<Page<RouteRecord>>> {
    let limit = clamp_limit(q.limit);
    let query = EventQuery {
        event_type: Some(EventType::RouteDecision),
        newest_first: true,
        after: q.cursor.as_deref().map(decode_cursor).transpose()?,
        limit: limit + 1,
        ..Default::default()
    };
    let mut events = st.db(move |s| s.query_events(&query)).await?;
    let next_cursor = (events.len() > limit).then(|| {
        events.truncate(limit);
        events.last().map(|e| encode_cursor(&e.ts, &e.id))
    });
    let items = events
        .into_iter()
        .filter_map(|e| {
            let mut result = super::runs::route_result_of(&e.data)?;
            result.rationale = so_core::redact::redact(&result.rationale);
            let excerpt = e.data["prompt_excerpt"].as_str().unwrap_or_default();
            Some(RouteRecord {
                ts: e.ts,
                run_id: e.run_id,
                prompt_excerpt: so_core::redact::redact(excerpt),
                result,
            })
        })
        .collect();
    Ok(Json(Page {
        items,
        next_cursor: next_cursor.flatten(),
    }))
}

// ------------------------------------------------------------------ config

fn restart_required(st: &AppState, disk: &Config) -> bool {
    *disk != st.sup.config
}

fn secret_sources(cfg: &Config) -> (SecretSource, SecretSource) {
    let ui = SecretSource {
        source: if cfg.ui.token.is_some() {
            SecretOrigin::File
        } else {
            SecretOrigin::Unset
        },
        name: None,
    };
    let jev = &cfg.router.jev;
    let key = if std::env::var_os(&jev.key_env).is_some_and(|v| !v.is_empty()) {
        SecretSource {
            source: SecretOrigin::Env,
            name: Some(jev.key_env.clone()),
        }
    } else if let Some(f) = &jev.key_file {
        SecretSource {
            source: SecretOrigin::File,
            name: Some(f.clone()),
        }
    } else {
        SecretSource {
            source: SecretOrigin::Unset,
            name: Some(jev.key_env.clone()),
        }
    };
    (ui, key)
}

fn config_view(st: &AppState) -> ApiResult<ConfigView> {
    let disk = Config::load(&st.sup.paths)?;
    let (ui_token, openrouter_key) = secret_sources(&disk);
    let restart_required = restart_required(st, &disk);
    let mut config = disk;
    config.ui.token = None;
    Ok(ConfigView {
        config,
        ui_token,
        openrouter_key,
        platform: match Platform::current() {
            Platform::MacOs => "macos",
            Platform::Linux => "linux",
        }
        .to_string(),
        restart_required,
    })
}

pub async fn config_get(State(st): State<AppState>) -> ApiResult<Json<ConfigView>> {
    let s = st.clone();
    st.blocking(move || Ok(Json(config_view(&s)?))).await
}

fn flatten(prefix: &str, v: &Value, out: &mut Vec<(String, Value)>) {
    match v {
        Value::Object(o) => {
            for (k, v) in o {
                let key = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                flatten(&key, v, out);
            }
        }
        other => out.push((prefix.to_string(), other.clone())),
    }
}

/// A JSON leaf as the TOML text `Config::set` reads.
fn toml_text(key: &str, v: &Value) -> ApiResult<String> {
    let t = toml::Value::try_from(v).map_err(|e| ApiError::invalid(format!("`{key}`: {e}")))?;
    Ok(t.to_string())
}

/// `PATCH /config`: a partial config object; every leaf is a dotted key like `config set`.
pub async fn config_patch(
    State(st): State<AppState>,
    ApiJson(body): ApiJson<Value>,
) -> ApiResult<Json<ConfigView>> {
    if !body.is_object() {
        return Err(ApiError::invalid("expected a JSON object"));
    }
    let s = st.clone();
    st.blocking(move || {
        let mut leaves = Vec::new();
        flatten("", &body, &mut leaves);
        let mut trial = Config::load(&s.sup.paths)?;
        let mut sets = Vec::new();
        for (key, v) in leaves {
            if key == "ui.token" {
                return Err(ApiError::invalid("ui.token cannot be set over the API"));
            }
            let text = toml_text(&key, &v)?;
            trial.set(&key, &text)?;
            sets.push((key, text));
        }
        let entries: Vec<(&str, &str)> =
            sets.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        Config::set_many_in_file(&s.sup.paths, &entries)?;
        s.refresh_skill();
        Ok(Json(config_view(&s)?))
    })
    .await
}

fn choices_view(st: &AppState) -> ApiResult<ChoicesView> {
    let disk = Config::load(&st.sup.paths)?;
    Ok(ChoicesView {
        restart_required: restart_required(st, &disk),
        choices: disk.choices(),
        routing_instruction: disk.router.instruction,
    })
}

pub async fn choices_get(State(st): State<AppState>) -> ApiResult<Json<ChoicesView>> {
    let s = st.clone();
    st.blocking(move || Ok(Json(choices_view(&s)?))).await
}

#[derive(Debug, Deserialize)]
pub struct ChoicesBody {
    #[allow(dead_code)]
    choices: Vec<so_core::Choice>,
    routing_instruction: String,
}

pub async fn choices_put(
    State(st): State<AppState>,
    ApiJson(b): ApiJson<ChoicesBody>,
) -> ApiResult<Json<ChoicesView>> {
    // T4 stub: choices are no longer stored; only the routing instruction is written.
    let s = st.clone();
    st.blocking(move || {
        let instruction = toml::Value::String(b.routing_instruction).to_string();
        Config::set_in_file(&s.sup.paths, "router.instruction", &instruction)?;
        s.refresh_skill();
        Ok(Json(choices_view(&s)?))
    })
    .await
}

// --------------------------------------------------------------- worktrees

/// Size of a directory tree, without following symlinks; bounded.
fn dir_size(path: &Path) -> u64 {
    let (mut total, mut stack, mut seen) = (0u64, vec![path.to_path_buf()], 0usize);
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            seen += 1;
            if seen > 200_000 {
                return total;
            }
            let Ok(md) = e.metadata() else { continue };
            if md.is_dir() {
                stack.push(e.path());
            } else if md.is_file() {
                total += md.len();
            }
        }
    }
    total
}

pub async fn worktrees_list(State(st): State<AppState>) -> ApiResult<Json<Vec<WorktreeView>>> {
    st.db(|s| {
        let rows = wt::runs_with_worktree(s)?;
        Ok(rows
            .iter()
            .filter(|r| wt::exists(r))
            .map(|r| {
                let path = r.worktree_path.clone().unwrap_or_default();
                WorktreeView {
                    size_bytes: dir_size(Path::new(&path)),
                    path,
                    branch: r.branch.clone(),
                    run_id: r.id.clone(),
                    orphaned: r.state.is_terminal(),
                }
            })
            .collect())
    })
    .await
    .map(Json)
}

pub async fn worktrees_prune(
    State(st): State<AppState>,
    OptionalJson(b): OptionalJson<PruneRequest>,
) -> ApiResult<Json<PruneResult>> {
    let store = st.sup.store.clone();
    st.blocking(move || {
        let mut targets = wt::prune_targets(wt::runs_with_worktree(&store)?);
        let mut skipped = Vec::new();
        if let Some(ids) = &b.ids {
            // Only what the caller confirmed, and only if it is still prunable now.
            targets.retain(|(run, _)| ids.contains(&run.id));
            skipped = ids
                .iter()
                .filter(|id| !targets.iter().any(|(r, _)| &r.id == *id))
                .cloned()
                .collect();
        }
        let mut removed = Vec::new();
        for (run, c) in &targets {
            if b.dry_run {
                removed.push(run.id.clone());
            } else {
                match wt::prune_one(&store, run, c) {
                    Ok(()) => removed.push(run.id.clone()),
                    Err(e) => {
                        tracing::warn!(path = %c.path.display(), "could not prune: {e}");
                        if b.ids.is_some() {
                            skipped.push(run.id.clone());
                        }
                    }
                }
            }
        }
        Ok(Json(PruneResult { removed, skipped }))
    })
    .await
}

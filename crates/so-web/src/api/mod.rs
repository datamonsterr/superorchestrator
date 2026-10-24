//! The `/api/v1` routes.

pub mod accounts;
pub mod agents;
pub mod models;
pub mod runs;
pub mod sessions;
pub mod system;
pub mod workflows;

use axum::Router;
use axum::routing::{get, patch, post};
use so_core::Event;
use so_core::redact::redact_json;

use crate::security::post_token;
use crate::state::AppState;
use crate::stream::get_stream;
use crate::types::RunViewPublic;

pub fn redact_event(e: &mut Event) {
    redact_json(&mut e.data);
}

pub fn redact_run(v: &mut RunViewPublic) {
    v.recent_events.iter_mut().for_each(redact_event);
    for m in &mut v.messages {
        m.body = so_core::redact::redact(&m.body);
    }
    if let Some(r) = v.result.as_mut() {
        redact_json(r);
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/token", post(post_token))
        .route("/api/v1/auth", post(post_token))
        .route("/api/v1/status", get(system::status))
        .route("/api/v1/dashboard", get(system::dashboard))
        .route("/api/v1/usage/daily", get(system::usage_daily))
        .route("/api/v1/doctor", get(system::doctor))
        .route("/api/v1/harnesses", get(system::harnesses))
        .route("/api/v1/sessions", get(sessions::list))
        .route("/api/v1/sessions/{id}", get(sessions::detail))
        .route("/api/v1/sessions/{id}/events", get(sessions::events))
        .route("/api/v1/runs/{id}", get(runs::get))
        .route("/api/v1/runs/{id}/events", get(runs::events))
        .route("/api/v1/runs/{id}/transcript", get(runs::transcript))
        .route("/api/v1/runs/{id}/route", get(runs::route))
        .route("/api/v1/runs/{id}/cancel", post(runs::cancel))
        .route("/api/v1/runs/{id}/join", post(runs::join))
        .route(
            "/api/v1/runs/{id}/messages",
            get(runs::messages).post(runs::send_message),
        )
        .route("/api/v1/runs/{id}/accept", post(runs::accept))
        .route("/api/v1/runs/{id}/reject", post(runs::reject))
        .route("/api/v1/runs/{id}/fork", post(runs::fork))
        .route("/api/v1/route", post(models::route))
        .route("/api/v1/models", get(models::list))
        .route("/api/v1/models/refresh", post(models::refresh))
        .route("/api/v1/agents", get(agents::list).post(agents::create))
        .route(
            "/api/v1/agents/{name}",
            get(agents::get).put(agents::update).delete(agents::remove),
        )
        .route("/api/v1/workflows", get(workflows::list))
        .route("/api/v1/workflows/runs", get(workflows::runs))
        .route("/api/v1/workflows/runs/{id}", get(workflows::run_detail))
        .route(
            "/api/v1/workflows/runs/{id}/cancel",
            post(workflows::cancel),
        )
        .route("/api/v1/workflows/{name}/run", post(workflows::run))
        .route("/api/v1/routing/records", get(system::routing_records))
        .route("/api/v1/accounts", get(accounts::list).post(accounts::add))
        .route("/api/v1/accounts/import", post(accounts::import))
        .route("/api/v1/accounts/scan", get(accounts::scan))
        .route(
            "/api/v1/accounts/{name}",
            patch(accounts::patch).delete(accounts::remove),
        )
        .route("/api/v1/accounts/{name}/flag", post(accounts::flag))
        .route(
            "/api/v1/accounts/{name}/login",
            post(accounts::login_start).get(accounts::login_status),
        )
        .route(
            "/api/v1/choices",
            get(system::choices_get).put(system::choices_put),
        )
        .route(
            "/api/v1/levels",
            get(system::levels_get)
                .put(system::levels_put)
                .delete(system::levels_delete),
        )
        .route(
            "/api/v1/config",
            get(system::config_get).patch(system::config_patch),
        )
        .route("/api/v1/worktrees", get(system::worktrees_list))
        .route("/api/v1/worktrees/prune", post(system::worktrees_prune))
        .route("/api/v1/stream", get(get_stream))
}

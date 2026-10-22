//! Runs: detail, events, transcript, route decision, cancel and join.

use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};

use axum::Json;
use axum::extract::{Path, State};
use serde::Deserialize;
use serde_json::Value;
use so_core::proto::{
    AckResult, ForkParams, INVALID_REQUEST, MessageView, RejectParams, RouteResult, RpcError,
    RunRef, SendMessageParams, SpawnResult, UNAVAILABLE,
};
use so_core::redact::redact;
use so_core::{Event, EventType};
use so_harness::TranscriptWriter;
use so_store::EventQuery;
use so_supervisor::ConnCtx;

use super::redact_run;
use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiJson, ApiQuery, OptionalJson};
use crate::page::clamp_limit;
use crate::state::AppState;
use crate::types::{
    ForkBody, JoinView, Page, RejectBody, RunViewPublic, SendMessageBody, TranscriptLine,
    TranscriptPage, TranscriptRole, TranscriptTool,
};

async fn public_view(st: &AppState, id: &str) -> ApiResult<RunViewPublic> {
    let row = st.sup.runs.resolve_run(id).await?;
    let mut v = RunViewPublic::from(st.sup.runs.run_view(row).await?);
    redact_run(&mut v);
    Ok(v)
}

pub async fn get(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<RunViewPublic>> {
    Ok(Json(public_view(&st, &id).await?))
}

pub async fn events(
    State(st): State<AppState>,
    Path(id): Path<String>,
    ApiQuery(params): ApiQuery<super::sessions::EventsQuery>,
) -> ApiResult<Json<Page<Event>>> {
    let row = st.sup.runs.resolve_run(&id).await?;
    let q = EventQuery {
        run_id: Some(row.id),
        ..Default::default()
    };
    Ok(Json(super::sessions::events_page(&st, q, &params).await?))
}

pub async fn cancel(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<AckResult>> {
    let row = st.sup.runs.resolve_run(&id).await?;
    st.sup.runs.cancel(&row.id).await?;
    Ok(Json(AckResult { ok: true }))
}

/// `POST /runs/:id/join`: records `join_command_issued`; secrets never leave this function.
pub async fn join(State(st): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<JoinView>> {
    let row = st.sup.runs.resolve_run(&id).await?;
    let j = st.sup.runs.join_run(row).await?;
    Ok(Json(JoinView {
        argv: so_supervisor::runs::mask_argv(&j.argv, &j.env),
        cwd: j.cwd,
        env_names: j.env.iter().map(|(k, _)| k.clone()).collect(),
    }))
}

/// A `RouteResult` from a `route_decision` event (the result itself, or under `result`).
pub fn route_result_of(data: &Value) -> Option<RouteResult> {
    serde_json::from_value(data.clone())
        .ok()
        .or_else(|| serde_json::from_value(data.get("result")?.clone()).ok())
}

pub async fn route(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<RouteResult>> {
    let row = st.sup.runs.resolve_run(&id).await?;
    let q = EventQuery {
        run_id: Some(row.id),
        event_type: Some(EventType::RouteDecision),
        newest_first: true,
        limit: 1,
        ..Default::default()
    };
    let ev = st.db(move |s| s.query_events(&q)).await?.pop();
    let mut result = ev
        .and_then(|e| route_result_of(&e.data))
        .ok_or_else(|| ApiError::not_found("no route decision recorded for this run"))?;
    result.rationale = redact(&result.rationale);
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct TranscriptQuery {
    offset: Option<u64>,
    limit: Option<usize>,
}

/// Longest text of one transcript line.
const MAX_TEXT: usize = 64 * 1024;
/// Longest transcript entry read; a longer one is skipped.
const MAX_LINE: u64 = 1024 * 1024;
/// Most bytes one transcript request reads.
const MAX_BYTES: u64 = 4 * 1024 * 1024;

fn clip(text: &str) -> String {
    let mut t = redact(text);
    if t.len() > MAX_TEXT {
        let mut cut = MAX_TEXT;
        while !t.is_char_boundary(cut) {
            cut -= 1;
        }
        t.truncate(cut);
        t.push('…');
    }
    t
}

/// Normalised lines of one transcript entry. `raw` is never read.
fn lines_of(entry: &Value, offset: u64) -> Vec<TranscriptLine> {
    let ts = entry["ts"].as_str().map(str::to_string);
    let events: Vec<&Value> = match &entry["event"] {
        Value::Array(a) => a.iter().collect(),
        Value::Null => Vec::new(),
        one => vec![one],
    };
    let line = |role, text: String, tool| TranscriptLine {
        offset,
        role,
        text,
        tool,
        ts: ts.clone(),
    };
    events
        .into_iter()
        .filter_map(|e| {
            let s = |k: &str| e[k].as_str().unwrap_or_default();
            Some(match e["type"].as_str()? {
                "assistant_message" => line(TranscriptRole::Assistant, clip(s("text")), None),
                "tool_call_started" => line(
                    TranscriptRole::Tool,
                    clip(s("name")),
                    Some(TranscriptTool {
                        name: s("name").to_string(),
                        ok: None,
                    }),
                ),
                "tool_call_finished" => line(
                    TranscriptRole::Tool,
                    clip(s("name")),
                    Some(TranscriptTool {
                        name: s("name").to_string(),
                        ok: e["ok"].as_bool(),
                    }),
                ),
                "turn_finished" => {
                    let text = if e["is_error"].as_bool() == Some(true) {
                        s("error_text")
                    } else {
                        s("result_text")
                    };
                    if text.is_empty() {
                        return None;
                    }
                    line(TranscriptRole::Assistant, clip(text), None)
                }
                "error" => line(TranscriptRole::System, clip(s("message")), None),
                "exited" => line(
                    TranscriptRole::System,
                    format!("harness exited (code {})", e["code"]),
                    None,
                ),
                _ => return None,
            })
        })
        .collect()
}

pub async fn transcript(
    State(st): State<AppState>,
    Path(id): Path<String>,
    ApiQuery(q): ApiQuery<TranscriptQuery>,
) -> ApiResult<Json<TranscriptPage>> {
    let row = st.sup.runs.resolve_run(&id).await?;
    let path = TranscriptWriter::path(&st.sup.paths.run_dir(&row.id));
    let start = q.offset.unwrap_or(0);
    let limit = clamp_limit(q.limit);
    st.blocking(move || {
        let Ok(file) = std::fs::File::open(&path) else {
            return Ok(Json(TranscriptPage {
                lines: Vec::new(),
                next_offset: Some(start),
                total_bytes: 0,
            }));
        };
        let total = file.metadata().map_err(ApiError::internal)?.len();
        let mut rd = BufReader::new(file);
        rd.seek(SeekFrom::Start(start))
            .map_err(ApiError::internal)?;
        let (mut pos, mut lines, mut buf) = (start, Vec::new(), Vec::new());
        while lines.len() < limit && pos - start < MAX_BYTES {
            buf.clear();
            let n = (&mut rd)
                .take(MAX_LINE + 1)
                .read_until(b'\n', &mut buf)
                .map_err(ApiError::internal)? as u64;
            if n == 0 {
                break;
            }
            if buf.ends_with(b"\n") {
                if n <= MAX_LINE
                    && let Ok(entry) = serde_json::from_slice::<Value>(&buf)
                {
                    lines.extend(lines_of(&entry, pos));
                }
                pos += n;
            } else if n > MAX_LINE {
                // Too long to hold: skip to the end of the entry.
                let mut skipped = n;
                loop {
                    buf.clear();
                    let m = (&mut rd)
                        .take(MAX_LINE)
                        .read_until(b'\n', &mut buf)
                        .map_err(ApiError::internal)? as u64;
                    skipped += m;
                    if m == 0 || buf.ends_with(b"\n") {
                        break;
                    }
                }
                if buf.ends_with(b"\n") {
                    pos += skipped;
                } else {
                    break;
                }
            } else {
                // A line without its newline is still being written.
                break;
            }
        }
        Ok(Json(TranscriptPage {
            lines,
            next_offset: Some(pos),
            total_bytes: total,
        }))
    })
    .await
}

/// Actions on a run happen in the run's session, as the person at the UI.
fn acting(row: &so_store::RunRow) -> ConnCtx {
    ConnCtx::acting_in(&row.session_id)
}

/// A run in the wrong state is a 409; a harness that cannot fork is a 422 naming it.
fn action_error(e: RpcError) -> ApiError {
    if e.code == INVALID_REQUEST {
        ApiError::state_conflict(e.message)
    } else if e.code == UNAVAILABLE && e.message.contains("cannot fork") {
        ApiError::invalid(e.message)
    } else {
        e.into()
    }
}

fn redact_message(mut m: MessageView) -> MessageView {
    m.body = redact(&m.body);
    m
}

pub async fn messages(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Vec<MessageView>>> {
    let row = st.sup.runs.resolve_run(&id).await?;
    let list = st.sup.runs.list_messages(&row.id).await?;
    Ok(Json(list.into_iter().map(redact_message).collect()))
}

pub async fn send_message(
    State(st): State<AppState>,
    Path(id): Path<String>,
    ApiJson(b): ApiJson<SendMessageBody>,
) -> ApiResult<Json<AckResult>> {
    let row = st.sup.runs.resolve_run(&id).await?;
    let p = SendMessageParams {
        run: row.id.clone(),
        text: b.text,
        kind: b.kind,
    };
    let ack = st
        .sup
        .runs
        .send_message(&acting(&row), p)
        .await
        .map_err(action_error)?;
    Ok(Json(ack))
}

pub async fn accept(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<AckResult>> {
    let row = st.sup.runs.resolve_run(&id).await?;
    let r = RunRef {
        run: row.id.clone(),
    };
    Ok(Json(
        st.sup
            .runs
            .accept_run(&acting(&row), r)
            .await
            .map_err(action_error)?,
    ))
}

pub async fn reject(
    State(st): State<AppState>,
    Path(id): Path<String>,
    ApiJson(b): ApiJson<RejectBody>,
) -> ApiResult<Json<AckResult>> {
    let row = st.sup.runs.resolve_run(&id).await?;
    let p = RejectParams {
        run: row.id.clone(),
        feedback: b.feedback,
    };
    Ok(Json(
        st.sup
            .runs
            .reject_run(&acting(&row), p)
            .await
            .map_err(action_error)?,
    ))
}

pub async fn fork(
    State(st): State<AppState>,
    Path(id): Path<String>,
    OptionalJson(b): OptionalJson<ForkBody>,
) -> ApiResult<(axum::http::StatusCode, Json<SpawnResult>)> {
    let row = st.sup.runs.resolve_run(&id).await?;
    let p = ForkParams {
        run: row.id.clone(),
        prompt: b
            .prompt
            .unwrap_or_else(|| "Continue from here.".to_string()),
    };
    let out = st
        .sup
        .runs
        .fork_run(&acting(&row), p)
        .await
        .map_err(action_error)?;
    Ok((axum::http::StatusCode::ACCEPTED, Json(out)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn transcript_lines_are_normalised_and_redacted_and_raw_is_dropped() {
        let entry = json!({
            "ts": "2026-01-01T00:00:00Z", "dir": "in", "channel": "stdout",
            "raw": "{\"secret\":\"sk-rawrawrawraw1234\"}",
            "event": [
                {"type": "assistant_message", "text": "key sk-abcdef1234567890 ok"},
                {"type": "tool_call_finished", "name": "bash", "ok": true},
                {"type": "usage", "input_tokens": 1, "output_tokens": 1},
            ],
        });
        let lines = lines_of(&entry, 7);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].text, "key [REDACTED] ok");
        assert_eq!(lines[1].tool.as_ref().unwrap().ok, Some(true));
        assert!(lines.iter().all(|l| l.offset == 7));
        assert!(!serde_json::to_string(&lines).unwrap().contains("rawraw"));
    }
}

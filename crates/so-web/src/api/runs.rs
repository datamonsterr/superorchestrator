//! Runs: detail, events, transcript, route decision, cancel and join.

use std::io::{BufRead, BufReader, Seek, SeekFrom};

use axum::Json;
use axum::extract::{Path, State};
use serde::Deserialize;
use serde_json::Value;
use so_core::proto::{AckResult, RouteResult};
use so_core::redact::redact;
use so_core::{Event, EventType};
use so_harness::TranscriptWriter;
use so_store::EventQuery;

use super::redact_run;
use crate::error::{ApiError, ApiResult};
use crate::extract::ApiQuery;
use crate::page::clamp_limit;
use crate::state::AppState;
use crate::types::{
    JoinView, Page, RunViewPublic, TranscriptLine, TranscriptPage, TranscriptRole, TranscriptTool,
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
        argv: mask_argv(&j.argv, &j.env),
        cwd: j.cwd,
        env_names: j.env.iter().map(|(k, _)| k.clone()).collect(),
    }))
}

/// argv with the values of the command's environment and anything that looks like a key
/// replaced by `***`.
pub fn mask_argv(argv: &[String], env: &[(String, String)]) -> Vec<String> {
    argv.iter()
        .map(|a| {
            let mut out = a.clone();
            for (_, v) in env.iter().filter(|(_, v)| v.len() >= 4) {
                out = out.replace(v.as_str(), "***");
            }
            redact(&out).replace("[REDACTED]", "***")
        })
        .collect()
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
        let (mut pos, mut lines, mut buf) = (start, Vec::new(), String::new());
        while lines.len() < limit && pos - start < MAX_BYTES {
            buf.clear();
            let n = rd.read_line(&mut buf).map_err(ApiError::internal)?;
            // A line without its newline is still being written.
            if n == 0 || !buf.ends_with('\n') {
                break;
            }
            if let Ok(entry) = serde_json::from_str::<Value>(&buf) {
                lines.extend(lines_of(&entry, pos));
            }
            pos += n as u64;
        }
        Ok(Json(TranscriptPage {
            lines,
            next_offset: Some(pos),
            total_bytes: total,
        }))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn join_argv_masks_env_values_and_key_patterns() {
        let argv = [
            "acc",
            "--token",
            "hunter2-secret",
            "sk-abcdef1234567890",
            "plain",
        ]
        .map(String::from);
        let env = [("API".to_string(), "hunter2-secret".to_string())];
        assert_eq!(
            mask_argv(&argv, &env),
            ["acc", "--token", "***", "***", "plain"].map(String::from)
        );
    }

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

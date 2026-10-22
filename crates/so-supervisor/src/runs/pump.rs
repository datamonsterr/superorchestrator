//! The per-run event pump: harness events in, store events, ledger rows and the run's
//! outcome out (SPEC 12).

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use jsonschema::Validator;
use serde_json::{Value, json};
use so_core::{AccountFile, ErrorKind, EventType, Harness, RunState};
use so_harness::{Channel, HarnessEvent, RunControl};
use so_store::{LedgerEntry, RunPatch};
use so_usage::{TokioProbeRunner, UsageService, parse_claude_rate_limit_event};
use tokio::sync::mpsc::UnboundedReceiver;

use super::manager::{RunManager, excerpt};

/// Re-prompts allowed when the final result does not match `output_schema`.
const MAX_REPROMPTS: u32 = 5;
const SHUTDOWN_DEADLINE: Duration = Duration::from_secs(5);
const EXCERPT_CHARS: usize = 280;

pub(crate) struct PumpCtx {
    pub run_id: String,
    pub session_id: String,
    pub harness: Harness,
    pub account: AccountFile,
    pub pool: String,
    pub validator: Option<Validator>,
}

#[derive(Default)]
struct State {
    /// The run reached its outcome; later events are ignored.
    done: bool,
    reprompts: u32,
    /// Transcript offset of the latest raw line.
    offset: Option<u64>,
    /// An error event seen outside a turn, used if the harness then exits.
    pending_error: Option<(ErrorKind, String)>,
}

impl RunManager {
    pub(crate) async fn pump(
        self,
        pc: PumpCtx,
        mut rx: UnboundedReceiver<HarnessEvent>,
        control: RunControl,
    ) {
        let usage = self.host().map(|h| {
            UsageService::new(
                self.store().clone(),
                self.config().clone(),
                h.clone(),
                Arc::new(TokioProbeRunner),
            )
        });
        let mut st = State::default();
        while let Some(ev) = rx.recv().await {
            // A cancel finishes the run itself; whatever the dying process says is noise.
            if st.done || self.is_cancelling(&pc.run_id) {
                continue;
            }
            self.on_event(&pc, &mut st, &control, usage.as_ref(), ev)
                .await;
        }
        if !st.done && !self.is_cancelling(&pc.run_id) {
            self.exited(&pc, &mut st, None).await;
        }
    }

    async fn emit_run(&self, pc: &PumpCtx, st: &State, t: EventType, mut data: Value) {
        if let (Some(off), Some(obj)) = (st.offset, data.as_object_mut()) {
            obj.insert("transcript_offset".into(), json!(off));
        }
        self.emit(&pc.session_id, Some(&pc.run_id), t, data).await;
    }

    async fn on_event(
        &self,
        pc: &PumpCtx,
        st: &mut State,
        control: &RunControl,
        usage: Option<&UsageService>,
        ev: HarnessEvent,
    ) {
        match ev {
            HarnessEvent::SessionStarted { native_session_id } => {
                let patch = RunPatch {
                    native_session_id: Some(Some(native_session_id)),
                    ..RunPatch::default()
                };
                let id = pc.run_id.clone();
                let _ = self.db(move |s| s.patch_run(&id, &patch)).await;
            }
            HarnessEvent::TurnStarted => {}
            HarnessEvent::AssistantMessage { text } => {
                self.emit_run(
                    pc,
                    st,
                    EventType::AssistantMessage,
                    json!({"excerpt": excerpt(&text, EXCERPT_CHARS)}),
                )
                .await;
            }
            HarnessEvent::ToolCallStarted { name } => {
                self.emit_run(pc, st, EventType::ToolCallStarted, json!({"name": name}))
                    .await;
            }
            HarnessEvent::ToolCallFinished { name, ok } => {
                self.emit_run(
                    pc,
                    st,
                    EventType::ToolCallFinished,
                    json!({"name": name, "ok": ok}),
                )
                .await;
            }
            HarnessEvent::Usage {
                input_tokens,
                output_tokens,
                cost,
                rate_limits,
            } => {
                self.emit_run(
                    pc,
                    st,
                    EventType::Usage,
                    json!({
                        "input_tokens": input_tokens,
                        "output_tokens": output_tokens,
                        "cost": cost,
                        "rate_limits": rate_limits,
                    }),
                )
                .await;
                let entry = LedgerEntry {
                    account: pc.account.name.clone(),
                    pool: pc.pool.clone(),
                    run_id: Some(pc.run_id.clone()),
                    ts: Utc::now(),
                    input_tokens: i64::try_from(input_tokens).unwrap_or(i64::MAX),
                    output_tokens: i64::try_from(output_tokens).unwrap_or(i64::MAX),
                    cost: cost.unwrap_or(0.0),
                };
                if let Err(e) = self.db(move |s| s.insert_ledger(&entry).map(|_| ())).await {
                    tracing::warn!(run_id = %pc.run_id, error = %e.message, "could not record usage");
                }
            }
            HarnessEvent::Raw {
                channel,
                line,
                offset,
            } => {
                st.offset = offset;
                if channel == Channel::Stdout
                    && pc.harness == Harness::Claude
                    && line.contains("rate_limit_event")
                    && let Some(svc) = usage
                    && let Ok(v) = serde_json::from_str::<Value>(&line)
                {
                    let readings = parse_claude_rate_limit_event(&v);
                    if !readings.is_empty()
                        && let Err(e) = svc.ingest_event(&pc.account, readings, Utc::now()).await
                    {
                        tracing::debug!(error = %e, "could not record passive usage");
                    }
                }
            }
            HarnessEvent::Error { kind, message } => {
                st.pending_error = Some((kind, message));
            }
            HarnessEvent::TurnFinished {
                result_text,
                structured,
                is_error,
                error_kind,
                error_text,
            } => {
                if is_error {
                    let kind = error_kind.unwrap_or(ErrorKind::Other);
                    let text = error_text
                        .or(result_text)
                        .unwrap_or_else(|| "the turn failed".to_string());
                    self.end_failed(pc, st, control, kind, &text).await;
                } else {
                    self.on_result(pc, st, control, result_text, structured)
                        .await;
                }
            }
            HarnessEvent::Exited { code } => self.exited(pc, st, code).await,
        }
    }

    async fn end_failed(
        &self,
        pc: &PumpCtx,
        st: &mut State,
        control: &RunControl,
        kind: ErrorKind,
        text: &str,
    ) {
        st.done = true;
        let _ = control.shutdown(SHUTDOWN_DEADLINE).await;
        self.fail_run(&pc.run_id, kind, text).await;
    }

    async fn exited(&self, pc: &PumpCtx, st: &mut State, code: Option<i32>) {
        if st.done {
            return;
        }
        st.done = true;
        let (kind, text) = st.pending_error.take().unwrap_or_else(|| {
            (
                ErrorKind::Other,
                format!(
                    "the harness exited ({}) before producing a result",
                    code.map_or("no exit code".to_string(), |c| format!("code {c}"))
                ),
            )
        });
        self.fail_run(&pc.run_id, kind, &text).await;
    }

    async fn on_result(
        &self,
        pc: &PumpCtx,
        st: &mut State,
        control: &RunControl,
        text: Option<String>,
        structured: Option<Value>,
    ) {
        let value = match &pc.validator {
            None => structured
                .or_else(|| text.map(Value::String))
                .unwrap_or(Value::Null),
            Some(validator) => {
                let candidate = structured.or_else(|| text.as_deref().and_then(parse_json_result));
                let problem = match &candidate {
                    None => Some("the final message is not a JSON value".to_string()),
                    Some(v) => {
                        let errors: Vec<String> = validator
                            .iter_errors(v)
                            .take(5)
                            .map(|e| e.to_string())
                            .collect();
                        (!errors.is_empty()).then(|| errors.join("; "))
                    }
                };
                if let Some(problem) = problem {
                    if st.reprompts >= MAX_REPROMPTS {
                        let msg = format!(
                            "the result did not match output_schema after {MAX_REPROMPTS} re-prompts: {problem}"
                        );
                        self.end_failed(pc, st, control, ErrorKind::Other, &msg)
                            .await;
                        return;
                    }
                    st.reprompts += 1;
                    let msg = format!(
                        "Your final result did not match the required JSON schema: {problem}. \
                         Reply again with only a JSON value that matches the schema."
                    );
                    if let Err(e) = control.send(&msg).await {
                        let msg = format!("could not re-prompt the harness: {e}");
                        self.end_failed(pc, st, control, ErrorKind::Other, &msg)
                            .await;
                    }
                    return;
                }
                candidate.unwrap_or(Value::Null)
            }
        };
        st.done = true;
        let _ = control.shutdown(SHUTDOWN_DEADLINE).await;
        let json = serde_json::to_string(&value).ok();
        // Review gate (`review: true`) is added later; a finished run is succeeded.
        self.finish(&pc.run_id, RunState::Succeeded, None, json, None)
            .await;
    }
}

/// A JSON value from a final message, tolerating a Markdown code fence around it.
fn parse_json_result(text: &str) -> Option<Value> {
    let t = text.trim();
    let t = t
        .strip_prefix("```json")
        .or_else(|| t.strip_prefix("```"))
        .map_or(t, |rest| rest.trim_end().trim_end_matches("```").trim());
    serde_json::from_str(t).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_result_parsing() {
        assert_eq!(parse_json_result(" {\"a\": 1} "), Some(json!({"a": 1})));
        assert_eq!(
            parse_json_result("```json\n{\"a\": 1}\n```"),
            Some(json!({"a": 1}))
        );
        assert_eq!(parse_json_result("done"), None);
    }
}

//! The per-run event pump: harness events in, store events, ledger rows and the run's
//! outcome out (SPEC 12).

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use jsonschema::Validator;
use serde_json::{Value, json};
use so_core::{AccountFile, ErrorKind, EventType, Harness};
use so_harness::{Channel, HarnessEvent, RunControl};
use so_store::{LedgerEntry, RunPatch};
use so_usage::{TokioProbeRunner, UsageService, parse_claude_rate_limit_event};
use tokio::sync::mpsc::UnboundedReceiver;

use so_core::{MessageKind, RunState};

use super::RunManager;
use super::lifecycle::{Outcome, excerpt};
use super::review::Gate;

/// Re-prompts allowed when the final result does not match `output_schema`.
const MAX_REPROMPTS: u32 = 5;
const EXCERPT_CHARS: usize = 280;
/// Nudges sent to a stalled run before it is escalated (SPEC 7.6).
const MAX_NUDGES: u32 = 3;
const NUDGE_TEXT: &str = "Status? Continue, or call ask_parent if blocked.";
const RESUME_AFTER_DEATH: &str =
    "Your process stopped unexpectedly. Continue the task from where you were.";

pub(crate) struct PumpCtx {
    pub run_id: String,
    pub session_id: String,
    pub harness: Harness,
    pub account: AccountFile,
    pub pool: String,
    pub validator: Option<Validator>,
    pub review: bool,
}

/// Where a run is in stall handling.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Stage {
    #[default]
    Active,
    Stalled,
    Escalated,
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
    stage: Stage,
    nudges: u32,
    /// Times the review checks failed and were sent back to the run.
    review_failures: u32,
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
        let stall = Duration::from_millis(self.config().runs.stall_ms.max(1));
        let mut check_at = tokio::time::Instant::now() + stall;
        loop {
            let ev = match tokio::time::timeout_at(check_at, rx.recv()).await {
                Ok(Some(ev)) => ev,
                Ok(None) => break,
                Err(_) => {
                    check_at = tokio::time::Instant::now() + stall;
                    self.on_idle(&pc, &mut st, &control).await;
                    continue;
                }
            };
            // A cancel finishes the run itself; whatever the dying process says is noise.
            if st.done || self.is_cancelling(&pc.run_id) {
                continue;
            }
            // Our own input (a nudge, a message) is echoed as a raw line: not the
            // harness showing signs of life.
            let echo = matches!(
                &ev,
                HarnessEvent::Raw {
                    channel: Channel::Stdin,
                    ..
                }
            );
            if !echo {
                check_at = tokio::time::Instant::now() + stall;
                if st.stage != Stage::Active {
                    self.recover(&pc, &mut st).await;
                }
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
        let o = Outcome::failed(kind, text, Some(control.clone()));
        self.finish(&pc.run_id, o).await;
    }

    /// No harness event for `runs.stall_ms`: nudge up to three times, then escalate
    /// (SPEC 7.6). A run that is waiting on a call of its own is not stalled.
    async fn on_idle(&self, pc: &PumpCtx, st: &mut State, control: &RunControl) {
        if st.done || self.is_cancelling(&pc.run_id) || self.is_waiting(&pc.run_id) {
            return;
        }
        let idle_ms = self.config().runs.stall_ms;
        match st.stage {
            Stage::Active => {
                if !self
                    .move_state(
                        &pc.run_id,
                        RunState::Running,
                        RunState::Stalled,
                        "no events",
                    )
                    .await
                {
                    return;
                }
                st.stage = Stage::Stalled;
                self.emit(
                    &pc.session_id,
                    Some(&pc.run_id),
                    EventType::StallDetected,
                    json!({"idle_ms": idle_ms}),
                )
                .await;
                self.nudge(pc, st, control).await;
            }
            Stage::Stalled if st.nudges < MAX_NUDGES => self.nudge(pc, st, control).await,
            Stage::Stalled => {
                if !self
                    .move_state(
                        &pc.run_id,
                        RunState::Stalled,
                        RunState::Escalated,
                        "no reply to nudges",
                    )
                    .await
                {
                    return;
                }
                st.stage = Stage::Escalated;
                self.escalated(pc).await;
            }
            Stage::Escalated => {}
        }
    }

    async fn nudge(&self, pc: &PumpCtx, st: &mut State, control: &RunControl) {
        st.nudges += 1;
        if let Err(e) = control.send(NUDGE_TEXT).await {
            tracing::debug!(run_id = %pc.run_id, error = %e, "could not send a nudge");
        }
        let _ = self
            .store_message(
                &pc.session_id,
                None,
                Some(&pc.run_id),
                MessageKind::Nudge,
                NUDGE_TEXT,
                true,
            )
            .await;
        self.emit(
            &pc.session_id,
            Some(&pc.run_id),
            EventType::NudgeSent,
            json!({"n": st.nudges}),
        )
        .await;
    }

    async fn escalated(&self, pc: &PumpCtx) {
        self.emit(
            &pc.session_id,
            Some(&pc.run_id),
            EventType::Escalated,
            json!({}),
        )
        .await;
        let id = pc.run_id.clone();
        let row = self.db(move |s| s.get_run(&id)).await.ok().flatten();
        let parent = row.as_ref().and_then(|r| r.parent_run_id.clone());
        let name = row
            .and_then(|r| r.name)
            .unwrap_or_else(|| pc.run_id.clone());
        let _ = self
            .store_message(
                &pc.session_id,
                Some(&pc.run_id),
                parent.as_deref(),
                MessageKind::Message,
                &format!(
                    "Run {} is stalled: it did not answer {MAX_NUDGES} nudges.",
                    pc.run_id
                ),
                false,
            )
            .await;
        self.notify(
            "escalated",
            &pc.run_id,
            &name,
            "the run is stalled and needs attention",
        )
        .await;
    }

    /// A harness event after a stall: back to `running`.
    async fn recover(&self, pc: &PumpCtx, st: &mut State) {
        let from = match st.stage {
            Stage::Stalled => RunState::Stalled,
            Stage::Escalated => RunState::Escalated,
            Stage::Active => return,
        };
        self.move_state(&pc.run_id, from, RunState::Running, "activity")
            .await;
        st.stage = Stage::Active;
        st.nudges = 0;
    }

    async fn exited(&self, pc: &PumpCtx, st: &mut State, code: Option<i32>) {
        if st.done {
            return;
        }
        st.done = true;
        let text = match st.pending_error.take() {
            // A quota, auth or similar error: resuming would only repeat it.
            Some(err) => err,
            None => {
                let text = format!(
                    "the harness exited ({}) before producing a result",
                    code.map_or("no exit code".to_string(), |c| format!("code {c}"))
                );
                // The process died on its own: resume it once from its native session.
                match self.prepare_resume_after_death(pc).await {
                    Ok(row) => {
                        // Resumed on another task: `go_live` starts a pump, and a pump
                        // that awaited it would be a future containing itself.
                        self.request_resume(row, RESUME_AFTER_DEATH);
                        return;
                    }
                    Err(why) => {
                        tracing::debug!(run_id = %pc.run_id, %why, "not resumed after exit");
                    }
                }
                (ErrorKind::Other, text)
            }
        };
        self.fail_run(&pc.run_id, text.0, &text.1).await;
    }

    /// Checks that a run whose process died can be resumed (once, SPEC 7.6) and gets it
    /// ready: the dead attempt is closed and the run is `running` again.
    async fn prepare_resume_after_death(&self, pc: &PumpCtx) -> Result<so_store::RunRow, String> {
        let id = pc.run_id.clone();
        let (row, attempts) = self
            .db(move |s| Ok((s.get_run(&id)?, s.list_attempts(&id, 10)?)))
            .await
            .map_err(|e| e.message)?;
        let row = row.ok_or("run is gone")?;
        if !row.state.is_active() || row.native_session_id.is_none() {
            return Err("nothing to resume".into());
        }
        if attempts.len() >= 2 {
            return Err("already resumed once".into());
        }
        // Close the dead attempt; the resume opens the next one.
        if let Some(a) = attempts.iter().rev().find(|a| a.ended_at.is_none()) {
            let aid = a.id.clone();
            let _ = self
                .db(move |s| s.finish_attempt(&aid, "died", Utc::now()))
                .await;
        }
        // The run is running again, whatever it was doing when its process died.
        for from in [RunState::Stalled, RunState::Escalated, RunState::Waiting] {
            if row.state == from {
                self.move_state(&row.id, from, RunState::Running, "process died")
                    .await;
            }
        }
        Ok(row)
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
        let json = serde_json::to_string(&value).ok();
        if pc.review {
            match self
                .review_gate(&pc.run_id, control, json, st.review_failures)
                .await
            {
                Gate::Retry => st.review_failures += 1,
                Gate::Entered => st.done = true,
            }
            return;
        }
        st.done = true;
        self.finish(&pc.run_id, Outcome::succeeded(json, control.clone()))
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

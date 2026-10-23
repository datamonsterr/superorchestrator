//! Claude Code driver (ADR-0004 "claude"): `claude -p` with stream-json on stdin
//! and stdout. One process serves many turns; closing stdin ends it.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use so_core::{ErrorKind, Harness};
use tokio::process::Command;
use tokio::sync::oneshot;

use crate::driver::{
    Channel, Driver, DriverConfig, DriverError, HarnessEvent, JoinCommand, JoinInfo, LaunchSpec,
    RateLimits, RunControl, RunHandle, ShutdownOutcome, check_effort, classify_common,
};
use crate::process::{
    LineParser, ProcessHandle, create_private_dir, default_truncated_event, spawn_supervised,
    write_private,
};

/// How long `interrupt` waits for claude's `control_response`.
const INTERRUPT_ACK_TIMEOUT: Duration = Duration::from_secs(5);

/// Driver for `claude`.
#[derive(Debug, Clone, Default)]
pub struct ClaudeDriver {
    config: DriverConfig,
}

impl ClaudeDriver {
    pub fn new(config: DriverConfig) -> Self {
        Self { config }
    }
}

/// Harness arguments (after `program argv_prefix extra_args`). The prompt is
/// never part of argv.
///
/// `--dangerously-skip-permissions` is deliberate: SPEC 4 ("Full permissions")
/// requires workers to run unattended with every permission granted.
pub fn build_args(spec: &LaunchSpec, session_id: &str) -> Vec<String> {
    let mut a: Vec<String> = [
        "-p",
        "--verbose",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--dangerously-skip-permissions",
    ]
    .map(String::from)
    .into();
    a.push(if spec.resume.is_some() {
        "--resume".into()
    } else {
        "--session-id".into()
    });
    a.push(session_id.into());
    if spec.resume.is_some() && spec.fork {
        a.push("--fork-session".into());
    }
    a.push("--mcp-config".into());
    a.push(spec.run_dir.join("mcp.json").display().to_string());
    a.push("--append-system-prompt-file".into());
    a.push(spec.run_dir.join("system.md").display().to_string());
    if let Some(m) = &spec.model {
        a.extend(["--model".into(), m.clone()]);
    }
    if let Some(e) = &spec.effort {
        a.extend(["--effort".into(), e.clone()]);
    }
    if let Some(s) = &spec.output_schema {
        a.extend(["--json-schema".into(), s.to_string()]);
    }
    if let Some(n) = spec.max_turns {
        a.extend(["--max-turns".into(), n.to_string()]);
    }
    // `launch` validates the agent arguments against the shared explicit allowlist.
    // Tool lists and extra config come last because permission lists are variadic.
    a.extend(spec.agent.extra_args.iter().cloned());
    a
}

fn write_run_files(spec: &LaunchSpec) -> std::io::Result<()> {
    create_private_dir(&spec.run_dir)?;
    let env: serde_json::Map<String, Value> = spec
        .mcp
        .env
        .iter()
        .map(|(k, v)| (k.clone(), Value::String(v.clone())))
        .collect();
    let mcp = json!({"mcpServers": {"superorchestrator": {
        "command": spec.mcp.command,
        "args": spec.mcp.args,
        "env": env,
    }}});
    write_private(
        &spec.run_dir.join("mcp.json"),
        &serde_json::to_vec_pretty(&mcp)?,
    )?;
    write_private(
        &spec.run_dir.join("system.md"),
        spec.system_addendum.as_bytes(),
    )
}

impl Driver for ClaudeDriver {
    fn harness(&self) -> Harness {
        Harness::Claude
    }

    async fn launch(&self, spec: LaunchSpec) -> Result<RunHandle, DriverError> {
        check_effort(Harness::Claude, spec.effort.as_deref())?;
        so_agents::materialise::validate_claude_extra_args(&spec.agent.extra_args)
            .map_err(DriverError::Other)?;
        write_run_files(&spec)?;
        let session_id = spec
            .resume
            .clone()
            .unwrap_or_else(|| uuid::Uuid::now_v7().to_string());

        let mut cmd = Command::new(&spec.launch.program);
        cmd.args(&spec.launch.argv_prefix)
            .args(&spec.launch.extra_args)
            .args(build_args(&spec, &session_id))
            .envs(spec.launch.vars.iter().map(|(k, v)| (k, v)))
            .current_dir(&spec.cwd);
        let acks = Acks::default();
        let sup = spawn_supervised(
            cmd,
            &spec.run_dir,
            self.config.shutdown_grace,
            ClaudeParser::new(acks.clone()),
        )?;
        let control = ClaudeControl {
            proc: Arc::new(sup.handle),
            acks,
            seq: Arc::new(AtomicU32::new(0)),
        };
        if let Err(e) = control.send(&spec.prompt).await {
            control.proc.shutdown(Duration::ZERO).await;
            return Err(e);
        }
        Ok(RunHandle {
            events: sup.events,
            control: RunControl::Claude(control),
            // A fork reports its new session id in the init message.
            native_session_id: (!spec.fork).then(|| session_id.clone()),
            join: JoinInfo {
                account_name: spec.account.name.clone(),
                native_session_id: if spec.fork { String::new() } else { session_id },
                endpoint: None,
                cwd: spec.cwd.clone(),
            },
        })
    }

    /// `<account wrapper> --resume <uuid>`, run from the run's cwd with the
    /// session paused or ended (the live process owns stdin).
    fn join_command(&self, j: &JoinInfo) -> JoinCommand {
        JoinCommand {
            argv: vec![
                j.account_name.clone(),
                "--resume".into(),
                j.native_session_id.clone(),
            ],
            env: vec![],
            cwd: j.cwd.clone(),
        }
    }

    fn classify_error(&self, text: &str) -> ErrorKind {
        classify_common(text)
    }
}

/// Pending interrupt acknowledgements, keyed by `request_id`.
#[derive(Debug, Clone, Default)]
pub struct Acks(Arc<Mutex<HashMap<String, oneshot::Sender<bool>>>>);

impl Acks {
    fn register(&self, id: &str) -> oneshot::Receiver<bool> {
        let (tx, rx) = oneshot::channel();
        if let Ok(mut m) = self.0.lock() {
            m.insert(id.to_string(), tx);
        }
        rx
    }

    /// Resolves every pending request as failed (the process is gone).
    fn fail_all(&self) {
        let all: Vec<_> = self
            .0
            .lock()
            .map(|mut m| m.drain().collect())
            .unwrap_or_default();
        for (_, tx) in all {
            let _ = tx.send(false);
        }
    }

    fn forget(&self, id: &str) {
        if let Ok(mut m) = self.0.lock() {
            m.remove(id);
        }
    }

    fn resolve(&self, id: &str, ok: bool) {
        let tx = self.0.lock().ok().and_then(|mut m| m.remove(id));
        if let Some(tx) = tx {
            let _ = tx.send(ok);
        }
    }
}

/// Clonable control handle for a running claude process. Dropping the last
/// clone kills the process group.
#[derive(Debug, Clone)]
pub struct ClaudeControl {
    proc: Arc<ProcessHandle>,
    acks: Acks,
    seq: Arc<AtomicU32>,
}

impl ClaudeControl {
    pub async fn send(&self, text: &str) -> Result<(), DriverError> {
        self.proc
            .stdin
            .write_line(user_message(text).to_string())
            .await
    }

    /// Writes the interrupt `control_request` and waits for claude's
    /// `control_response`.
    pub async fn interrupt(&self) -> Result<(), DriverError> {
        let n = self.seq.fetch_add(1, Ordering::Relaxed) + 1;
        let id = format!("so-int-{n}");
        let ack = self.acks.register(&id);
        let req = json!({
            "type": "control_request",
            "request_id": id,
            "request": {"subtype": "interrupt"},
        });
        if let Err(e) = self.proc.stdin.write_line(req.to_string()).await {
            self.acks.forget(&id);
            return Err(e);
        }
        match tokio::time::timeout(INTERRUPT_ACK_TIMEOUT, ack).await {
            Ok(Ok(true)) => Ok(()),
            Ok(Ok(false)) => Err(DriverError::Other(
                "claude rejected the interrupt or exited".into(),
            )),
            _ => {
                self.acks.forget(&id);
                Err(DriverError::Other(
                    "interrupt was not acknowledged within 5 s".into(),
                ))
            }
        }
    }

    pub async fn shutdown(&self, deadline: Duration) -> Result<ShutdownOutcome, DriverError> {
        Ok(self.proc.shutdown(deadline).await)
    }

    pub fn pid(&self) -> Option<u32> {
        self.proc.pid
    }
}

fn user_message(text: &str) -> Value {
    json!({"type": "user", "message": {"role": "user", "content": [{"type": "text", "text": text}]}})
}

/// Stateful stdout parser: remembers tool names so results can be named.
#[derive(Debug, Default)]
pub struct ClaudeParser {
    started: bool,
    turn_open: bool,
    tools: HashMap<String, String>,
    rate_limits: Option<RateLimits>,
    acks: Acks,
}

impl ClaudeParser {
    pub fn new(acks: Acks) -> Self {
        Self {
            acks,
            ..Self::default()
        }
    }

    /// Normalised events for one stdout line (without the `Raw` event).
    /// Non-JSON lines produce nothing.
    pub fn parse_line(&mut self, line: &str) -> Vec<HarnessEvent> {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            return vec![];
        };
        match v["type"].as_str() {
            Some("system") if v["subtype"] == "init" => self.init(&v),
            Some("assistant") => self.assistant(&v),
            Some("user") => self.user(&v),
            Some("rate_limit_event") => self.rate_limit(&v),
            Some("result") => self.result(&v),
            Some("control_response") => {
                let r = &v["response"];
                if let Some(id) = r["request_id"].as_str() {
                    self.acks.resolve(id, r["subtype"] == "success");
                }
                vec![]
            }
            _ => vec![],
        }
    }

    /// Claude re-sends `init` every turn: the first starts the session, each
    /// starts a turn.
    fn init(&mut self, v: &Value) -> Vec<HarnessEvent> {
        let mut out = Vec::new();
        if !self.started {
            self.started = true;
            if let Some(s) = v["session_id"].as_str() {
                out.push(HarnessEvent::SessionStarted {
                    native_session_id: s.to_string(),
                });
            }
        }
        self.turn_open = true;
        out.push(HarnessEvent::TurnStarted);
        out
    }

    fn assistant(&mut self, v: &Value) -> Vec<HarnessEvent> {
        let mut out = Vec::new();
        for block in v["message"]["content"].as_array().into_iter().flatten() {
            match block["type"].as_str() {
                Some("text") => {
                    if let Some(text) = block["text"].as_str() {
                        out.push(HarnessEvent::AssistantMessage { text: text.into() });
                    }
                }
                Some("tool_use") => {
                    let name = block["name"].as_str().unwrap_or("unknown").to_string();
                    if let Some(id) = block["id"].as_str() {
                        self.tools.insert(id.into(), name.clone());
                    }
                    out.push(HarnessEvent::ToolCallStarted { name });
                }
                _ => {}
            }
        }
        out
    }

    fn user(&mut self, v: &Value) -> Vec<HarnessEvent> {
        v["message"]["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|b| b["type"] == "tool_result")
            .map(|b| HarnessEvent::ToolCallFinished {
                name: b["tool_use_id"]
                    .as_str()
                    .and_then(|id| self.tools.remove(id))
                    .unwrap_or_else(|| "unknown".into()),
                ok: !b["is_error"].as_bool().unwrap_or(false),
            })
            .collect()
    }

    /// Kept and attached to the next `Usage`. Only a known rejection status is
    /// an error (`allowed` and `allowed_warning` are not).
    fn rate_limit(&mut self, v: &Value) -> Vec<HarnessEvent> {
        let info = &v["rate_limit_info"];
        let win = &info["unifiedWindows"];
        self.rate_limits = Some(RateLimits {
            five_hour_used_pct: win["five_hour"]["utilization"].as_f64().map(|u| u * 100.0),
            five_hour_resets_at: win["five_hour"]["resetsAt"].as_i64(),
            weekly_used_pct: win["seven_day"]["utilization"].as_f64().map(|u| u * 100.0),
            weekly_resets_at: win["seven_day"]["resetsAt"].as_i64(),
        });
        match info["status"].as_str() {
            Some(s @ ("rejected" | "exceeded")) => vec![HarnessEvent::Error {
                kind: ErrorKind::Quota,
                message: format!(
                    "rate limit status {s} ({})",
                    info["rateLimitType"].as_str().unwrap_or("unknown")
                ),
            }],
            _ => vec![],
        }
    }

    fn result(&mut self, v: &Value) -> Vec<HarnessEvent> {
        self.turn_open = false;
        let u = &v["usage"];
        let n = |k: &str| u[k].as_u64().unwrap_or(0);
        let usage = HarnessEvent::Usage {
            input_tokens: n("input_tokens")
                + n("cache_creation_input_tokens")
                + n("cache_read_input_tokens"),
            output_tokens: n("output_tokens"),
            cost: v["total_cost_usd"].as_f64(),
            rate_limits: self.rate_limits.take(),
        };
        let is_error = v["is_error"].as_bool().unwrap_or(false);
        let result_text = v["result"].as_str().map(String::from);
        let error_text = is_error.then(|| {
            result_text
                .clone()
                .or_else(|| v["subtype"].as_str().map(String::from))
                .unwrap_or_else(|| "claude reported an error".into())
        });
        let error_kind = error_text.as_deref().map(classify_common);
        vec![
            usage,
            HarnessEvent::TurnFinished {
                result_text,
                structured: v.get("structured_output").filter(|s| !s.is_null()).cloned(),
                is_error,
                error_kind,
                error_text,
            },
        ]
    }
}

impl LineParser for ClaudeParser {
    fn parse_line(&mut self, line: &str) -> Vec<HarnessEvent> {
        ClaudeParser::parse_line(self, line)
    }

    /// The dropped line may have been the turn's result, so an open turn is
    /// closed as failed rather than left hanging.
    fn on_truncated(&mut self, _channel: Channel) -> Vec<HarnessEvent> {
        self.tools.clear();
        if std::mem::take(&mut self.turn_open) {
            vec![HarnessEvent::TurnFinished {
                result_text: None,
                structured: None,
                is_error: true,
                error_kind: Some(ErrorKind::Other),
                error_text: Some("truncated output".into()),
            }]
        } else {
            vec![default_truncated_event()]
        }
    }

    fn on_eof(&mut self) {
        self.acks.fail_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../tests/fixtures/claude/transcript.jsonl");

    #[test]
    fn parses_real_fixture() {
        let mut p = ClaudeParser::default();
        let mut events = Vec::new();
        for l in FIXTURE.lines() {
            let v: Value = serde_json::from_str(l).unwrap();
            if v["dir"] == "in" {
                events.extend(p.parse_line(&v["msg"].to_string()));
            }
        }
        use HarnessEvent::*;
        let kinds: Vec<&str> = events
            .iter()
            .map(|e| match e {
                SessionStarted { .. } => "start",
                TurnStarted => "turn_start",
                AssistantMessage { .. } => "msg",
                ToolCallStarted { .. } => "tool+",
                ToolCallFinished { .. } => "tool-",
                Usage { .. } => "usage",
                TurnFinished { .. } => "turn_end",
                Error { .. } => "err",
                _ => "other",
            })
            .collect();
        assert_eq!(
            kinds,
            [
                "start",
                "turn_start",
                "tool+",
                "tool-",
                "msg",
                "usage",
                "turn_end",
                "turn_start",
                "msg",
                "usage",
                "turn_end"
            ]
        );
        assert_eq!(
            events[0],
            SessionStarted {
                native_session_id: "237270c5-8296-406c-951b-4cc15b8714cf".into()
            }
        );
        assert_eq!(
            events[2],
            ToolCallStarted {
                name: "Bash".into()
            }
        );
        assert_eq!(
            events[3],
            ToolCallFinished {
                name: "Bash".into(),
                ok: true
            }
        );
        let Usage {
            input_tokens,
            output_tokens,
            cost,
            rate_limits,
        } = &events[5]
        else {
            panic!()
        };
        assert_eq!((*input_tokens, *output_tokens), (4 + 39075 + 36210, 194));
        assert!(cost.is_some());
        let rl = rate_limits.as_ref().unwrap();
        assert_eq!(rl.five_hour_used_pct, Some(21.0));
        assert_eq!(rl.weekly_resets_at, Some(1793001600));
        assert!(
            matches!(&events[6], TurnFinished { is_error: false, error_kind: None, result_text: Some(t), .. } if t == "done")
        );
    }

    #[test]
    fn error_result_and_rate_limit_statuses() {
        let mut p = ClaudeParser::default();
        let rl = |status: &str| {
            format!(
                r#"{{"type":"rate_limit_event","rate_limit_info":{{"status":"{status}","rateLimitType":"five_hour"}}}}"#
            )
        };
        assert!(p.parse_line(&rl("allowed")).is_empty());
        assert!(p.parse_line(&rl("allowed_warning")).is_empty());
        assert!(p.parse_line(&rl("something_new")).is_empty());
        assert!(matches!(
            &p.parse_line(&rl("rejected"))[..],
            [HarnessEvent::Error {
                kind: ErrorKind::Quota,
                ..
            }]
        ));
        let ev = p.parse_line(
            r#"{"type":"result","subtype":"error_during_execution","is_error":true,"result":"Invalid API key. Please run /login","usage":{}}"#,
        );
        assert!(matches!(
            ev.last(),
            Some(HarnessEvent::TurnFinished {
                is_error: true,
                error_kind: Some(ErrorKind::Auth),
                ..
            })
        ));
        assert_eq!(ev.len(), 2, "no duplicate Error event");
        assert!(
            p.parse_line("There's an issue: usage limit reached")
                .is_empty()
        );
    }

    #[test]
    fn control_response_resolves_ack() {
        let acks = Acks::default();
        let mut p = ClaudeParser::new(acks.clone());
        let mut ok = acks.register("a");
        let mut bad = acks.register("b");
        p.parse_line(
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"a"}}"#,
        );
        p.parse_line(
            r#"{"type":"control_response","response":{"subtype":"error","request_id":"b"}}"#,
        );
        assert_eq!(ok.try_recv(), Ok(true));
        assert_eq!(bad.try_recv(), Ok(false));
    }
}

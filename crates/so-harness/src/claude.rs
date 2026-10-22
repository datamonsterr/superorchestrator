//! Claude Code driver (ADR-0004 "claude"): `claude -p` with stream-json on stdin
//! and stdout. One process serves many turns; closing stdin ends it.

use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use so_core::{ErrorKind, Harness};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::sync::{mpsc, oneshot, watch};

use crate::driver::{
    Driver, DriverError, HarnessEvent, JoinInfo, LaunchSpec, ProcessGuard, RateLimits, RunControl,
    RunHandle, classify_common,
};
use crate::transcript::{Dir, TranscriptWriter};

type SharedTranscript = Arc<Mutex<TranscriptWriter>>;

/// Driver for `claude`.
#[derive(Debug, Clone)]
pub struct ClaudeDriver {
    shutdown_grace: Duration,
}

impl Default for ClaudeDriver {
    fn default() -> Self {
        Self {
            shutdown_grace: Duration::from_secs(5),
        }
    }
}

impl ClaudeDriver {
    /// How long `shutdown` waits after closing stdin before killing.
    pub fn with_shutdown_grace(mut self, grace: Duration) -> Self {
        self.shutdown_grace = grace;
        self
    }
}

/// Harness arguments (after `program argv_prefix extra_args`). The prompt is
/// never part of argv.
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
    a
}

fn write_run_files(spec: &LaunchSpec) -> std::io::Result<()> {
    std::fs::create_dir_all(&spec.run_dir)?;
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
    std::fs::write(
        spec.run_dir.join("mcp.json"),
        serde_json::to_vec_pretty(&mcp)?,
    )?;
    std::fs::write(spec.run_dir.join("system.md"), &spec.system_addendum)
}

impl Driver for ClaudeDriver {
    fn harness(&self) -> Harness {
        Harness::Claude
    }

    async fn launch(&self, spec: LaunchSpec) -> Result<RunHandle, DriverError> {
        write_run_files(&spec)?;
        let session_id = spec
            .resume
            .clone()
            .unwrap_or_else(|| uuid::Uuid::now_v7().to_string());
        let transcript: SharedTranscript =
            Arc::new(Mutex::new(TranscriptWriter::create(&spec.run_dir)?));

        let mut cmd = Command::new(&spec.launch.program);
        cmd.args(&spec.launch.argv_prefix)
            .args(&spec.launch.extra_args)
            .args(build_args(&spec, &session_id))
            .envs(spec.launch.vars.iter().map(|(k, v)| (k, v)))
            .current_dir(&spec.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .process_group(0);
        let mut child = cmd.spawn().map_err(|source| DriverError::Spawn {
            program: spec.launch.program.display().to_string(),
            source,
        })?;
        let pid = child.id();
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        let (tx, rx) = mpsc::channel(256);
        let out_task = stdout.map(|o| tokio::spawn(read_stdout(o, tx.clone(), transcript.clone())));
        let err_task = stderr.map(|e| tokio::spawn(read_stderr(e, tx.clone(), transcript.clone())));

        let (kill_tx, kill_rx) = oneshot::channel::<()>();
        let (exited_tx, exited_rx) = watch::channel(false);
        tokio::spawn(async move {
            let status = tokio::select! {
                s = child.wait() => s,
                _ = kill_rx => {
                    let _ = child.kill().await;
                    child.wait().await
                }
            };
            for t in [out_task, err_task].into_iter().flatten() {
                let _ = tokio::time::timeout(Duration::from_secs(2), t).await;
            }
            let code = status.ok().and_then(|s| s.code());
            let _ = tx.send(HarnessEvent::Exited { code }).await;
            let _ = exited_tx.send(true);
        });

        let mut control = ClaudeControl {
            stdin,
            guard: ProcessGuard::new(pid, exited_rx.clone()),
            pid,
            kill_tx: Some(kill_tx),
            exited: exited_rx,
            transcript,
            grace: self.shutdown_grace,
            interrupts: 0,
        };
        control.send(&spec.prompt).await?;
        Ok(RunHandle {
            events: rx,
            control: RunControl::Claude(control),
            native_session_id: Some(session_id),
        })
    }

    /// `<account wrapper> --resume <uuid>`, run from the run's cwd with the
    /// session paused or ended (the live process owns stdin).
    fn join_command(&self, j: &JoinInfo) -> Vec<String> {
        vec![
            j.account_name.clone(),
            "--resume".into(),
            j.native_session_id.clone(),
        ]
    }

    fn classify_error(&self, text: &str) -> ErrorKind {
        classify_common(text)
    }
}

/// Control handle for a running claude process.
#[derive(Debug)]
pub struct ClaudeControl {
    stdin: Option<ChildStdin>,
    guard: ProcessGuard,
    pid: Option<u32>,
    kill_tx: Option<oneshot::Sender<()>>,
    exited: watch::Receiver<bool>,
    transcript: SharedTranscript,
    grace: Duration,
    interrupts: u32,
}

impl ClaudeControl {
    async fn write_line(&mut self, v: &Value) -> Result<(), DriverError> {
        let stdin = self.stdin.as_mut().ok_or(DriverError::Closed)?;
        let mut line = v.to_string();
        if let Ok(mut t) = self.transcript.lock() {
            let _ = t.append(Dir::Out, &line, &[]);
        }
        line.push('\n');
        stdin.write_all(line.as_bytes()).await?;
        stdin.flush().await?;
        Ok(())
    }

    pub async fn send(&mut self, text: &str) -> Result<(), DriverError> {
        self.write_line(&user_message(text)).await
    }

    pub async fn interrupt(&mut self) -> Result<(), DriverError> {
        self.interrupts += 1;
        let req = json!({
            "type": "control_request",
            "request_id": format!("so-int-{}", self.interrupts),
            "request": {"subtype": "interrupt"},
        });
        self.write_line(&req).await
    }

    /// Closes stdin, waits `grace` for a clean exit, then kills.
    pub async fn shutdown(&mut self) -> Result<(), DriverError> {
        self.stdin = None;
        if !self.wait_exit(self.grace).await {
            self.guard.terminate(Duration::from_secs(2)).await;
            if let Some(k) = self.kill_tx.take() {
                let _ = k.send(());
            }
            self.wait_exit(Duration::from_secs(5)).await;
        }
        Ok(())
    }

    async fn wait_exit(&mut self, d: Duration) -> bool {
        tokio::time::timeout(d, self.exited.wait_for(|e| *e))
            .await
            .is_ok()
    }

    pub fn pid(&self) -> Option<u32> {
        self.pid
    }
}

fn user_message(text: &str) -> Value {
    json!({"type": "user", "message": {"role": "user", "content": [{"type": "text", "text": text}]}})
}

async fn read_stdout(out: ChildStdout, tx: mpsc::Sender<HarnessEvent>, t: SharedTranscript) {
    let mut lines = BufReader::new(out).lines();
    let mut parser = ClaudeParser::default();
    while let Ok(Some(line)) = lines.next_line().await {
        let events = parser.parse_line(&line);
        let offset = t
            .lock()
            .ok()
            .and_then(|mut w| w.append(Dir::In, &line, &events).ok())
            .unwrap_or(0);
        if tx.send(HarnessEvent::Raw { line, offset }).await.is_err() {
            return;
        }
        for e in events {
            if tx.send(e).await.is_err() {
                return;
            }
        }
    }
}

async fn read_stderr(err: ChildStderr, tx: mpsc::Sender<HarnessEvent>, t: SharedTranscript) {
    let mut lines = BufReader::new(err).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if let Ok(mut w) = t.lock() {
            let _ = w.append(Dir::Err, &line, &[]);
        }
        if tx.send(HarnessEvent::Stderr { line }).await.is_err() {
            return;
        }
    }
}

/// Stateful stdout parser: remembers tool names so results can be named.
#[derive(Debug, Default)]
pub struct ClaudeParser {
    started: bool,
    tools: std::collections::HashMap<String, String>,
    rate_limits: Option<RateLimits>,
}

impl ClaudeParser {
    /// Normalised events for one stdout line (without the `Raw` event).
    pub fn parse_line(&mut self, line: &str) -> Vec<HarnessEvent> {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            return match classify_common(line) {
                ErrorKind::Other => vec![],
                kind => vec![HarnessEvent::Error {
                    kind,
                    message: line.to_string(),
                }],
            };
        };
        match v["type"].as_str() {
            Some("system") if v["subtype"] == "init" && !self.started => {
                self.started = true;
                v["session_id"]
                    .as_str()
                    .map(|s| HarnessEvent::SessionStarted {
                        native_session_id: s.to_string(),
                    })
                    .into_iter()
                    .collect()
            }
            Some("assistant") => self.assistant(&v),
            Some("user") => self.user(&v),
            Some("rate_limit_event") => self.rate_limit(&v),
            Some("result") => self.result(&v),
            _ => vec![],
        }
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

    /// Kept and attached to the next `Usage`; a non-allowed status is an error.
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
            Some(s) if !s.starts_with("allowed") => vec![HarnessEvent::Error {
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
        let u = &v["usage"];
        let n = |k: &str| u[k].as_u64().unwrap_or(0);
        let mut out = vec![HarnessEvent::Usage {
            input_tokens: n("input_tokens")
                + n("cache_creation_input_tokens")
                + n("cache_read_input_tokens"),
            output_tokens: n("output_tokens"),
            cost: v["total_cost_usd"].as_f64(),
            rate_limits: self.rate_limits.take(),
        }];
        let is_error = v["is_error"].as_bool().unwrap_or(false);
        let result_text = v["result"].as_str().map(String::from);
        let error_text = is_error.then(|| {
            result_text
                .clone()
                .or_else(|| v["subtype"].as_str().map(String::from))
                .unwrap_or_else(|| "claude reported an error".into())
        });
        out.push(HarnessEvent::TurnFinished {
            result_text,
            structured: v.get("structured_output").filter(|s| !s.is_null()).cloned(),
            is_error,
            error_text: error_text.clone(),
        });
        if let Some(message) = error_text {
            out.push(HarnessEvent::Error {
                kind: classify_common(&message),
                message,
            });
        }
        out
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
                AssistantMessage { .. } => "msg",
                ToolCallStarted { .. } => "tool+",
                ToolCallFinished { .. } => "tool-",
                Usage { .. } => "usage",
                TurnFinished { .. } => "turn",
                Error { .. } => "err",
                _ => "other",
            })
            .collect();
        assert_eq!(
            kinds,
            [
                "start", "tool+", "tool-", "msg", "usage", "turn", "msg", "usage", "turn"
            ]
        );
        assert_eq!(
            events[0],
            SessionStarted {
                native_session_id: "237270c5-8296-406c-951b-4cc15b8714cf".into()
            }
        );
        assert_eq!(
            events[1],
            ToolCallStarted {
                name: "Bash".into()
            }
        );
        assert_eq!(
            events[2],
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
        } = &events[4]
        else {
            panic!()
        };
        assert_eq!((*input_tokens, *output_tokens), (4 + 39075 + 36210, 194));
        assert!(cost.is_some());
        let rl = rate_limits.as_ref().unwrap();
        assert_eq!(rl.five_hour_used_pct, Some(21.0));
        assert_eq!(rl.weekly_resets_at, Some(1793001600));
        assert!(
            matches!(&events[5], TurnFinished { is_error: false, result_text: Some(t), .. } if t == "done")
        );
    }

    #[test]
    fn error_result_and_rejected_limit() {
        let mut p = ClaudeParser::default();
        let ev = p.parse_line(
            r#"{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","rateLimitType":"five_hour"}}"#,
        );
        assert!(matches!(
            &ev[..],
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
            Some(HarnessEvent::Error {
                kind: ErrorKind::Auth,
                ..
            })
        ));
    }
}

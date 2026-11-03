//! Shared ACP stdio driver.
mod rpc;
pub mod spec;

use crate::{
    driver::{
        Driver, DriverConfig, DriverError, HarnessEvent, JoinCommand, JoinInfo, LaunchSpec,
        RunControl, RunHandle, ShutdownOutcome, check_effort, check_launch_values, classify_common,
    },
    process::{EventSink, ProcessHandle, spawn_supervised, startup_failure},
};
use serde_json::{Value, json};
use so_core::{ErrorKind, Harness};
use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
    time::Duration,
};
use tokio::{
    process::Command,
    sync::{mpsc, oneshot},
};

#[derive(Debug, Clone)]
pub struct AcpDriver {
    harness: Harness,
    config: DriverConfig,
}
impl AcpDriver {
    pub fn new(harness: Harness, config: DriverConfig) -> Self {
        Self { harness, config }
    }
}

pub fn build_args(harness: Harness, launch: &LaunchSpec) -> Vec<String> {
    let Some(s) = spec::spec(harness) else {
        return vec![];
    };
    let mut args: Vec<String> = s.launch.iter().map(|s| (*s).into()).collect();
    if let (Some(flag), Some(model)) = (s.model_flag, &launch.model) {
        args.extend([flag.into(), model.clone()]);
    }
    if let (Some(flag), Some(effort)) = (s.effort_flag, &launch.effort) {
        args.extend([flag.into(), effort.clone()]);
    }
    args
}

async fn write(proc: &ProcessHandle, value: Value) -> Result<(), DriverError> {
    let mut redacted = value.clone();
    if let Some(servers) = redacted
        .pointer_mut("/params/mcpServers")
        .and_then(Value::as_array_mut)
    {
        for server in servers {
            if let Some(env) = server.get_mut("env").and_then(Value::as_array_mut) {
                for item in env {
                    if let Some(value) = item.get_mut("value") {
                        *value = json!("[REDACTED]");
                    }
                }
            }
        }
        proc.stdin
            .write_redacted(value.to_string(), redacted.to_string())
            .await
    } else {
        proc.stdin.write_line(value.to_string()).await
    }
}
async fn request(
    proc: &ProcessHandle,
    rx: &mut mpsc::UnboundedReceiver<Value>,
    id: u64,
    method: &str,
    params: Value,
) -> Result<Value, DriverError> {
    write(
        proc,
        json!({"jsonrpc":"2.0", "id":id, "method":method, "params":params}),
    )
    .await?;
    loop {
        let v = tokio::time::timeout(Duration::from_secs(30), rx.recv())
            .await
            .map_err(|_| DriverError::Other(format!("ACP {method} timed out")))?
            .ok_or_else(|| DriverError::Other(format!("ACP exited during {method}")))?;
        if v.get("method").is_some() && v.get("id").is_some() {
            answer(proc, &v).await?;
        } else if v.get("id").and_then(Value::as_u64) == Some(id) {
            if let Some(err) = v.get("error") {
                return Err(DriverError::Other(format!("ACP {method}: {err}")));
            }
            return Ok(v.get("result").cloned().unwrap_or(Value::Null));
        }
    }
}
async fn answer(proc: &ProcessHandle, v: &Value) -> Result<(), DriverError> {
    let id = &v["id"];
    let response = if v["method"] == "session/request_permission" {
        let options = v["params"]["options"].as_array();
        let selected = ["allow_always", "allow_once"]
            .iter()
            .find_map(|kind| options.and_then(|o| o.iter().find(|x| x["kind"] == *kind)));
        let outcome = selected.map_or_else(
            || json!({"outcome":"cancelled"}),
            |x| json!({"outcome":"selected", "optionId":x["optionId"]}),
        );
        json!({"jsonrpc":"2.0", "id":id, "result":{"outcome":outcome}})
    } else {
        json!({"jsonrpc":"2.0", "id":id, "error":{"code":-32601,"message":"Method not found"}})
    };
    write(proc, response).await
}

impl Driver for AcpDriver {
    fn harness(&self) -> Harness {
        self.harness
    }
    async fn launch(&self, spec: LaunchSpec) -> Result<RunHandle, DriverError> {
        check_launch_values(
            self.harness,
            spec.model.as_deref(),
            spec.effort.as_deref(),
            spec.resume.as_deref(),
        )?;
        check_effort(self.harness, spec.effort.as_deref())?;
        // ACP does not expose a max-turns control.
        if spec.fork {
            return Err(DriverError::Other("ACP does not support fork".into()));
        }
        let row = spec::spec(self.harness).ok_or(DriverError::Unsupported(self.harness))?;
        let mut cmd = Command::new(&spec.launch.program);
        cmd.args(&spec.launch.argv_prefix)
            .args(&spec.launch.extra_args)
            .args(build_args(self.harness, &spec))
            .envs(spec.launch.vars.iter().map(|(k, v)| (k, v)))
            .current_dir(&spec.cwd);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut sup = spawn_supervised(
            cmd,
            &spec.run_dir,
            self.config.shutdown_grace,
            rpc::Parser(tx),
            &spec.launch.pass_env,
        )?;
        let init = async {
            let init = request(&sup.handle, &mut rx, 1, "initialize", json!({"protocolVersion":1,"clientCapabilities":{"fs":{"readTextFile":false,"writeTextFile":false},"terminal":false},"clientInfo":{"name":"superorchestrator","version":env!("CARGO_PKG_VERSION")}})).await?;
            let env: Vec<Value> = spec.mcp.env.iter().map(|(name,value)| json!({"name":name,"value":value})).collect();
            let servers = json!([{"name":"superorchestrator","command":spec.mcp.command,"args":spec.mcp.args,"env":env}]);
            let (method, params) = if let Some(id) = &spec.resume {
                let caps = &init["agentCapabilities"];
                let method = if caps["loadSession"] == true { "session/load" } else if caps["sessionCapabilities"]["resume"].is_object() || caps["sessionCapabilities"]["resume"] == true { "session/resume" } else { return Err(DriverError::Other("ACP agent does not support session load or resume".into())); };
                (method, json!({"sessionId":id,"cwd":spec.cwd,"mcpServers":servers}))
            } else { ("session/new", json!({"cwd":spec.cwd,"mcpServers":servers})) };
            let session = request(&sup.handle, &mut rx, 2, method, params).await?;
            let id = spec.resume.clone().or_else(|| session["sessionId"].as_str().map(str::to_owned)).ok_or_else(|| DriverError::Other("ACP session response missing sessionId".into()))?;
            if row.model_flag.is_none() && let Some(model) = &spec.model {
                let option = session["configOptions"].as_array().and_then(|v| v.iter().find(|v| v["category"] == "model")).ok_or_else(|| DriverError::Other("ACP agent did not offer a model config option".into()))?;
                let valid = option["options"].as_array().is_some_and(|opts| opts.iter().any(|o| o["value"] == *model || o["options"].as_array().is_some_and(|sub| sub.iter().any(|o| o["value"] == *model))));
                if !valid { return Err(DriverError::Other(format!("ACP model {model} is not offered"))); }
                request(&sup.handle, &mut rx, 3, "session/set_config_option", json!({"sessionId":id,"configId":option["id"],"value":model})).await?;
            }
            Ok(id)
        }.await;
        let id = match init {
            Ok(id) => id,
            Err(e) => {
                let exited = sup.handle.exit.borrow().is_some()
                    || matches!(&e, DriverError::Other(text) if text.starts_with("ACP exited during "));
                sup.handle.shutdown(Duration::ZERO).await;
                let failure =
                    startup_failure(e, &mut sup.events, |text| self.classify_error(text)).await;
                return Err(match failure {
                    DriverError::Other(text) if exited => DriverError::Exited {
                        kind: self.classify_error(&text),
                        text,
                    },
                    other => other,
                });
            }
        };
        sup.events_sink.send(HarnessEvent::SessionStarted {
            native_session_id: id.clone(),
        });
        let proc = Arc::new(sup.handle);
        let (ctl_tx, ctl_rx) = mpsc::unbounded_channel();
        let control = AcpControl {
            proc: proc.clone(),
            tx: ctl_tx.clone(),
        };
        let blocks: Vec<Value> = [&spec.system_addendum, &spec.prompt]
            .into_iter()
            .filter(|s| !s.is_empty())
            .map(|s| json!({"type":"text","text":s}))
            .collect();
        let actor = Actor {
            proc: Arc::downgrade(&proc),
            rx,
            ctl_rx,
            events: sup.events_sink,
            session: id.clone(),
            schema: spec.output_schema,
            harness: self.harness,
            seq: 3,
            active: false,
            text: String::new(),
            message: String::new(),
            final_message: String::new(),
            tools: HashMap::new(),
            queue: VecDeque::new(),
            retries: 0,
            interrupt: None,
        };
        tokio::spawn(actor.run(blocks));
        Ok(RunHandle {
            events: sup.events,
            control: RunControl::Acp(control),
            native_session_id: Some(id.clone()),
            join: JoinInfo {
                account_name: spec.account.name,
                native_session_id: id,
                endpoint: None,
                cwd: spec.cwd,
            },
        })
    }
    fn join_command(&self, j: &JoinInfo) -> JoinCommand {
        let flag = spec::spec(self.harness).map_or("--resume", |s| s.join_flag);
        let mut argv = vec![j.account_name.clone()];
        if flag.ends_with('=') {
            argv.push(format!("{flag}{}", j.native_session_id));
        } else {
            argv.extend([flag.into(), j.native_session_id.clone()]);
        }
        JoinCommand {
            argv,
            env: vec![],
            cwd: j.cwd.clone(),
        }
    }
    fn classify_error(&self, text: &str) -> ErrorKind {
        let common = classify_common(text);
        if common != ErrorKind::Other {
            return common;
        }
        let text = text.to_lowercase();
        spec::spec(self.harness)
            .and_then(|row| {
                row.error_patterns
                    .iter()
                    .find(|(pattern, _)| text.contains(pattern))
                    .map(|(_, kind)| *kind)
            })
            .unwrap_or(ErrorKind::Other)
    }
}

#[derive(Debug, Clone)]
pub struct AcpControl {
    proc: Arc<ProcessHandle>,
    tx: mpsc::UnboundedSender<Control>,
}
#[derive(Debug)]
enum Control {
    Send(String),
    Interrupt(oneshot::Sender<()>),
}
impl AcpControl {
    pub async fn send(&self, text: &str) -> Result<(), DriverError> {
        self.tx
            .send(Control::Send(text.into()))
            .map_err(|_| DriverError::Closed)
    }
    pub async fn interrupt(&self) -> Result<(), DriverError> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(Control::Interrupt(tx))
            .map_err(|_| DriverError::Closed)?;
        tokio::time::timeout(Duration::from_secs(5), rx)
            .await
            .map_err(|_| DriverError::Other("ACP cancellation timed out".into()))?
            .map_err(|_| DriverError::Closed)
    }
    pub async fn shutdown(&self, deadline: Duration) -> Result<ShutdownOutcome, DriverError> {
        Ok(self.proc.shutdown(deadline).await)
    }
    pub fn pid(&self) -> Option<u32> {
        self.proc.pid
    }
}

struct Actor {
    proc: std::sync::Weak<ProcessHandle>,
    rx: mpsc::UnboundedReceiver<Value>,
    ctl_rx: mpsc::UnboundedReceiver<Control>,
    events: EventSink,
    session: String,
    schema: Option<Value>,
    harness: Harness,
    seq: u64,
    active: bool,
    text: String,
    message: String,
    final_message: String,
    tools: HashMap<String, String>,
    queue: VecDeque<Vec<Value>>,
    retries: u8,
    interrupt: Option<oneshot::Sender<()>>,
}
impl Actor {
    async fn prompt(&mut self, blocks: Vec<Value>, announce: bool) -> Result<(), DriverError> {
        self.seq += 1;
        self.active = true;
        self.text.clear();
        self.message.clear();
        self.final_message.clear();
        self.tools.clear();
        if announce {
            self.events.send(HarnessEvent::TurnStarted);
        }
        let proc = self.proc.upgrade().ok_or(DriverError::Closed)?;
        write(&proc, json!({"jsonrpc":"2.0","id":self.seq,"method":"session/prompt","params":{"sessionId":self.session,"prompt":blocks}})).await
    }
    fn flush(&mut self) {
        if !self.message.is_empty() {
            self.final_message.clone_from(&self.message);
            self.events.send(HarnessEvent::AssistantMessage {
                text: std::mem::take(&mut self.message),
            });
        }
    }
    fn update(&mut self, v: &Value) {
        let u = &v["params"]["update"];
        match u["sessionUpdate"].as_str().unwrap_or("") {
            "agent_message_chunk" => {
                if u["content"]["type"] == "text"
                    && let Some(t) = u["content"]["text"].as_str()
                {
                    self.text.push_str(t);
                    self.message.push_str(t);
                }
            }
            "tool_call" => {
                self.flush();
                let id = u["toolCallId"].as_str().unwrap_or("").to_owned();
                let name = u["title"]
                    .as_str()
                    .or_else(|| u["kind"].as_str())
                    .unwrap_or("tool")
                    .to_owned();
                self.tools.insert(id, name.clone());
                self.events.send(HarnessEvent::ToolCallStarted { name });
            }
            "tool_call_update" => {
                if let Some(status @ ("completed" | "failed")) = u["status"].as_str() {
                    let id = u["toolCallId"].as_str().unwrap_or("");
                    if let Some(name) = self.tools.remove(id) {
                        self.events.send(HarnessEvent::ToolCallFinished {
                            name,
                            ok: status == "completed",
                        });
                    }
                }
            }
            _ => {}
        }
    }
    async fn complete(&mut self, v: Value) -> Result<(), DriverError> {
        self.flush();
        let result = &v["result"];
        let err = v.get("error");
        // max_tokens and max_turn_requests are successful stops, like end_turn.
        let reason = result["stopReason"].as_str().unwrap_or("end_turn");
        let error_text = err
            .map(ToString::to_string)
            .or_else(|| (reason == "refusal").then(|| "Agent refused the request".to_owned()));
        let kind = if err.is_some() {
            Some(if v["error"]["code"] == -32000 {
                ErrorKind::Auth
            } else {
                AcpDriver::new(self.harness, DriverConfig::default())
                    .classify_error(error_text.as_deref().unwrap_or(""))
            })
        } else if reason == "refusal" {
            Some(ErrorKind::Refused)
        } else {
            None
        };
        if let (Some(input), Some(output)) = (
            result["usage"]["inputTokens"].as_u64(),
            result["usage"]["outputTokens"].as_u64(),
        ) {
            self.events.send(HarnessEvent::Usage {
                input_tokens: input,
                output_tokens: output,
                cost: None,
                rate_limits: None,
            });
        }
        let mut structured = None;
        let mut validation_error = None;
        if kind.is_none()
            && reason != "cancelled"
            && let Some(schema) = &self.schema
        {
            let text = self.final_message.trim();
            let text = text
                .strip_prefix("```json")
                .or_else(|| text.strip_prefix("```"))
                .and_then(|s| s.trim().strip_suffix("```"))
                .unwrap_or(text)
                .trim();
            match serde_json::from_str::<Value>(text) {
                Ok(value) => match jsonschema::validator_for(schema) {
                    Ok(validator) => match validator.validate(&value) {
                        Ok(()) => structured = Some(value),
                        Err(e) => validation_error = Some(e.to_string()),
                    },
                    Err(e) => validation_error = Some(e.to_string()),
                },
                Err(e) => validation_error = Some(e.to_string()),
            }
        }
        if let Some(error) = validation_error.as_ref()
            && self.retries < 5
        {
            self.retries += 1;
            return self.prompt(vec![json!({"type":"text","text":format!("Your response was not valid JSON matching the required schema: {error}. Please respond again with valid JSON.")})], false).await;
        }
        let error_kind = kind.or_else(|| validation_error.as_ref().map(|_| ErrorKind::Other));
        self.events.send(HarnessEvent::TurnFinished {
            result_text: (!self.text.is_empty()).then(|| self.text.clone()),
            structured,
            is_error: error_kind.is_some(),
            error_kind,
            error_text: error_text.or(validation_error),
        });
        self.active = false;
        self.retries = 0;
        if let Some(ack) = self.interrupt.take() {
            let _ = ack.send(());
        }
        if let Some(next) = self.queue.pop_front() {
            self.prompt(next, true).await?;
        }
        Ok(())
    }
    async fn run(mut self, first: Vec<Value>) {
        if self.prompt(first, true).await.is_err() {
            return;
        }
        loop {
            tokio::select! {
                Some(cmd) = self.ctl_rx.recv() => match cmd {
                    Control::Send(text) => { let blocks = vec![json!({"type":"text","text":text})]; if self.active { self.queue.push_back(blocks); } else if self.prompt(blocks,true).await.is_err() { break; } },
                    Control::Interrupt(ack) => if self.active { if let Some(proc) = self.proc.upgrade() { if write(&proc,json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":self.session}})).await.is_err() { break; } self.interrupt = Some(ack); } else { break; } } else { let _ = ack.send(()); },
                },
                Some(v) = self.rx.recv() => {
                    if v.get("method").is_some() && v.get("id").is_some() { if let Some(proc) = self.proc.upgrade() { if answer(&proc,&v).await.is_err() { break; } } else { break; } }
                    else if v["method"] == "session/update" && self.active { self.update(&v); }
                    else if v["id"].as_u64() == Some(self.seq) && self.active && self.complete(v).await.is_err() { break; }
                },
                else => break,
            }
        }
    }
}

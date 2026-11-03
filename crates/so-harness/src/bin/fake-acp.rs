//! Scriptable ACP server for driver integration tests.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use serde_json::{Value, json};
use std::io::{BufRead, Write};

fn emit(v: Value) {
    println!("{v}");
}
fn main() {
    let mode = std::env::var("FAKE_ACP_MODE").unwrap_or_default();
    if let Ok(path) = std::env::var("FAKE_ACP_META") {
        let argv: Vec<_> = std::env::args().collect();
        let env_names: Vec<_> = std::env::vars().map(|(name, _)| name).collect();
        std::fs::write(
            path,
            json!({"argv": argv, "env_names": env_names}).to_string(),
        )
        .unwrap();
    }
    let mut log = std::env::var("FAKE_ACP_LOG").ok().map(|p| {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(p)
            .unwrap()
    });
    let mut turns = 0;
    let mut pending = None;
    for line in std::io::stdin().lock().lines().map_while(Result::ok) {
        if let Some(log) = &mut log {
            writeln!(log, "{line}").unwrap();
        }
        let v: Value = serde_json::from_str(&line).unwrap();
        let id = v["id"].clone();
        let method = v["method"].as_str().unwrap_or("");
        let mut result = None;
        match method {
            "initialize" => {
                if mode == "exit_silent" {
                    return;
                }
                if mode == "exit" {
                    eprintln!("quota exhausted");
                    return;
                }
                if mode == "handshake_auth" {
                    emit(
                        json!({"jsonrpc":"2.0","id":id,"error":{"code":-32000,"message":"credentials required"}}),
                    );
                    continue;
                }
                let capabilities = match mode.as_str() {
                    "resume" => json!({"sessionCapabilities":{"resume":{}}}),
                    "no_resume" => json!({}),
                    _ => json!({"loadSession":true}),
                };
                result = Some(json!({"protocolVersion":1,"agentCapabilities":capabilities}));
            }
            "session/new" | "session/load" | "session/resume" => {
                if method == "session/load" {
                    emit(
                        json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"native-1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"old history"}}}}),
                    );
                }
                result = Some(
                    json!({"sessionId":"native-1","configOptions":[{"id":"model","category":"model","options":[{"value":"test-model","name":"test"}]}]}),
                );
            }
            "session/set_config_option" => result = Some(json!({})),
            "session/prompt" => {
                turns += 1;
                if mode == "exit_prompt" {
                    return;
                }
                if (mode == "slow" || mode == "slow_interrupt") && turns == 1 {
                    pending = Some(id.clone());
                    continue;
                }
                if mode == "auth" || mode == "quota" {
                    emit(
                        json!({"jsonrpc":"2.0","id":id,"error":{"code":if mode == "auth" {-32000} else {-32603},"message":"quota exhausted"}}),
                    );
                    continue;
                }
                if mode == "permission" {
                    emit(
                        json!({"jsonrpc":"2.0","id":700,"method":"session/request_permission","params":{"options":[{"kind":"allow_once","optionId":"once"},{"kind":"allow_always","optionId":"always"}]}}),
                    );
                    emit(json!({"jsonrpc":"2.0","id":701,"method":"unknown/method","params":{}}));
                }
                if mode == "structured" && turns > 1 {
                    emit(
                        json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"native-1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Checking first"}}}}),
                    );
                    emit(
                        json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"native-1","update":{"sessionUpdate":"tool_call","toolCallId":"first","title":"Check"}}}),
                    );
                }
                let text = if mode == "invalid"
                    || (mode == "structured" && turns == 1)
                    || (mode == "structured_twice" && turns % 6 != 0)
                {
                    "invalid"
                } else if mode == "structured" || mode == "structured_twice" {
                    "{\"ok\":true}"
                } else {
                    "hello world"
                };
                emit(
                    json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"native-1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":text}}}}),
                );
                emit(
                    json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"native-1","update":{"sessionUpdate":"tool_call","toolCallId":"tool-1","title":"Read"}}}),
                );
                emit(
                    json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"native-1","update":{"sessionUpdate":"tool_call_update","toolCallId":"tool-1","status":if mode == "tool_failed" {"failed"} else {"completed"}}}}),
                );
                let reason = match mode.as_str() {
                    "refusal" | "max_tokens" | "max_turn_requests" => mode.as_str(),
                    _ => "end_turn",
                };
                result = Some(json!({
                    "stopReason": reason,
                    "usage": {"inputTokens": 123, "outputTokens": 45}
                }));
            }
            "session/cancel" => {
                if mode == "slow_interrupt" {
                    std::thread::sleep(std::time::Duration::from_millis(200));
                }
                if let Some(id) = pending.take() {
                    emit(json!({"jsonrpc":"2.0","id":id,"result":{"stopReason":"cancelled"}}));
                }
            }
            _ => {}
        }
        if let Some(result) = result {
            emit(json!({"jsonrpc":"2.0","id":id,"result":result}));
        }
    }
}

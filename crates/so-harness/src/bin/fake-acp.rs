//! Scriptable ACP server for driver integration tests.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use serde_json::{Value, json};
use std::io::{BufRead, Write};

fn emit(v: Value) {
    println!("{v}");
}
fn main() {
    let mode = std::env::var("FAKE_ACP_MODE").unwrap_or_default();
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
                result =
                    Some(json!({"protocolVersion":1,"agentCapabilities":{"loadSession":true}}));
            }
            "session/new" | "session/load" => {
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
                if mode == "slow" && turns == 1 {
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
                let text = if mode == "invalid" || (mode == "structured" && turns == 1) {
                    "invalid"
                } else if mode == "structured" {
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
                    json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"native-1","update":{"sessionUpdate":"tool_call_update","toolCallId":"tool-1","status":"completed"}}}),
                );
                result =
                    Some(json!({"stopReason":if mode == "refusal" {"refusal"} else {"end_turn"}}));
            }
            "session/cancel" => {
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

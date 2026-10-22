//! Test-only scripted stand-in for `codex app-server` (feature `test-support`).
//!
//! Started like the real thing: `fake-codex app-server --listen
//! ws://127.0.0.1:<port> --ws-auth capability-token --ws-token-file <file>`.
//! It serves `GET /readyz` (200) and accepts WebSocket upgrades only with
//! `Authorization: Bearer <contents of the token file>` (else HTTP 401).
//!
//! `FAKE_CODEX_SCRIPT=<path>` is a JSONL script run on the first authenticated
//! connection, in order:
//! - `{"expect": "method", "respond": <result>}`: read one client frame, which
//!   must have this method (else exit 3), and answer it with `result` under the
//!   frame's id. `"respond_error": {"code", "message"}` answers with an error;
//!   without either nothing is sent (notifications). `"check": {"/json/pointer": value}`
//!   asserts values inside the frame (mismatch: exit 3).
//! - `{"expect_response": true}`: read one client frame that has no method
//!   (the answer to a server request)
//! - `{"send": <json>}`: send one frame; `{"sleep_ms": n}`; `{"close": true}`
//!
//! After the script it reads (and logs) frames until the client leaves, then
//! keeps serving like the real server. `FAKE_CODEX_LOG=<path>` receives JSONL:
//! `{"argv", "env_keys"}` first (keys only), then `{"recv": <frame>}` per client
//! frame and `{"auth_rejected": true}` per refused upgrade.
//! `FAKE_CODEX_READY_DELAY_MS` delays binding the port. After binding (port 0 is
//! fine) it prints the real server's banner `  listening on: ws://127.0.0.1:<port>`
//! on stderr; `FAKE_CODEX_BANNER_HOST` replaces the host in it.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::result_large_err)]

use std::fs::OpenOptions;
use std::io::Write as _;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::StatusCode;
use tokio_tungstenite::tungstenite::protocol::Message;

type Log = Arc<Mutex<Option<std::fs::File>>>;

fn log(l: &Log, v: Value) {
    if let Some(f) = l.lock().unwrap().as_mut() {
        writeln!(f, "{v}").expect("write log");
    }
}

fn arg_after(args: &[String], flag: &str) -> String {
    let i = args.iter().position(|a| a == flag).expect("missing flag");
    args[i + 1].clone()
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let listen = arg_after(&args, "--listen");
    let addr = listen.strip_prefix("ws://").expect("ws:// url").to_string();
    let token = std::fs::read_to_string(arg_after(&args, "--ws-token-file"))
        .expect("token file")
        .trim()
        .to_string();
    let l: Log = Arc::new(Mutex::new(std::env::var("FAKE_CODEX_LOG").ok().map(|p| {
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(p)
            .expect("open log")
    })));
    let mut keys: Vec<String> = std::env::vars().map(|(k, _)| k).collect();
    keys.sort();
    log(&l, json!({"argv": args, "env_keys": keys}));
    let script: Vec<Value> = std::env::var("FAKE_CODEX_SCRIPT")
        .map(|p| {
            std::fs::read_to_string(p)
                .expect("read script")
                .lines()
                .filter(|l| !l.trim().is_empty())
                .map(|l| serde_json::from_str(l).expect("bad script step"))
                .collect()
        })
        .unwrap_or_default();
    let script = Arc::new(Mutex::new(Some(script)));

    if let Some(ms) = std::env::var("FAKE_CODEX_READY_DELAY_MS")
        .ok()
        .and_then(|m| m.parse().ok())
    {
        tokio::time::sleep(Duration::from_millis(ms)).await;
    }
    let listener = TcpListener::bind(&addr).await.expect("bind");
    let port = listener.local_addr().unwrap().port();
    let host = std::env::var("FAKE_CODEX_BANNER_HOST").unwrap_or_else(|_| "127.0.0.1".into());
    eprintln!("codex app-server (WebSockets)");
    eprintln!("  listening on: ws://{host}:{port}");
    eprintln!("  readyz: http://127.0.0.1:{port}/readyz");
    loop {
        let (stream, _) = listener.accept().await.expect("accept");
        tokio::spawn(serve(stream, token.clone(), l.clone(), script.clone()));
    }
}

async fn serve(
    mut stream: TcpStream,
    token: String,
    l: Log,
    script: Arc<Mutex<Option<Vec<Value>>>>,
) {
    let mut head = [0u8; 16];
    let n = stream.peek(&mut head).await.unwrap_or(0);
    if head[..n].starts_with(b"GET /readyz") || head[..n].starts_with(b"GET /healthz") {
        let mut buf = vec![0u8; 4096];
        let _ = stream.read(&mut buf).await;
        let _ = stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await;
        return;
    }
    let want = format!("Bearer {token}");
    let l2 = l.clone();
    let auth = move |req: &Request, resp: Response| -> Result<Response, ErrorResponse> {
        let got = req
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok());
        if got == Some(want.as_str()) {
            Ok(resp)
        } else {
            log(&l2, json!({"auth_rejected": true}));
            let mut r = ErrorResponse::new(Some("unauthorized".into()));
            *r.status_mut() = StatusCode::UNAUTHORIZED;
            Err(r)
        }
    };
    let Ok(ws) = tokio_tungstenite::accept_hdr_async(stream, auth).await else {
        return;
    };
    let Some(steps) = script.lock().unwrap().take() else {
        return;
    };
    run_script(ws, steps, l).await;
}

async fn run_script(ws: tokio_tungstenite::WebSocketStream<TcpStream>, steps: Vec<Value>, l: Log) {
    let (mut tx, mut rx) = ws.split();
    macro_rules! recv {
        () => {
            loop {
                match rx.next().await {
                    Some(Ok(Message::Text(t))) => {
                        let v: Value = serde_json::from_str(t.as_str()).expect("client json");
                        log(&l, json!({"recv": v}));
                        break v;
                    }
                    Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                    _ => {
                        eprintln!("fake-codex: client left mid-script");
                        std::process::exit(4);
                    }
                }
            }
        };
    }
    for step in steps {
        if let Some(method) = step["expect"].as_str() {
            let frame: Value = recv!();
            if frame["method"] != method {
                eprintln!("fake-codex: wanted {method}, got {frame}");
                std::process::exit(3);
            }
            for (ptr, want) in step["check"].as_object().into_iter().flatten() {
                if frame.pointer(ptr) != Some(want) {
                    eprintln!("fake-codex: {method} {ptr} is not {want}: {frame}");
                    std::process::exit(3);
                }
            }
            let reply = if let Some(r) = step.get("respond") {
                Some(json!({"id": frame["id"], "result": r}))
            } else {
                step.get("respond_error")
                    .map(|e| json!({"id": frame["id"], "error": e}))
            };
            if let Some(r) = reply {
                tx.send(Message::Text(r.to_string().into())).await.unwrap();
            }
        } else if step["expect_response"].as_bool() == Some(true) {
            let frame: Value = recv!();
            if frame.get("method").is_some() {
                eprintln!("fake-codex: wanted a response, got {frame}");
                std::process::exit(3);
            }
        } else if let Some(v) = step.get("send") {
            tx.send(Message::Text(v.to_string().into())).await.unwrap();
        } else if let Some(ms) = step["sleep_ms"].as_u64() {
            tokio::time::sleep(Duration::from_millis(ms)).await;
        } else if step["close"].as_bool() == Some(true) {
            let _ = tx.close().await;
            return;
        }
    }
    while let Some(Ok(m)) = rx.next().await {
        if let Message::Text(t) = m
            && let Ok(v) = serde_json::from_str::<Value>(t.as_str())
        {
            log(&l, json!({"recv": v}));
        }
    }
}

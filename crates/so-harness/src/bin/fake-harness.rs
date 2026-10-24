//! Test-only scripted stand-in for a harness (feature `test-support`).
//!
//! `FAKE_HARNESS_SCRIPT=<path>` is a JSONL file of steps, run in order:
//! - `{"expect_stdin_contains": "text"}`: read one stdin line, which must contain `text`
//!   (otherwise exit 3; stdin EOF exits 4)
//! - `{"stdout": <json value or string>}`: print one line (strings verbatim)
//! - `{"stderr": "text"}`: print one stderr line
//! - `{"sleep_ms": n}`
//! - `{"spawn_sleep": secs, "detach": bool}`: spawn a `sleep` grandchild (same process group), log `{"grandchild": pid}`
//! - `{"shell": "cmd"}`: run `sh -c cmd` in the current directory (e.g. to commit in a worktree)
//! - `{"exit": code}`: exit immediately
//!
//! After the last step it drains stdin until EOF and exits 0, like `claude -p`.
//! `FAKE_HARNESS_LOG=<path>` receives JSONL: `{"argv": [...], "env_keys": [...]}`
//! first (keys only, never values), then `{"stdin": "<line>"}` per line read.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::zombie_processes)]

use std::fs::OpenOptions;
use std::io::{BufRead, Write};
use std::os::unix::process::CommandExt;

use serde_json::{Value, json};

fn main() {
    let script = std::fs::read_to_string(
        std::env::var("FAKE_HARNESS_SCRIPT").expect("FAKE_HARNESS_SCRIPT not set"),
    )
    .expect("read script");
    let mut log = std::env::var("FAKE_HARNESS_LOG").ok().map(|p| {
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(p)
            .expect("open log")
    });
    let mut record = |v: Value| {
        if let Some(f) = log.as_mut() {
            // One write(2) per record: the test reads the log while this process runs.
            f.write_all(format!("{v}\n").as_bytes()).expect("write log");
        }
    };
    let mut keys: Vec<String> = std::env::vars().map(|(k, _)| k).collect();
    keys.sort();
    record(json!({"argv": std::env::args().skip(1).collect::<Vec<_>>(), "env_keys": keys}));

    let stdin = std::io::stdin();
    let mut input = stdin.lock().lines();
    let mut out = std::io::stdout();
    for step in script.lines().filter(|l| !l.trim().is_empty()) {
        let step: Value = serde_json::from_str(step).expect("bad script step");
        if let Some(want) = step["expect_stdin_contains"].as_str() {
            let Some(Ok(line)) = input.next() else {
                eprintln!("fake-harness: stdin closed while expecting {want:?}");
                std::process::exit(4);
            };
            record(json!({"stdin": line}));
            if !line.contains(want) {
                eprintln!("fake-harness: stdin {line:?} lacks {want:?}");
                std::process::exit(3);
            }
        } else if let Some(o) = step.get("stdout") {
            match o.as_str() {
                Some(s) => writeln!(out, "{s}"),
                None => writeln!(out, "{o}"),
            }
            .expect("stdout");
            out.flush().expect("flush");
        } else if let Some(e) = step["stderr"].as_str() {
            eprintln!("{e}");
        } else if let Some(ms) = step["sleep_ms"].as_u64() {
            std::thread::sleep(std::time::Duration::from_millis(ms));
        } else if let Some(secs) = step["spawn_sleep"].as_u64() {
            let detach = step["detach"].as_bool().unwrap_or(false);
            let mut cmd = std::process::Command::new("sleep");
            cmd.arg(secs.to_string()).stdin(std::process::Stdio::null());
            if detach {
                // Own process group, inherits stdout/stderr: escapes group kills
                // and keeps the harness pipes open.
                cmd.process_group(0);
            } else {
                cmd.stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null());
            }
            let child = cmd.spawn().expect("spawn sleep");
            record(json!({"grandchild": child.id()}));
        } else if let Some(cmd) = step["shell"].as_str() {
            let status = std::process::Command::new("sh")
                .arg("-c")
                .arg(cmd)
                .stdin(std::process::Stdio::null())
                .status()
                .expect("run shell step");
            record(json!({"shell": cmd, "ok": status.success()}));
        } else if let Some(code) = step["exit"].as_i64() {
            std::process::exit(code as i32);
        }
    }
    for line in input.map_while(Result::ok) {
        record(json!({"stdin": line}));
    }
}

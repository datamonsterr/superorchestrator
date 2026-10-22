//! `doctor`: environment checks as data, rendered as text or JSON.

use std::io::Read;
use std::net::TcpListener;
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

use anyhow::Result;
use serde::Serialize;
use so_accounts::{HostEnv, SignIn, resolve_binary};
use so_core::{AccountFile, Config, Harness, Paths, Platform};
use so_store::Store;

use crate::ctx::Ctx;

const DEFAULT_TIMEOUT_MS: u64 = 5000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum Status {
    Ok,
    Warn,
    Fail,
}

#[derive(Debug, Serialize)]
struct Check {
    name: String,
    status: Status,
    detail: String,
}

#[derive(Default)]
struct Checks(Vec<Check>);

impl Checks {
    fn push(&mut self, status: Status, name: &str, detail: impl Into<String>) {
        self.0.push(Check {
            name: name.to_string(),
            status,
            detail: detail.into(),
        });
    }
    fn ok(&mut self, name: &str, detail: impl Into<String>) {
        self.push(Status::Ok, name, detail);
    }
    fn warn(&mut self, name: &str, detail: impl Into<String>) {
        self.push(Status::Warn, name, detail);
    }
    fn fail(&mut self, name: &str, detail: impl Into<String>) {
        self.push(Status::Fail, name, detail);
    }
    fn failures(&self) -> usize {
        self.0.iter().filter(|c| c.status == Status::Fail).count()
    }
}

/// `--version` timeout; `SUPERORCHESTRATOR_DOCTOR_TIMEOUT_MS` overrides the 5 s default.
fn version_timeout() -> Duration {
    let ms = std::env::var("SUPERORCHESTRATOR_DOCTOR_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|ms| *ms > 0)
        .unwrap_or(DEFAULT_TIMEOUT_MS);
    Duration::from_millis(ms)
}

fn human_duration(d: Duration) -> String {
    let ms = d.as_millis();
    if ms.is_multiple_of(1000) {
        format!("{}s", ms / 1000)
    } else {
        format!("{ms}ms")
    }
}

/// Runs `<bin> --version`, killing it after `timeout`. Returns the first output line.
fn version_of(bin: &std::path::Path, timeout: Duration) -> Result<String, String> {
    let mut child = Command::new(bin)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut out = String::new();
                if let Some(mut so) = child.stdout.take() {
                    let _ = so.read_to_string(&mut out);
                }
                let line = out.lines().next().unwrap_or("").trim().to_string();
                return if status.success() {
                    Ok(line)
                } else {
                    Err(format!("`--version` exited with {status}"))
                };
            }
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "`--version` timed out after {}",
                    human_duration(timeout)
                ));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(e) => return Err(e.to_string()),
        }
    }
}

fn socket_answers(path: &std::path::Path) -> bool {
    std::os::unix::net::UnixStream::connect(path).is_ok()
}

pub fn run(ctx: &Ctx) -> Result<ExitCode> {
    let mut c = Checks::default();
    let host = ctx.host()?;
    let accounts = match AccountFile::list(&ctx.paths) {
        Ok(a) => a,
        Err(e) => {
            c.fail("accounts", format!("accounts do not load: {e}"));
            Vec::new()
        }
    };

    check_binaries(&mut c, &host, &accounts);
    let cfg = check_config(&mut c, &ctx.paths);
    check_db(&mut c, &ctx.paths);
    let supervisor_up = check_socket(&mut c);
    check_ui_port(&mut c, cfg.as_ref(), supervisor_up);
    check_accounts(&mut c, &host, &accounts);

    let failures = c.failures();
    let value = serde_json::json!({ "ok": failures == 0, "checks": c.0 });
    ctx.emit(&value, |w| {
        for check in &c.0 {
            let mark = match check.status {
                Status::Ok => "✓",
                Status::Warn => "!",
                Status::Fail => "✗",
            };
            writeln!(w, "{mark} {}: {}", check.name, check.detail)?;
        }
        if failures == 0 {
            writeln!(w, "\nall checks passed")
        } else {
            writeln!(w, "\n{failures} check(s) failed")
        }
    })?;
    Ok(if failures == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

fn check_binaries(c: &mut Checks, host: &HostEnv, accounts: &[AccountFile]) {
    let timeout = version_timeout();
    for h in [
        Harness::Codex,
        Harness::Agy,
        Harness::Opencode,
        Harness::Claude,
    ] {
        let name = h.as_str();
        let needed = accounts.iter().any(|a| a.enabled && a.harness == h);
        match resolve_binary(name, host.path_var.as_deref()) {
            Ok(path) => match version_of(&path, timeout) {
                Ok(v) => c.ok(name, format!("{} ({v})", path.display())),
                Err(e) if needed => c.fail(name, format!("{} {e}", path.display())),
                Err(e) => c.warn(name, format!("{} {e}", path.display())),
            },
            Err(_) if needed => c.fail(name, "not on PATH, but an enabled account uses it"),
            Err(_) => c.warn(name, "not on PATH (no enabled account needs it)"),
        }
    }
    if host.platform == Platform::Linux {
        let needs_bwrap = accounts
            .iter()
            .any(|a| a.enabled && a.harness == Harness::Agy);
        match resolve_binary("bwrap", host.path_var.as_deref()) {
            Ok(p) => c.ok("bwrap", p.display().to_string()),
            Err(_) if needs_bwrap => {
                c.fail("bwrap", "not on PATH, but an enabled agy account needs it");
            }
            Err(_) => c.warn("bwrap", "not on PATH (needed for agy accounts)"),
        }
    }
}

fn check_config(c: &mut Checks, paths: &Paths) -> Option<Config> {
    if !paths.config_dir.is_dir() {
        c.fail(
            "config dir",
            format!(
                "{} does not exist; run `superorchestrator init`",
                paths.config_dir.display()
            ),
        );
        return None;
    }
    c.ok("config dir", paths.config_dir.display().to_string());
    match Config::load(paths) {
        Ok(cfg) => {
            if paths.config_toml().exists() {
                c.ok("config.toml", "loads");
            } else {
                c.warn(
                    "config.toml",
                    "missing; using defaults (run `superorchestrator init`)",
                );
            }
            Some(cfg)
        }
        Err(e) => {
            c.fail("config.toml", format!("does not load: {e}"));
            None
        }
    }
}

fn check_db(c: &mut Checks, paths: &Paths) {
    let db = paths.state_db();
    if !db.exists() {
        c.warn("state.db", "not created yet (created on first use)");
        return;
    }
    match Store::open(&db) {
        Ok(_) => c.ok("state.db", format!("opens: {}", db.display())),
        Err(e) => c.fail("state.db", format!("does not open: {e}")),
    }
}

fn check_socket(c: &mut Checks) -> bool {
    let path = match Paths::socket_path() {
        Ok(p) => p,
        Err(e) => {
            c.fail("socket", e.to_string());
            return false;
        }
    };
    if socket_answers(&path) {
        c.ok("supervisor", format!("answers on {}", path.display()));
        true
    } else if path.exists() {
        c.warn(
            "supervisor",
            format!("stale socket {} (not answering)", path.display()),
        );
        false
    } else {
        c.warn(
            "supervisor",
            format!("not running (socket {}; starts on demand)", path.display()),
        );
        false
    }
}

fn check_ui_port(c: &mut Checks, cfg: Option<&Config>, supervisor_up: bool) {
    let default = Config::default();
    let ui = &cfg.unwrap_or(&default).ui;
    match TcpListener::bind((ui.bind.as_str(), ui.port)) {
        Ok(_) => c.ok("ui port", format!("{} is free", ui.port)),
        Err(_) if supervisor_up => {
            c.ok(
                "ui port",
                format!("{} is in use (by the supervisor)", ui.port),
            );
        }
        Err(_) => c.warn(
            "ui port",
            format!("{} is in use by another process", ui.port),
        ),
    }
}

fn check_accounts(c: &mut Checks, host: &HostEnv, accounts: &[AccountFile]) {
    for a in accounts {
        let name = format!("account {}", a.name);
        match so_accounts::sign_in_state(a, host) {
            Ok(SignIn::SignedIn) => c.ok(&name, "signed in"),
            Ok(SignIn::SignedOut) => c.warn(&name, "signed out"),
            Ok(SignIn::Unknown) => c.ok(&name, "sign-in state unknown"),
            Err(e) => c.fail(&name, e.to_string()),
        }
    }
    if host.platform == Platform::MacOs {
        let n = accounts
            .iter()
            .filter(|a| a.enabled && a.harness == Harness::Agy)
            .count();
        if n > 1 {
            c.fail(
                "agy accounts",
                format!(
                    "macOS allows one enabled agy account (it uses the real ~/.gemini); {n} are enabled"
                ),
            );
        } else {
            c.ok("agy accounts", "macOS single-account rule holds");
        }
    }
}

//! `doctor`: environment checks with pass, warning and failure lines.

use std::io::Read;
use std::net::TcpListener;
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

use anyhow::Result;
use so_accounts::{HostEnv, SignIn, resolve_binary};
use so_core::{AccountFile, Config, Harness, Paths, Platform};
use so_store::Store;

use crate::ctx::Ctx;

const VERSION_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Default)]
struct Report {
    failures: usize,
}

impl Report {
    fn ok(&self, msg: impl AsRef<str>) {
        println!("✓ {}", msg.as_ref());
    }
    fn warn(&self, msg: impl AsRef<str>) {
        println!("! {}", msg.as_ref());
    }
    fn fail(&mut self, msg: impl AsRef<str>) {
        self.failures += 1;
        println!("✗ {}", msg.as_ref());
    }
}

/// Runs `<bin> --version`, killing it after the timeout. Returns the first output line.
fn version_of(bin: &std::path::Path) -> Result<String, String> {
    let mut child = Command::new(bin)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let deadline = Instant::now() + VERSION_TIMEOUT;
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
                return Err("`--version` timed out after 5s".into());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(e.to_string()),
        }
    }
}

fn socket_answers(path: &std::path::Path) -> bool {
    std::os::unix::net::UnixStream::connect(path).is_ok()
}

pub fn run(ctx: &Ctx) -> Result<ExitCode> {
    let mut r = Report::default();
    let host = ctx.host()?;
    let accounts = AccountFile::list(&ctx.paths).unwrap_or_default();

    check_binaries(&mut r, &host, &accounts);
    let cfg = check_config(&mut r, &ctx.paths);
    check_db(&mut r, &ctx.paths);
    let supervisor_up = check_socket(&mut r);
    check_ui_port(&mut r, cfg.as_ref(), supervisor_up);
    check_accounts(&mut r, ctx, &host, &accounts);

    if r.failures == 0 {
        println!("\nall checks passed");
        Ok(ExitCode::SUCCESS)
    } else {
        println!("\n{} check(s) failed", r.failures);
        Ok(ExitCode::FAILURE)
    }
}

fn check_binaries(r: &mut Report, host: &HostEnv, accounts: &[AccountFile]) {
    for h in [
        Harness::Codex,
        Harness::Agy,
        Harness::Opencode,
        Harness::Claude,
    ] {
        let name = h.as_str();
        let needed = accounts.iter().any(|a| a.enabled && a.harness == h);
        match resolve_binary(name, host.path_var.as_deref()) {
            Ok(path) => match version_of(&path) {
                Ok(v) => r.ok(format!("{name}: {} ({v})", path.display())),
                Err(e) if needed => r.fail(format!("{name}: {} {e}", path.display())),
                Err(e) => r.warn(format!("{name}: {} {e}", path.display())),
            },
            Err(_) if needed => r.fail(format!(
                "{name}: not on PATH, but an enabled account uses it"
            )),
            Err(_) => r.warn(format!("{name}: not on PATH (no enabled account needs it)")),
        }
    }
    if host.platform == Platform::Linux {
        let needs_bwrap = accounts
            .iter()
            .any(|a| a.enabled && a.harness == Harness::Agy);
        match resolve_binary("bwrap", host.path_var.as_deref()) {
            Ok(p) => r.ok(format!("bwrap: {}", p.display())),
            Err(_) if needs_bwrap => {
                r.fail("bwrap: not on PATH, but an enabled agy account needs it")
            }
            Err(_) => r.warn("bwrap: not on PATH (needed for agy accounts)"),
        }
    }
}

fn check_config(r: &mut Report, paths: &Paths) -> Option<Config> {
    if !paths.config_dir.is_dir() {
        r.fail(format!(
            "config dir {} does not exist; run `superorchestrator init`",
            paths.config_dir.display()
        ));
        return None;
    }
    r.ok(format!("config dir: {}", paths.config_dir.display()));
    match Config::load(paths) {
        Ok(c) => {
            if paths.config_toml().exists() {
                r.ok("config.toml loads");
            } else {
                r.warn("config.toml missing; using defaults (run `superorchestrator init`)");
            }
            Some(c)
        }
        Err(e) => {
            r.fail(format!("config.toml does not load: {e}"));
            None
        }
    }
}

fn check_db(r: &mut Report, paths: &Paths) {
    let db = paths.state_db();
    if !db.exists() {
        r.warn("state.db not created yet (created on first use)");
        return;
    }
    match Store::open(&db) {
        Ok(_) => r.ok(format!("state.db opens: {}", db.display())),
        Err(e) => r.fail(format!("state.db does not open: {e}")),
    }
}

fn check_socket(r: &mut Report) -> bool {
    let path = match Paths::socket_path() {
        Ok(p) => p,
        Err(e) => {
            r.fail(format!("socket path: {e}"));
            return false;
        }
    };
    if socket_answers(&path) {
        r.ok(format!("supervisor answers on {}", path.display()));
        true
    } else if path.exists() {
        r.warn(format!(
            "stale socket {} (supervisor not answering)",
            path.display()
        ));
        false
    } else {
        r.warn(format!(
            "supervisor not running (socket {}; starts on demand)",
            path.display()
        ));
        false
    }
}

fn check_ui_port(r: &mut Report, cfg: Option<&Config>, supervisor_up: bool) {
    let default = Config::default();
    let ui = &cfg.unwrap_or(&default).ui;
    match TcpListener::bind((ui.bind.as_str(), ui.port)) {
        Ok(_) => r.ok(format!("UI port {} is free", ui.port)),
        Err(_) if supervisor_up => {
            r.ok(format!("UI port {} is in use (by the supervisor)", ui.port))
        }
        Err(_) => r.warn(format!("UI port {} is in use by another process", ui.port)),
    }
}

fn check_accounts(r: &mut Report, ctx: &Ctx, host: &HostEnv, accounts: &[AccountFile]) {
    if let Err(e) = AccountFile::list(&ctx.paths) {
        r.fail(format!("accounts do not load: {e}"));
        return;
    }
    for a in accounts {
        match so_accounts::sign_in_state(a, host) {
            Ok(SignIn::SignedIn) => r.ok(format!("account {}: signed in", a.name)),
            Ok(SignIn::SignedOut) => r.warn(format!("account {}: signed out", a.name)),
            Ok(SignIn::Unknown) => r.ok(format!("account {}: sign-in state unknown", a.name)),
            Err(e) => r.fail(format!("account {}: {e}", a.name)),
        }
    }
    if host.platform == Platform::MacOs {
        let n = accounts
            .iter()
            .filter(|a| a.enabled && a.harness == Harness::Agy)
            .count();
        if n > 1 {
            r.fail(format!(
                "macOS allows one enabled agy account (it uses the real ~/.gemini); {n} are enabled"
            ));
        } else {
            r.ok("macOS agy single-account rule holds");
        }
    }
}

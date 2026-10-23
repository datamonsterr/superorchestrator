//! Environment checks behind `doctor` and `GET /api/v1/doctor`: binaries, config, database,
//! socket, UI port and sign-ins, as data.

use std::io::Read;
use std::net::TcpListener;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use schemars::JsonSchema;
use serde::Serialize;
use so_accounts::{HostEnv, SignIn, resolve_binary};
use so_core::{AccountFile, Config, Harness, Paths, Platform};
use so_store::Store;

const DEFAULT_TIMEOUT_MS: u64 = 5000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Ok,
    Warn,
    Fail,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Check {
    pub name: String,
    pub status: Status,
    /// False only for `fail`.
    pub ok: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct DoctorReport {
    pub ok: bool,
    pub checks: Vec<Check>,
}

impl DoctorReport {
    pub fn failures(&self) -> usize {
        self.checks
            .iter()
            .filter(|c| c.status == Status::Fail)
            .count()
    }
}

#[derive(Default)]
struct Checks(Vec<Check>);

impl Checks {
    fn push(&mut self, status: Status, name: &str, detail: impl Into<String>) {
        self.0.push(Check {
            name: name.to_string(),
            status,
            ok: status != Status::Fail,
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
}

/// Runs every check. Blocking: it runs `<harness> --version` and probes the UI port, so
/// call it from a blocking context.
pub fn run_checks(paths: &Paths, host: &HostEnv) -> DoctorReport {
    let mut c = Checks::default();
    let accounts = match AccountFile::list(paths) {
        Ok(a) => a,
        Err(e) => {
            c.fail("accounts", format!("accounts do not load: {e}"));
            Vec::new()
        }
    };

    check_binaries(&mut c, host, &accounts);
    let cfg = check_config(&mut c, paths);
    check_decider(&mut c, cfg.as_ref());
    check_levels(
        &mut c,
        cfg.as_ref(),
        so_router::Catalog::load(paths).ok().as_ref(),
    );
    check_pass_env(&mut c, cfg.as_ref(), &accounts, &|n| {
        std::env::var_os(n).is_some()
    });
    check_db(&mut c, paths);
    let supervisor_up = check_socket(&mut c);
    check_ui_port(&mut c, cfg.as_ref(), supervisor_up);
    check_accounts(&mut c, host, &accounts);
    let report = DoctorReport {
        ok: false,
        checks: c.0,
    };
    DoctorReport {
        ok: report.failures() == 0,
        ..report
    }
}

/// The routing decider (it scores complexity; code picks the choice): the models in effect
/// (env > config > default), the fallback and whether a key is present. Environment variables
/// are read from this process, so a supervisor started with a different environment may
/// differ.
fn check_decider(c: &mut Checks, cfg: Option<&Config>) {
    let Some(cfg) = cfg else { return };
    let jev = &cfg.router.jev;
    let models = jev.resolve_models(so_router::jev::process_env);
    let has_key = so_router::jev::resolve_key(jev, so_router::jev::process_env).is_some();
    let policy = match jev.on_unreachable {
        so_core::config::OnUnreachable::Error => "error",
        so_core::config::OnUnreachable::Heuristic => "heuristic",
    };
    let detail = format!(
        "{}; on_unreachable = {policy}; read from this shell's environment",
        models.describe()
    );
    if has_key {
        c.ok("router decider", detail);
    } else {
        c.warn(
            "router decider",
            format!(
                "no OpenRouter key ({} or router.jev.key_file): routing uses the heuristic only; {detail}",
                jev.key_env
            ),
        );
    }
}

/// Complexity levels with nothing to route to for role `worker`: every listed choice is
/// disabled, unknown or not a worker choice, or no enabled worker choice has the level's tier.
/// A level that sets both `tier` and `choices` ignores the tier. Account headroom is not
/// checked: it changes by the minute.
fn check_levels(c: &mut Checks, cfg: Option<&Config>, catalog: Option<&so_router::Catalog>) {
    let Some(cfg) = cfg else { return };
    let usable = |ch: &so_core::types::Choice| ch.enabled && ch.roles.iter().any(|r| r == "worker");
    let mut problems = Vec::new();
    let mut tiers_skipped = false;
    for l in &cfg.router.levels {
        if !l.choices.is_empty() {
            let any = l
                .choices
                .iter()
                .any(|lc| cfg.choices.iter().any(|ch| ch.id == lc.id && usable(ch)));
            if !any {
                problems.push(format!(
                    "level {}: every listed choice is disabled or unknown, so it falls through to the nearest level",
                    l.name
                ));
            }
            if l.tier.is_some() {
                problems.push(format!(
                    "level {}: `tier` is ignored because `choices` is not empty",
                    l.name
                ));
            }
        } else if let Some(tier) = l.tier {
            let Some(catalog) = catalog else {
                tiers_skipped = true;
                continue;
            };
            let want = so_router::levels::tier_of(tier);
            if !cfg
                .choices
                .iter()
                .any(|ch| usable(ch) && catalog.tier(ch.harness, &ch.model) == want)
            {
                problems.push(format!(
                    "level {}: no enabled choice has tier {tier:?}, so it falls through to the nearest level",
                    l.name
                ));
            }
        }
    }
    if tiers_skipped {
        problems.push("tier checks skipped: catalog unavailable".to_string());
    }
    if problems.is_empty() {
        c.ok(
            "levels",
            format!(
                "{} level(s) have a usable choice for role worker",
                cfg.router.levels.len()
            ),
        );
    } else {
        c.warn("levels", format!("{} (role worker)", problems.join("; ")));
    }
}

/// Secret-bearing variables that harnesses commonly need but the child-environment
/// allowlist (`so_core::child_env`) drops unless they are named in `pass_env`. Keep this
/// list to names that allowlist does not inherit.
const COMMON_PASS_ENV: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
    "OPENROUTER_API_KEY",
    "ANTHROPIC_BASE_URL",
    "GEMINI_API_KEY",
    "GOOGLE_API_KEY",
    "NODE_EXTRA_CA_CERTS",
    "SSH_AUTH_SOCK",
    "GH_TOKEN",
    "GITHUB_TOKEN",
];

/// Warns about common secret variables set in the supervisor's environment that no
/// `pass_env` lists, so harness runs will not see them. Prints names only, never values.
fn check_pass_env(
    c: &mut Checks,
    cfg: Option<&Config>,
    accounts: &[AccountFile],
    is_set: &dyn Fn(&str) -> bool,
) {
    let listed = |name: &str| {
        cfg.is_some_and(|cfg| cfg.accounts.pass_env.iter().any(|n| n == name))
            || accounts
                .iter()
                .any(|a| a.pass_env.iter().any(|n| n == name))
    };
    let missing: Vec<&str> = COMMON_PASS_ENV
        .iter()
        .copied()
        .filter(|n| is_set(n) && !listed(n))
        .collect();
    if !missing.is_empty() {
        c.warn(
            "pass_env",
            format!(
                "set in the supervisor environment but not passed to harness runs: {}. \
                 Harness processes start with an allowlisted environment; if a run needs \
                 one, add its name to `accounts.pass_env` in config.toml or to an account's \
                 `pass_env` (see docs/guide/troubleshooting.md)",
                missing.join(", ")
            ),
        );
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
    let mut command = Command::new(bin);
    so_core::child_env::isolate(&mut command, &[]);
    let mut child = command
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
        match resolve_binary("bwrap", host.path_var.as_deref()) {
            Ok(p) => c.ok("bwrap", p.display().to_string()),
            Err(_) => c.warn(
                "bwrap",
                "not on PATH (optional: agy runs are launched without --die-with-parent)",
            ),
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
            check_old_codex_choices(c, &cfg);
            check_choice_efforts(c, &cfg);
            Some(cfg)
        }
        Err(e) => {
            c.fail("config.toml", format!("does not load: {e}"));
            None
        }
    }
}

/// The description older `init` versions gave the codex choices they seeded off.
const OLD_CODEX_SEED_NOTE: &str = "Disabled until a codex account is signed in.";

/// Codex choices that an older `init` seeded as disabled "until a codex account is signed
/// in". Routing now skips a choice without a usable account by itself, so they only need
/// turning on. The config is the user's: this says what to change and never edits it.
fn check_old_codex_choices(c: &mut Checks, cfg: &Config) {
    let stale: Vec<&str> = cfg
        .choices
        .iter()
        .filter(|ch| {
            !ch.enabled
                && ch.harness == so_core::Harness::Codex
                && ch.description.contains(OLD_CODEX_SEED_NOTE)
        })
        .map(|ch| ch.id.as_str())
        .collect();
    if !stale.is_empty() {
        c.warn(
            "choices",
            format!(
                "{} still off from an older init, so `harness: auto` never routes to codex. \
                 Routing skips a choice without a signed-in account by itself: set \
                 `enabled = true` on {} in config.toml",
                stale.join(", "),
                if stale.len() == 1 { "it" } else { "each" }
            ),
        );
    }
}

/// Enabled choices whose effort their harness CLI would refuse. Routing skips them, so they
/// never run; the fix is in config.toml, which this never edits.
fn check_choice_efforts(c: &mut Checks, cfg: &Config) {
    let bad: Vec<String> = cfg
        .choices
        .iter()
        .filter(|ch| ch.enabled)
        .filter_map(|ch| {
            ch.effort_problem()
                .map(|p| format!("choice {}: {p}", ch.id))
        })
        .collect();
    if !bad.is_empty() {
        c.warn(
            "choice efforts",
            format!(
                "{}. Routing skips these choices; change `effort` in config.toml to a listed value",
                bad.join("; ")
            ),
        );
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

#[cfg(test)]
mod tests {
    use super::*;

    fn account(pass_env: &[&str]) -> AccountFile {
        let mut a =
            AccountFile::new("codex-acc2", Harness::Codex, "~/.codex-acc2", "codex").unwrap();
        a.pass_env = pass_env.iter().map(|n| n.to_string()).collect();
        a
    }

    fn detail(c: &Checks) -> Option<&str> {
        c.0.iter()
            .find(|x| x.name == "pass_env" && x.status == Status::Warn)
            .map(|x| x.detail.as_str())
    }

    #[test]
    fn warns_for_unlisted_secret_names_without_values() {
        let set = |n: &str| matches!(n, "OPENAI_API_KEY" | "GH_TOKEN" | "SSH_AUTH_SOCK" | "HOME");
        let mut cfg = Config::default();
        cfg.accounts.pass_env = vec!["GH_TOKEN".into()];
        let accounts = [account(&["SSH_AUTH_SOCK"])];
        let mut c = Checks::default();
        check_pass_env(&mut c, Some(&cfg), &accounts, &set);
        let d = detail(&c).expect("warning");
        let names = d
            .split("runs: ")
            .nth(1)
            .and_then(|r| r.split(". ").next())
            .unwrap();
        assert_eq!(names, "OPENAI_API_KEY", "{d}");
    }

    fn levels_detail(cfg: &Config) -> (Status, String) {
        let mut c = Checks::default();
        check_levels(&mut c, Some(cfg), None);
        let x = c.0.into_iter().find(|x| x.name == "levels").unwrap();
        (x.status, x.detail)
    }

    #[test]
    fn levels_warn_for_disabled_choices_and_ignored_tier() {
        let mut cfg = Config::default();
        let id = "x".to_string();
        cfg.choices.push(so_core::types::Choice {
            id: id.clone(),
            harness: Harness::Agy,
            model: "m".into(),
            effort: None,
            roles: vec!["worker".to_string()],
            description: String::new(),
            enabled: false,
        });
        let mut l = cfg.router.levels[0].clone();
        l.choices = vec![so_core::config::LevelChoice {
            id: id.clone(),
            priority: None,
        }];
        l.tier = Some(so_core::config::LevelTier::Fast);
        cfg.router.levels = vec![l];
        let (st, d) = levels_detail(&cfg);
        assert_eq!(st, Status::Warn);
        assert!(d.contains("disabled or unknown"), "{d}");
        assert!(d.contains("`tier` is ignored"), "{d}");
        cfg.choices[0].enabled = true;
        let (st, d) = levels_detail(&cfg);
        assert_eq!(st, Status::Warn);
        assert!(!d.contains("disabled or unknown"), "{d}");
    }

    #[test]
    fn levels_without_a_catalog_skip_tier_checks_with_a_warning() {
        let (st, d) = levels_detail(&Config::default());
        assert_eq!(st, Status::Warn);
        assert!(
            d.contains("tier checks skipped: catalog unavailable"),
            "{d}"
        );
    }

    #[test]
    fn silent_when_everything_set_is_listed_or_nothing_is_set() {
        let mut c = Checks::default();
        check_pass_env(&mut c, None, &[], &|_| false);
        assert!(c.0.is_empty());
        let accounts = [account(&["GITHUB_TOKEN"])];
        check_pass_env(&mut c, None, &accounts, &|n| n == "GITHUB_TOKEN");
        assert!(c.0.is_empty());
    }
}

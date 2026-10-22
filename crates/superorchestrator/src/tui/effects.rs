//! Work done off the draw loop. Each function calls the same library code the CLI
//! commands use and reports through [`Update`]; none of it prints.

use std::sync::mpsc::{Receiver, Sender};

use anyhow::{Context, Result, anyhow};
use chrono::{Duration, Utc};
use serde::Serialize;
use serde::de::DeserializeOwned;
use so_accounts::{HostEnv, manage};
use so_core::proto::{
    AcceptParams, AcceptResult, AckResult, GetRunParams, JoinResult, METHOD_ACCEPT_RUN,
    METHOD_CANCEL_AGENT, METHOD_GET_RUN, METHOD_JOIN_RUN, METHOD_REJECT_RUN, METHOD_STATUS,
    RejectParams, RunRef, RunView, StatusResult,
};
use so_core::{AccountFile, Harness, Paths};
use so_store::Store;
use so_supervisor::SupervisorClient;
use so_supervisor::services::accounts::{FlagAction, apply_flag, flag_pools};

use super::app::{Effect, Launch, SetupStep, StepState, Update};
use super::data::{SupInfo, load_detail, load_snapshot};
use crate::cmd::account::{login_json, write_login_hint};
use crate::cmd::{install, runs, setup};
use crate::ctx::Ctx;

/// Shown when an action needs the supervisor and none answers.
pub const NOT_RUNNING: &str =
    "supervisor not running — start with any delegation or `superorchestrator daemon`";

fn send(tx: &Sender<Update>, u: Update) {
    // The receiver is gone when the UI is quitting; nothing to report to.
    let _ = tx.send(u);
}

/// Runs `f`, turning a panic into its message. The panic hook stays quiet on worker
/// threads (see `mod.rs`), so nothing is written over the screen.
pub fn guarded<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).map_err(|p| {
        let msg = p
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| p.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic".to_string());
        format!("internal error: {msg}")
    })
}

/// How long a supervisor call (connect, hello and answer together) may take on the reader.
const SUP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// A long-lived connection to the supervisor for the reader thread: one runtime, one client
/// reused between reads, every call bounded by a timeout. It never starts the supervisor.
pub struct Link {
    rt: tokio::runtime::Runtime,
    socket: Option<std::path::PathBuf>,
    client: Option<SupervisorClient>,
    timeout: std::time::Duration,
}

enum Fail {
    Down,
    Rpc(String),
    Other(String),
}

async fn attempt<P: Serialize, R: DeserializeOwned>(
    client: &mut Option<SupervisorClient>,
    socket: &std::path::Path,
    method: &str,
    params: P,
) -> Result<R, Fail> {
    if client.is_none() {
        let c = SupervisorClient::connect(socket)
            .await
            .map_err(|_| Fail::Down)?;
        let cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        c.hello_inspect("tui", &cwd, Some(std::process::id()))
            .await
            .map_err(|_| Fail::Down)?;
        *client = Some(c);
    }
    let Some(c) = client.as_ref() else {
        return Err(Fail::Down);
    };
    match c.call(method, params).await {
        Ok(v) => Ok(v),
        Err(so_supervisor::ClientError::Rpc(r)) => Err(Fail::Rpc(r.message)),
        Err(e) => {
            // The connection is suspect: reconnect next time.
            *client = None;
            Err(Fail::Other(e.to_string()))
        }
    }
}

impl Link {
    pub fn new(socket: Option<std::path::PathBuf>, timeout: std::time::Duration) -> Option<Self> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .ok()?;
        Some(Self {
            rt,
            socket,
            client: None,
            timeout,
        })
    }

    pub fn call<P: Serialize, R: DeserializeOwned>(
        &mut self,
        method: &str,
        params: P,
    ) -> Result<R, String> {
        let socket = self.socket.clone().ok_or(NOT_RUNNING)?;
        let mut slot = self.client.take();
        let timeout = self.timeout;
        let timed = self.rt.block_on(async {
            tokio::time::timeout(timeout, attempt(&mut slot, &socket, method, params)).await
        });
        match timed {
            Ok(r) => {
                self.client = slot;
                r.map_err(|f| match f {
                    Fail::Down => NOT_RUNNING.to_string(),
                    Fail::Rpc(m) | Fail::Other(m) => m,
                })
            }
            // Dropping `slot` closes a connection that did not answer.
            Err(_) => Err(format!(
                "the supervisor did not answer within {} s",
                self.timeout.as_secs().max(1)
            )),
        }
    }
}

/// Reads: the snapshot and the open run's detail. Never blocks the draw loop, and never
/// waits on the supervisor for longer than [`SUP_TIMEOUT`].
pub fn read_loop(ctx: Ctx, host: HostEnv, rx: Receiver<Effect>, tx: Sender<Update>) {
    let Some(mut link) = Link::new(Paths::socket_path().ok(), SUP_TIMEOUT) else {
        send(&tx, Update::ReadFailed("could not start the reader".into()));
        return;
    };
    while let Ok(effect) = rx.recv() {
        let done = guarded(|| read_one(&ctx, &host, &mut link, effect, &tx));
        if let Err(e) = done {
            // A panic must not leave the loop waiting for an answer that never comes.
            send(&tx, Update::ReadFailed(e));
        }
    }
}

fn read_one(ctx: &Ctx, host: &HostEnv, link: &mut Link, effect: Effect, tx: &Sender<Update>) {
    match effect {
        Effect::Refresh => {
            let sup = link
                .call::<_, StatusResult>(METHOD_STATUS, serde_json::json!({}))
                .ok()
                .map(|s| SupInfo {
                    pid: s.pid,
                    version: s.version,
                    uptime_s: s.uptime_s,
                });
            let snap = load_snapshot(ctx, host, sup, Utc::now());
            send(tx, Update::Snapshot(Box::new(snap)));
        }
        Effect::Detail { id, diff } => {
            let d = load_detail(ctx, &id).map(|mut d| {
                // The diff comes from the supervisor (it owns the worktree's git calls).
                if diff && d.state == so_core::RunState::PendingReview && d.worktree.is_some() {
                    d.diff = Some(fetch_diff(link, &id));
                }
                Box::new(d)
            });
            send(tx, Update::Detail(d));
        }
        _ => {}
    }
}

/// Actions, one at a time in the order they were asked.
pub fn action_loop(ctx: Ctx, host: HostEnv, rx: Receiver<Effect>, tx: Sender<Update>) {
    while let Ok(effect) = rx.recv() {
        let done = match guarded(|| perform(&ctx, &host, effect, &tx)) {
            Ok(Ok(Some(text))) => Update::Done { ok: true, text },
            Ok(Ok(None)) => continue,
            Ok(Err(e)) => Update::Done {
                ok: false,
                text: format!("{e:#}"),
            },
            Err(text) => Update::Done { ok: false, text },
        };
        send(&tx, done);
    }
}

/// `Ok(Some(text))` is a success message, `Ok(None)` means the effect already sent its own
/// final update.
fn perform(
    ctx: &Ctx,
    host: &HostEnv,
    effect: Effect,
    tx: &Sender<Update>,
) -> Result<Option<String>> {
    let skill_warnings = |text: String| -> String {
        match ctx.refresh_skill_errors().first() {
            Some(w) => format!("{text} (warning: {w})"),
            None => text,
        }
    };
    Ok(Some(match effect {
        Effect::Refresh | Effect::Detail { .. } => return Ok(None),
        Effect::AddAccounts { harness, count } => {
            let cfg = ctx.config()?;
            let batch = manage::add_numbered(&ctx.paths, &cfg, host, harness, count);
            let names: Vec<_> = batch.created.iter().map(|a| a.name.as_str()).collect();
            if names.is_empty() {
                return Err(batch
                    .error
                    .map_or_else(|| anyhow!("no account was created"), Into::into));
            }
            let text = skill_warnings(format!("added {}; press l to sign in", names.join(", ")));
            if harness == Harness::Claude {
                send(
                    tx,
                    Update::Hint {
                        title: "Claude accounts".into(),
                        lines: vec![setup::CLAUDE_NOTE.to_string()],
                    },
                );
            }
            if let Some(e) = batch.error {
                return Err(anyhow!("{text}; then: {e}"));
            }
            text
        }
        Effect::Login(name) => {
            let account = load_account(&ctx.paths, &name)?;
            // Fails early when the harness binary is missing.
            let plan = manage::login_command(&account, host, true)?;
            let (program, args) = plan.argv.split_first().context("empty login command")?;
            let via_cli = matches!(account.harness, Harness::Codex | Harness::Opencode);
            // claude and agy sign in inside a session of their wrapper.
            let wrapper_ok =
                via_cli || so_accounts::resolve_binary(program, host.path_var.as_deref()).is_ok();
            if wrapper_ok {
                let note = match account.harness {
                    Harness::Claude => {
                        Some("type /login inside the session, then exit to come back".to_string())
                    }
                    Harness::Agy => Some("sign in, then exit to come back".to_string()),
                    _ => plan.note.clone(),
                };
                send(
                    tx,
                    Update::Launch(Launch {
                        title: format!("Sign in to {name}"),
                        note,
                        program: program.clone(),
                        args: args.to_vec(),
                        env: plan.env.clone(),
                        cwd: None,
                    }),
                );
            } else {
                // The wrapper is not on PATH: show the command instead.
                let login = login_json(&account, host)?;
                let mut buf = Vec::new();
                write_login_hint(&mut buf, &login, false)?;
                let mut lines: Vec<String> = String::from_utf8_lossy(&buf)
                    .lines()
                    .map(str::to_string)
                    .collect();
                lines.insert(0, format!("The wrapper `{program}` is not on PATH."));
                send(
                    tx,
                    Update::Hint {
                        title: format!("Sign in to {name}"),
                        lines,
                    },
                );
            }
            return Ok(None);
        }
        Effect::SetEnabled { name, enabled } => {
            manage::set_enabled(&ctx.paths, host, &name, enabled)?;
            skill_warnings(format!(
                "{} {name}",
                if enabled { "enabled" } else { "disabled" }
            ))
        }
        Effect::Cooldown { name, hours } => {
            let account = load_account(&ctx.paths, &name)?;
            let pools = flag_pools(&account, None);
            let store = Store::open(ctx.paths.state_db()).context("could not open state.db")?;
            match hours {
                None => {
                    apply_flag(&store, &name, &pools, FlagAction::Clear, None)?;
                    format!("cleared the cooldown on {name}")
                }
                Some(h) => {
                    let until = Utc::now() + Duration::seconds((h * 3600.0) as i64);
                    apply_flag(&store, &name, &pools, FlagAction::Until(until), None)?;
                    format!("{name} in cooldown for {h} h")
                }
            }
        }
        Effect::Remove(name) => {
            let cfg = ctx.config()?;
            manage::remove(&ctx.paths, &cfg, host, &name, false)?;
            skill_warnings(format!("removed account {name}"))
        }
        Effect::ScanImport => {
            send(tx, Update::Candidates(setup::scan_candidates(ctx, host)?));
            return Ok(None);
        }
        Effect::ImportAll(found) => {
            let cfg = ctx.config()?;
            let (names, failed) = import_all(ctx, &cfg, host, &found);
            if !failed.is_empty() {
                return Err(anyhow!(
                    "imported {}; could not import {}",
                    names.len(),
                    failed.join("; ")
                ));
            }
            skill_warnings(format!("imported {}", names.join(", ")))
        }
        Effect::Cancel(run) => {
            let v: RunView = supervisor_call(METHOD_CANCEL_AGENT, RunRef { run: run.clone() })?;
            format!("run {} is {}", short(&v.run_id), v.state)
        }
        Effect::Accept { run, merge } => {
            let r: AcceptResult = supervisor_call(
                METHOD_ACCEPT_RUN,
                AcceptParams {
                    run: run.clone(),
                    merge,
                },
            )?;
            accept_text(&short(&run), &r)
        }
        Effect::Reject { run, feedback } => {
            let _: AckResult = supervisor_call(
                METHOD_REJECT_RUN,
                RejectParams {
                    run: run.clone(),
                    feedback,
                },
            )?;
            format!("sent feedback; run {} continues", short(&run))
        }
        Effect::Join(run) => {
            let j: JoinResult = supervisor_call(METHOD_JOIN_RUN, RunRef { run: run.clone() })?;
            let (program, args) = j.argv.split_first().context("empty join command")?;
            send(
                tx,
                Update::Launch(Launch {
                    title: format!("Joining run {}", short(&run)),
                    note: None,
                    program: program.clone(),
                    args: args.to_vec(),
                    env: j.env.clone(),
                    cwd: Some(j.cwd.clone()),
                }),
            );
            return Ok(None);
        }
        Effect::Setup(step) => {
            let (state, _) = run_step(ctx, host, step, tx);
            format!("{}: {}", step.label(), state_word(state))
        }
        Effect::SetupAll => {
            for step in SetupStep::ALL {
                let (state, _) = run_step(ctx, host, step, tx);
                if state == StepState::Failed {
                    return Err(anyhow!("{} failed; later steps were not run", step.label()));
                }
            }
            "setup finished".to_string()
        }
    }))
}

/// The toast for an accepted run.
fn accept_text(run: &str, r: &AcceptResult) -> String {
    if r.merged {
        return format!("accepted run {run} and merged its branch");
    }
    match (&r.merge_command, &r.note) {
        (Some(c), _) => format!("accepted run {run}; merge it with: {c}"),
        (None, Some(n)) => format!("accepted run {run}; {n}"),
        _ => format!("accepted run {run}"),
    }
}

/// `get_run` with the diff, without starting the supervisor.
fn fetch_diff(link: &mut Link, id: &str) -> Result<so_core::proto::RunDiff, String> {
    let v: RunView = link.call(
        METHOD_GET_RUN,
        GetRunParams {
            run: id.to_string(),
            diff: true,
        },
    )?;
    match v.diff {
        Some(d) => match d.error.clone() {
            Some(e) => Err(e),
            None => Ok(d),
        },
        None => Err("the supervisor returned no diff".to_string()),
    }
}

fn state_word(s: StepState) -> &'static str {
    match s {
        StepState::Done => "done",
        StepState::Warn => "done, needs attention",
        StepState::Failed => "failed",
        StepState::Pending | StepState::Running => "not finished",
    }
}

fn short(id: &str) -> String {
    id.chars().take(8).collect()
}

fn load_account(paths: &Paths, name: &str) -> Result<AccountFile> {
    let file = paths.account_file(name);
    if !file.exists() {
        return Err(anyhow!("account `{name}` not found"));
    }
    Ok(AccountFile::load(&file)?)
}

fn import_all(
    ctx: &Ctx,
    cfg: &so_core::Config,
    host: &HostEnv,
    found: &[so_accounts::ImportCandidate],
) -> (Vec<String>, Vec<String>) {
    let (mut names, mut failed) = (Vec::new(), Vec::new());
    for c in found {
        let home = host.tildify(&c.home);
        match manage::import(&ctx.paths, cfg, host, &c.name, c.harness, &home) {
            Ok(a) => names.push(a.name),
            Err(e) => failed.push(format!("{}: {e}", c.name)),
        }
    }
    (names, failed)
}

/// Runs one blocking call against the supervisor; never starts it.
fn supervisor_call<P: Serialize, R: DeserializeOwned>(method: &str, params: P) -> Result<R> {
    let socket = Paths::socket_path()?;
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    runs::runtime()?.block_on(async {
        // Connecting and the hello are quick or the supervisor is not there; the call itself
        // may take a while (accept runs git).
        let client = tokio::time::timeout(SUP_TIMEOUT, async {
            let client = SupervisorClient::connect(&socket).await.ok()?;
            client
                .hello_inspect("tui", &cwd, Some(std::process::id()))
                .await
                .ok()?;
            Some(client)
        })
        .await
        .ok()
        .flatten()
        .ok_or_else(|| anyhow!(NOT_RUNNING))?;
        client
            .call_timeout(method, params, std::time::Duration::from_secs(60))
            .await
            .map_err(runs::rpc)
    })
}

// ------------------------------------------------------------------ setup steps

fn run_step(
    ctx: &Ctx,
    host: &HostEnv,
    step: SetupStep,
    tx: &Sender<Update>,
) -> (StepState, Vec<String>) {
    send(
        tx,
        Update::Step {
            step,
            state: StepState::Running,
            lines: Vec::new(),
        },
    );
    let (state, lines) = match guarded(|| step_body(ctx, host, step)) {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => (StepState::Failed, vec![format!("{e:#}")]),
        Err(e) => (StepState::Failed, vec![e]),
    };
    send(
        tx,
        Update::Step {
            step,
            state,
            lines: lines.clone(),
        },
    );
    (state, lines)
}

fn step_body(ctx: &Ctx, host: &HostEnv, step: SetupStep) -> Result<(StepState, Vec<String>)> {
    Ok(match step {
        SetupStep::Init => {
            let created = setup::step_init(ctx)?;
            let line = if created > 0 {
                format!(
                    "created {created} item(s) in {}",
                    host.tildify(&ctx.paths.config_dir)
                )
            } else {
                format!("{} already set up", host.tildify(&ctx.paths.config_dir))
            };
            (StepState::Done, vec![line])
        }
        SetupStep::Harnesses => {
            let infos = setup::detect_harnesses(ctx, host, &[])?;
            let lines = infos
                .iter()
                .map(|i| match &i.binary {
                    Some(p) => format!(
                        "{:<9} {} ({} account(s))",
                        i.harness,
                        p.display(),
                        i.accounts.len()
                    ),
                    None => format!("{:<9} not installed", i.harness),
                })
                .collect();
            let any = infos.iter().any(|i| i.binary.is_some());
            (
                if any {
                    StepState::Done
                } else {
                    StepState::Failed
                },
                lines,
            )
        }
        SetupStep::Import => {
            let found = setup::scan_candidates(ctx, host)?;
            if found.is_empty() {
                return Ok((StepState::Done, vec!["no existing logins found".into()]));
            }
            let cfg = ctx.config()?;
            let (names, failed) = import_all(ctx, &cfg, host, &found);
            let mut lines: Vec<String> = names.iter().map(|n| format!("imported {n}")).collect();
            lines.extend(failed.iter().map(|f| format!("could not import {f}")));
            ctx.refresh_skill_errors();
            (
                if failed.is_empty() {
                    StepState::Done
                } else {
                    StepState::Failed
                },
                lines,
            )
        }
        SetupStep::AddAccounts => {
            let cfg = ctx.config()?;
            let infos = setup::detect_harnesses(ctx, host, &[])?;
            let mut lines = Vec::new();
            let mut failed = false;
            for i in &infos {
                let want = setup::default_count(i);
                if want == 0 {
                    continue;
                }
                let batch = manage::add_numbered(&ctx.paths, &cfg, host, i.harness, want);
                lines.extend(batch.created.iter().map(|a| format!("added {}", a.name)));
                if let Some(e) = batch.error {
                    lines.push(format!("could not add every {} account: {e}", i.harness));
                    failed = true;
                }
            }
            if lines.is_empty() {
                lines.push("nothing to add: every installed harness already has an account".into());
            }
            ctx.refresh_skill_errors();
            (
                if failed {
                    StepState::Failed
                } else {
                    StepState::Done
                },
                lines,
            )
        }
        SetupStep::SignIn => {
            let pending = setup::signed_out(ctx, host)?;
            if pending.is_empty() {
                return Ok((StepState::Done, vec!["no account is signed out".into()]));
            }
            let mut lines = vec![format!(
                "{} account(s) signed out. Accounts tab (2), select one, press l:",
                pending.len()
            )];
            for a in &pending {
                let login = login_json(a, host)?;
                let via_cli = matches!(a.harness, Harness::Codex | Harness::Opencode);
                let key = if via_cli { "cli" } else { "command" };
                lines.push(format!(
                    "{}: {}",
                    a.name,
                    login[key].as_str().unwrap_or_default()
                ));
            }
            (StepState::Warn, lines)
        }
        SetupStep::Install => {
            let v = install::install_quiet(ctx, false, true)?;
            let lines = v
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|o| {
                            let g = |k: &str| o[k].as_str().unwrap_or_default();
                            format!("{:<9} {}", g("action"), g("target"))
                        })
                        .collect()
                })
                .unwrap_or_default();
            (StepState::Done, lines)
        }
        SetupStep::Doctor => {
            let d = setup::doctor_summary(ctx, host);
            let mut lines = vec![format!(
                "{} ok, {} warning(s), {} failure(s)",
                d.ok, d.warn, d.fail
            )];
            lines.extend(d.lines);
            let state = if d.fail > 0 {
                StepState::Failed
            } else if d.warn > 0 {
                StepState::Warn
            } else {
                StepState::Done
            };
            (state, lines)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    fn fake_host(dir: &std::path::Path) -> HostEnv {
        HostEnv {
            home: dir.join("home"),
            path_var: Some(dir.join("bin").display().to_string()),
            platform: so_core::Platform::Linux,
        }
    }

    fn ctx(dir: &std::path::Path) -> Ctx {
        Ctx::new(Some(dir.join("cfg")), false, true).unwrap()
    }

    #[test]
    fn init_step_creates_the_config_then_reports_it_present() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("home")).unwrap();
        let (ctx, host) = (ctx(d.path()), fake_host(d.path()));
        let (tx, rx) = channel();
        let (s, lines) = run_step(&ctx, &host, SetupStep::Init, &tx);
        assert_eq!(s, StepState::Done);
        assert!(lines[0].starts_with("created"), "{lines:?}");
        assert!(ctx.paths.config_toml().exists());
        let (_, again) = run_step(&ctx, &host, SetupStep::Init, &tx);
        assert!(again[0].ends_with("already set up"), "{again:?}");
        // Each step reports running, then its result.
        let states: Vec<_> = rx
            .try_iter()
            .map(|u| match u {
                Update::Step { state, .. } => state,
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(
            states,
            vec![
                StepState::Running,
                StepState::Done,
                StepState::Running,
                StepState::Done
            ]
        );
    }

    #[test]
    fn harness_step_fails_when_no_binary_is_on_path() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("bin")).unwrap();
        let (ctx, host) = (ctx(d.path()), fake_host(d.path()));
        let (tx, _rx) = channel();
        let (s, lines) = run_step(&ctx, &host, SetupStep::Harnesses, &tx);
        assert_eq!(s, StepState::Failed);
        assert!(
            lines.iter().all(|l| l.contains("not installed")),
            "{lines:?}"
        );
    }

    #[test]
    fn actions_on_a_missing_account_report_an_error_not_a_panic() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("home")).unwrap();
        let (ctx, host) = (ctx(d.path()), fake_host(d.path()));
        let (tx, _rx) = channel();
        let e = perform(&ctx, &host, Effect::Login("nope".into()), &tx).unwrap_err();
        assert!(e.to_string().contains("not found"), "{e}");
    }

    #[test]
    fn accept_text_names_the_merge_outcome() {
        let r = AcceptResult {
            ok: true,
            merged: true,
            branch: None,
            merge_command: None,
            note: None,
        };
        assert_eq!(
            accept_text("ab", &r),
            "accepted run ab and merged its branch"
        );
        let r = AcceptResult {
            merged: false,
            merge_command: Some("git merge so/x".into()),
            ..r
        };
        assert_eq!(
            accept_text("ab", &r),
            "accepted run ab; merge it with: git merge so/x"
        );
    }

    fn claude_account(d: &std::path::Path, name: &str) {
        std::fs::create_dir_all(d.join("cfg/accounts")).unwrap();
        std::fs::write(
            d.join(format!("cfg/accounts/{name}.toml")),
            format!(
                "name = \"{name}\"\nharness = \"claude\"\nhome = \"{}\"\nbinary = \"{name}\"\n",
                d.join("home/.claude-1").display()
            ),
        )
        .unwrap();
    }

    #[test]
    fn claude_sign_in_runs_the_wrapper_when_it_exists() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("bin")).unwrap();
        std::fs::create_dir_all(d.path().join("home")).unwrap();
        let w = d.path().join("bin/claude-1");
        std::fs::write(&w, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&w, std::fs::Permissions::from_mode(0o755)).unwrap();
        claude_account(d.path(), "claude-1");
        let (ctx, host) = (ctx(d.path()), fake_host(d.path()));
        let (tx, rx) = channel();
        perform(&ctx, &host, Effect::Login("claude-1".into()), &tx).unwrap();
        match rx.recv().unwrap() {
            Update::Launch(l) => {
                assert_eq!(l.program, "claude-1");
                assert!(l.note.unwrap().contains("/login"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn claude_sign_in_shows_the_command_when_the_wrapper_is_missing() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("bin")).unwrap();
        std::fs::create_dir_all(d.path().join("home")).unwrap();
        claude_account(d.path(), "claude-1");
        let (ctx, host) = (ctx(d.path()), fake_host(d.path()));
        let (tx, rx) = channel();
        perform(&ctx, &host, Effect::Login("claude-1".into()), &tx).unwrap();
        match rx.recv().unwrap() {
            Update::Hint { lines, .. } => assert!(lines[0].contains("not on PATH"), "{lines:?}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn panics_are_caught_with_their_message() {
        assert_eq!(guarded(|| 7), Ok(7));
        let e = guarded(|| -> () { panic!("static message") }).unwrap_err();
        assert_eq!(e, "internal error: static message");
        let n = 3;
        let e = guarded(|| -> () { panic!("formatted {n}") }).unwrap_err();
        assert_eq!(e, "internal error: formatted 3");
    }

    #[test]
    fn link_says_not_running_for_a_missing_socket_and_when_unset() {
        let d = tempfile::tempdir().unwrap();
        let mut l = Link::new(
            Some(d.path().join("none.sock")),
            std::time::Duration::from_secs(1),
        )
        .unwrap();
        let e = l
            .call::<_, serde_json::Value>("status", serde_json::json!({}))
            .unwrap_err();
        assert!(e.contains("not running"), "{e}");
        let mut l = Link::new(None, std::time::Duration::from_secs(1)).unwrap();
        assert!(
            l.call::<_, serde_json::Value>("status", serde_json::json!({}))
                .is_err()
        );
    }

    #[test]
    fn link_gives_up_on_a_supervisor_that_never_answers() {
        let d = tempfile::tempdir().unwrap();
        let sock = d.path().join("hang.sock");
        let listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        // Accepts and then says nothing.
        let t = std::thread::spawn(move || {
            let (s, _) = listener.accept().unwrap();
            std::thread::sleep(std::time::Duration::from_millis(1500));
            drop(s);
        });
        let mut l = Link::new(Some(sock), std::time::Duration::from_millis(300)).unwrap();
        let start = std::time::Instant::now();
        let e = l
            .call::<_, serde_json::Value>("status", serde_json::json!({}))
            .unwrap_err();
        assert!(
            start.elapsed() < std::time::Duration::from_secs(1),
            "{:?}",
            start.elapsed()
        );
        assert!(e.contains("did not answer"), "{e}");
        t.join().unwrap();
    }

    #[test]
    fn the_action_loop_turns_errors_into_a_done_update() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("home")).unwrap();
        let (ctx, host) = (ctx(d.path()), fake_host(d.path()));
        let (etx, erx) = channel();
        let (utx, urx) = channel();
        etx.send(Effect::Remove("ghost".into())).unwrap();
        drop(etx);
        action_loop(ctx, host, erx, utx);
        match urx.recv().unwrap() {
            Update::Done { ok, text } => {
                assert!(!ok);
                assert!(!text.is_empty());
            }
            other => panic!("{other:?}"),
        }
    }
}

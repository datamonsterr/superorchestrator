//! `setup`: the first command a new user runs. Idempotent.
//!
//! The work is split into steps the CLI and the terminal UI drive: `step_init`,
//! `detect_harnesses`, `scan_candidates`, `default_count`, `signed_out`, `doctor_summary`.
//! `run` drives them with prompts.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Result;
use dialoguer::{Confirm, Input};
use serde_json::{Value, json};
use so_accounts::{HostEnv, ImportCandidate, SignIn, manage, resolve_binary, sign_in_state};
use so_core::{AccountFile, Harness};
use so_supervisor::doctor::{Status, run_checks};

use crate::cli::SetupArgs;
use crate::cmd::account::{ask_and_import, login_json, run_login, write_login_hint};
use crate::cmd::init;
use crate::ctx::{Ctx, interactive};
use crate::exit::CliError;

/// Harness order used for listing and prompting.
const ORDER: [Harness; 4] = [
    Harness::Codex,
    Harness::Claude,
    Harness::Opencode,
    Harness::Agy,
];

/// One harness as `setup` sees it.
#[derive(Debug, Clone)]
pub(crate) struct HarnessInfo {
    pub harness: Harness,
    /// The binary on PATH, when installed.
    pub binary: Option<PathBuf>,
    /// Names of the accounts that already exist.
    pub accounts: Vec<String>,
    /// Importable logins found by the scan for this harness.
    pub importable: usize,
}

/// Creates what `init` creates. Returns how many items were created (0 when all existed).
pub(crate) fn step_init(ctx: &Ctx) -> Result<usize> {
    let report = init::init(&ctx.paths)?;
    Ok(report
        .iter()
        .filter(|(_, o)| *o == init::Outcome::Created)
        .count())
}

/// Finds the harness binaries on PATH and the accounts that exist. Writes nothing.
pub(crate) fn detect_harnesses(
    ctx: &Ctx,
    host: &HostEnv,
    candidates: &[ImportCandidate],
) -> Result<Vec<HarnessInfo>> {
    let accounts = AccountFile::list(&ctx.paths)?;
    Ok(ORDER
        .iter()
        .map(|&harness| HarnessInfo {
            harness,
            binary: resolve_binary(harness.as_str(), host.path_var.as_deref()).ok(),
            accounts: accounts
                .iter()
                .filter(|a| a.harness == harness)
                .map(|a| a.name.clone())
                .collect(),
            importable: candidates.iter().filter(|c| c.harness == harness).count(),
        })
        .collect())
}

/// Logins found on disk that are not accounts yet.
pub(crate) fn scan_candidates(ctx: &Ctx, host: &HostEnv) -> Result<Vec<ImportCandidate>> {
    Ok(so_accounts::scan_new(&ctx.paths, host)?)
}

/// How many accounts to add by default: one when the harness is installed and has no
/// account (or importable login) yet. Claude defaults to none: its accounts are kept for
/// the orchestrator (SPEC 8.2), so adding one is a deliberate choice.
pub(crate) fn default_count(info: &HarnessInfo) -> u32 {
    let has_any = !info.accounts.is_empty() || info.importable > 0;
    u32::from(info.binary.is_some() && !has_any && info.harness != Harness::Claude)
}

/// Accounts that are known to be signed out (unknown states are left alone).
pub(crate) fn signed_out(ctx: &Ctx, host: &HostEnv) -> Result<Vec<AccountFile>> {
    Ok(AccountFile::list(&ctx.paths)?
        .into_iter()
        .filter(|a| a.enabled && sign_in_state(a, host).is_ok_and(|s| s == SignIn::SignedOut))
        .collect())
}

/// The note printed after adding claude accounts.
pub(crate) const CLAUDE_NOTE: &str = "claude accounts are kept for the orchestrator: the claude choices are disabled, so routing never spends them. To let routing use them, run `superorchestrator config edit` and set `enabled = true` on claude-sonnet and claude-haiku.";

/// Counts of doctor checks by status, plus the lines worth reading.
pub(crate) struct DoctorSummary {
    pub ok: usize,
    pub warn: usize,
    pub fail: usize,
    pub lines: Vec<String>,
}

pub(crate) fn doctor_summary(ctx: &Ctx, host: &HostEnv) -> DoctorSummary {
    let report = run_checks(&ctx.paths, host);
    let mut s = DoctorSummary {
        ok: 0,
        warn: 0,
        fail: 0,
        lines: Vec::new(),
    };
    for c in &report.checks {
        let mark = match c.status {
            Status::Ok => {
                s.ok += 1;
                continue;
            }
            Status::Warn => {
                s.warn += 1;
                "!"
            }
            Status::Fail => {
                s.fail += 1;
                "x"
            }
        };
        s.lines.push(format!("{mark} {}: {}", c.name, c.detail));
    }
    s
}

/// The counts asked for with flags, per harness.
fn asked(args: &SetupArgs, h: Harness) -> Option<u32> {
    match h {
        Harness::Codex => args.codex,
        Harness::Claude => args.claude,
        Harness::Opencode => args.opencode,
        Harness::Agy => args.agy,
    }
}

fn print_plan(
    ctx: &Ctx,
    infos: &[HarnessInfo],
    candidates: &[ImportCandidate],
    args: &SetupArgs,
    host: &HostEnv,
) -> Result<()> {
    let adds: Vec<(Harness, u32)> = infos
        .iter()
        .map(|i| {
            (
                i.harness,
                asked(args, i.harness).unwrap_or_else(|| default_count(i)),
            )
        })
        .collect();
    let value = json!({
        "ok": true,
        "dry_run": true,
        "harnesses": infos.iter().map(|i| json!({
            "harness": i.harness.as_str(), "installed": i.binary.is_some(), "accounts": i.accounts,
        })).collect::<Vec<_>>(),
        "import": candidates.iter().map(|c| c.name.clone()).collect::<Vec<_>>(),
        "add": adds.iter().map(|(h, n)| json!({"harness": h.as_str(), "count": n})).collect::<Vec<_>>(),
        "login": !args.no_login,
        "install": !args.no_install,
    });
    ctx.emit(&value, |w| {
        writeln!(w, "setup would (pass --yes to do it):")?;
        writeln!(w, "  1. create the config dir if missing (init)")?;
        writeln!(w, "  2. harnesses found:")?;
        for i in infos {
            let state = match &i.binary {
                Some(p) => p.display().to_string(),
                None => "not installed".to_string(),
            };
            writeln!(
                w,
                "       {:<9} {state} ({} account(s))",
                i.harness.as_str(),
                i.accounts.len()
            )?;
        }
        if candidates.is_empty() {
            writeln!(w, "  3. import: no existing logins found")?;
        } else {
            writeln!(w, "  3. import {} existing login(s):", candidates.len())?;
            for c in candidates {
                writeln!(
                    w,
                    "       {} ({}) at {}",
                    c.name,
                    c.harness,
                    host.tildify(&c.home)
                )?;
            }
        }
        writeln!(w, "  4. add accounts:")?;
        for (h, n) in &adds {
            writeln!(w, "       {:<9} {n}", h.as_str())?;
        }
        let login = if args.no_login {
            "skipped (--no-login)"
        } else {
            "sign in to accounts that are signed out, or print the commands (no terminal)"
        };
        writeln!(w, "  5. sign in: {login}")?;
        let install = if args.no_install {
            "skipped (--no-install)"
        } else {
            "register the MCP server and skill (install)"
        };
        writeln!(w, "  6. {install}")?;
        writeln!(w, "  7. summarise doctor")?;
        writeln!(
            w,
            "\nAdjust with --codex N --claude N --opencode N --agy N."
        )
    })
}

pub fn run(ctx: &Ctx, args: SetupArgs) -> Result<ExitCode> {
    match run_steps(ctx, args) {
        Err(e) if interrupted(&e) => {
            eprintln!("setup cancelled; re-run it to continue (safe to repeat)");
            Ok(ExitCode::from(130))
        }
        r => r,
    }
}

/// True when a prompt was cancelled with Ctrl-C.
fn interrupted(e: &anyhow::Error) -> bool {
    e.chain().any(|c| {
        matches!(c.downcast_ref::<dialoguer::Error>(), Some(dialoguer::Error::IO(io))
            if io.kind() == std::io::ErrorKind::Interrupted)
            || c.downcast_ref::<std::io::Error>()
                .is_some_and(|io| io.kind() == std::io::ErrorKind::Interrupted)
    })
}

fn run_steps(ctx: &Ctx, args: SetupArgs) -> Result<ExitCode> {
    let host = ctx.host()?;
    let tty = interactive();
    // Human output only; with --json the final summary is the only thing on stdout.
    macro_rules! say {
        ($($t:tt)*) => { if !ctx.json { println!($($t)*); } };
    }

    // Read-only first: the plan is also what a run without a terminal prints.
    let candidates = scan_candidates(ctx, &host)?;
    let infos = detect_harnesses(ctx, &host, &candidates)?;
    if !ctx.yes && !tty {
        print_plan(ctx, &infos, &candidates, &args, &host)?;
        return Ok(ExitCode::SUCCESS);
    }
    if ctx.json && !ctx.yes {
        return Err(CliError::new(
            crate::exit::USAGE,
            "setup --json needs --yes (without it, run without a terminal to print the plan)",
        )
        .into());
    }

    if step_init(ctx)? > 0 {
        say!("created the config dir {}", ctx.paths.config_dir.display());
    }
    let cfg = ctx.config()?;

    say!("harnesses:");
    for i in &infos {
        match &i.binary {
            Some(p) => say!("  {:<9} {}", i.harness, p.display()),
            None => say!("  {:<9} not installed", i.harness),
        }
    }

    let mut failed = false;
    let imported = ask_and_import(ctx, &cfg, &host, &candidates)?;
    failed |= imported.failed;
    imported.names.iter().for_each(|n| say!("imported {n}"));
    let infos = detect_harnesses(ctx, &host, &[])?;

    let mut created: Vec<AccountFile> = Vec::new();
    for i in &infos {
        let flag = asked(&args, i.harness);
        let default = flag.unwrap_or_else(|| default_count(i));
        let want = if i.binary.is_none() && flag.is_none() {
            0
        } else if ctx.yes || flag.is_some() {
            default
        } else {
            Input::<u32>::new()
                .with_prompt(format!(
                    "how many {} accounts to add ({} now, 0-99)?",
                    i.harness,
                    i.accounts.len()
                ))
                .default(default)
                .validate_with(|n: &u32| -> Result<(), &str> {
                    if *n <= 99 {
                        Ok(())
                    } else {
                        Err("enter a number from 0 to 99")
                    }
                })
                .interact_text()?
        };
        if want == 0 {
            continue;
        }
        if i.binary.is_none() {
            eprintln!(
                "{} was requested but its binary is not on PATH; install it and re-run setup",
                i.harness
            );
            failed = true;
            continue;
        }
        // On macOS a second enabled agy account is refused by `add_numbered`; that error
        // is reported below like any other.
        let batch = manage::add_numbered(&ctx.paths, &cfg, &host, i.harness, want);
        for a in &batch.created {
            say!("added account {} ({}) at {}", a.name, a.harness, a.home);
        }
        if !batch.created.is_empty() && i.harness == Harness::Claude {
            say!("{CLAUDE_NOTE}");
        }
        if let Some(e) = batch.error {
            eprintln!("could not add every {} account: {e}", i.harness);
            failed = true;
        }
        created.extend(batch.created);
    }
    if created.is_empty() {
        let all = AccountFile::list(&ctx.paths)?;
        let names: Vec<_> = all.iter().map(|a| a.name.as_str()).collect();
        say!(
            "{} accounts already set up: {}",
            names.len(),
            names.join(", ")
        );
    }

    // Sign-in covers new accounts and old ones that are still signed out.
    let pending = signed_out(ctx, &host)?;
    let mut needed: Vec<Value> = Vec::new();
    for a in &pending {
        let run = tty
            && !args.no_login
            && (ctx.yes
                || Confirm::new()
                    .with_prompt(format!("sign in to {} now?", a.name))
                    .default(true)
                    .interact()?);
        if run {
            if !run_login(ctx, a, &host)? {
                failed = true;
            }
            continue;
        }
        let login = login_json(a, &host)?;
        needed.push(json!({ "name": a.name, "login": login }));
        if !args.no_login {
            say!("{}:", a.name);
            if !ctx.json {
                let via_cli = matches!(a.harness, Harness::Codex | Harness::Opencode);
                write_login_hint(&mut std::io::stdout().lock(), &login, via_cli)?;
            }
        }
    }

    let mut installed = Value::Null;
    if !args.no_install {
        say!("\ninstall:");
        // The calls into `install`; the CLAUDE.md block is on unless `--no-claude-md`.
        let claude_md = !args.no_claude_md;
        let result = if ctx.json {
            crate::cmd::install::install_quiet(ctx, false, claude_md)
        } else {
            crate::cmd::install::install(ctx, false, false, claude_md).map(|()| Value::Null)
        };
        installed = json!(result.is_ok());
        if let Err(e) = result {
            eprintln!("install failed: {e:#}");
            failed = true;
        }
    }

    let d = doctor_summary(ctx, &host);
    say!(
        "\ndoctor: {} ok, {} warning(s), {} failure(s)",
        d.ok,
        d.warn,
        d.fail
    );
    if !ctx.json {
        d.lines.iter().for_each(|l| println!("  {l}"));
    }
    say!("\nnext: open Claude Code and ask it to delegate a task");

    if ctx.json {
        ctx.emit(
            &json!({
                "ok": !failed,
                "created": created.iter().map(|a| a.name.clone()).collect::<Vec<_>>(),
                "imported": imported.names,
                "installed": installed,
                "signed_in_needed": needed,
                "doctor": { "ok": d.ok, "warn": d.warn, "fail": d.fail, "lines": d.lines },
            }),
            |_| Ok(()),
        )?;
    }
    Ok(if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(harness: Harness, installed: bool, accounts: usize, importable: usize) -> HarnessInfo {
        HarnessInfo {
            harness,
            binary: installed.then(|| PathBuf::from("/bin/x")),
            accounts: (0..accounts).map(|n| format!("a-{n}")).collect(),
            importable,
        }
    }

    #[test]
    fn default_is_one_for_an_installed_harness_with_nothing() {
        assert_eq!(default_count(&info(Harness::Codex, true, 0, 0)), 1);
    }

    #[test]
    fn default_is_zero_when_not_installed_or_already_set_up() {
        assert_eq!(default_count(&info(Harness::Codex, false, 0, 0)), 0);
        assert_eq!(default_count(&info(Harness::Codex, true, 1, 0)), 0);
        assert_eq!(default_count(&info(Harness::Opencode, true, 0, 2)), 0);
    }

    #[test]
    fn ctrl_c_in_a_prompt_is_recognised() {
        let io = std::io::Error::from(std::io::ErrorKind::Interrupted);
        let e = anyhow::Error::from(dialoguer::Error::IO(io));
        assert!(interrupted(&e));
        assert!(!interrupted(&anyhow::anyhow!("other")));
    }

    #[test]
    fn claude_defaults_to_none() {
        assert_eq!(default_count(&info(Harness::Claude, true, 0, 0)), 0);
    }
}

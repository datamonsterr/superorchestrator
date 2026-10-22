//! `setup`: the first command a new user runs. Idempotent.
//!
//! The work is split into step functions (`step_init`, `detect_harnesses`,
//! `scan_candidates`, `add_accounts`, `login_accounts`, `run_install`,
//! `doctor_summary`) so a terminal UI can call them one by one; `run` drives them
//! with prompts.

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Result, bail};
use dialoguer::{Confirm, Input};
use serde_json::json;
use so_accounts::{HostEnv, ImportCandidate, manage, resolve_binary};
use so_core::{AccountFile, Config, Harness, Platform};
use so_supervisor::doctor::{Status, run_checks};

use crate::cli::SetupArgs;
use crate::cmd::account::{login_json, run_login, write_login_hint};
use crate::cmd::init;
use crate::ctx::{Ctx, interactive};

/// Harness order used for listing and prompting.
const ORDER: [Harness; 4] = [
    Harness::Codex,
    Harness::Claude,
    Harness::Opencode,
    Harness::Agy,
];

/// One harness as `setup` sees it.
#[derive(Debug, Clone)]
pub struct HarnessInfo {
    pub harness: Harness,
    /// The binary on PATH, when installed.
    pub binary: Option<PathBuf>,
    /// Names of the accounts that already exist.
    pub accounts: Vec<String>,
    /// Importable logins found by the scan for this harness.
    pub importable: usize,
}

/// What `setup` would add for one harness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Addition {
    pub harness: Harness,
    pub count: u32,
    /// Why the count differs from what was asked, or a hint worth printing.
    pub note: Option<String>,
}

/// Creates what `init` creates. Returns how many items were created (0 when all existed).
pub fn step_init(ctx: &Ctx) -> Result<usize> {
    let report = init::init(&ctx.paths)?;
    Ok(report
        .iter()
        .filter(|(_, o)| *o == init::Outcome::Created)
        .count())
}

/// Finds the harness binaries on PATH and the accounts that exist. Writes nothing.
pub fn detect_harnesses(
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
pub fn scan_candidates(ctx: &Ctx, host: &HostEnv) -> Result<Vec<ImportCandidate>> {
    Ok(so_accounts::scan_new(&ctx.paths, host)?)
}

/// Imports the chosen candidates. Returns the names imported; failures are reported
/// on stderr and skipped.
pub fn import_candidates(
    ctx: &Ctx,
    cfg: &Config,
    host: &HostEnv,
    chosen: &[ImportCandidate],
) -> Vec<String> {
    let mut done = Vec::new();
    for c in chosen {
        let home = host.tildify(&c.home);
        match manage::import(&ctx.paths, cfg, host, &c.name, c.harness, &home) {
            Ok(a) => done.push(a.name),
            Err(e) => eprintln!("could not import {}: {e}", c.name),
        }
    }
    done
}

/// How many accounts to add by default: one when the harness is installed and has no
/// account (or importable login) yet. Claude defaults to none: its accounts are kept for
/// the orchestrator (SPEC 8.2), so adding one is a deliberate choice.
pub fn default_count(info: &HarnessInfo) -> u32 {
    let has_any = !info.accounts.is_empty() || info.importable > 0;
    u32::from(info.binary.is_some() && !has_any && info.harness != Harness::Claude)
}

/// Applies the macOS rule that only one agy account may be enabled (ADR-0005).
pub fn cap_for_platform(
    info: &HarnessInfo,
    platform: Platform,
    asked: u32,
    paths: &so_core::Paths,
) -> Addition {
    let mut add = Addition {
        harness: info.harness,
        count: asked,
        note: None,
    };
    if info.harness == Harness::Agy && platform == Platform::MacOs && asked > 0 {
        let enabled_agy = AccountFile::list(paths)
            .unwrap_or_default()
            .iter()
            .any(|a| a.harness == Harness::Agy && a.enabled);
        let cap = u32::from(!enabled_agy);
        if asked > cap {
            add.count = cap;
            add.note = Some(if enabled_agy {
                "agy allows one enabled account on macOS and one exists; none added".to_string()
            } else {
                "agy allows one enabled account on macOS; adding one".to_string()
            });
        }
    }
    add
}

/// The note printed after adding claude accounts.
pub const CLAUDE_NOTE: &str = "claude accounts are kept for the orchestrator: the claude choices are disabled, so routing never spends them. To let routing use them, run `superorchestrator config edit` and set `enabled = true` on claude-sonnet and claude-haiku.";

/// Adds accounts with the lowest free `<harness>-<n>` names. A mid-batch failure keeps
/// what was created and is returned beside it.
pub fn add_accounts(
    ctx: &Ctx,
    cfg: &Config,
    host: &HostEnv,
    harness: Harness,
    count: u32,
) -> (Vec<AccountFile>, Option<so_accounts::AccountsError>) {
    let batch = manage::add_numbered(&ctx.paths, cfg, host, harness, count);
    (batch.created, batch.error)
}

/// Signs in to each account in turn, or prints the commands when there is no terminal.
/// Returns false when a login exited non-zero.
pub fn login_accounts(
    ctx: &Ctx,
    host: &HostEnv,
    accounts: &[AccountFile],
    tty: bool,
) -> Result<bool> {
    let mut all_ok = true;
    for a in accounts {
        let run = tty
            && (ctx.yes
                || Confirm::new()
                    .with_prompt(format!("sign in to {} now?", a.name))
                    .default(true)
                    .interact()?);
        if run {
            all_ok &= run_login(ctx, a, host)?;
        } else {
            let login = login_json(a, host)?;
            let mut out = std::io::stdout().lock();
            writeln!(out, "{}:", a.name)?;
            write_login_hint(
                &mut out,
                &login,
                matches!(a.harness, Harness::Codex | Harness::Opencode),
            )?;
        }
    }
    Ok(all_ok)
}

/// Registers the MCP server and skill. The one call site of `install` in this module.
pub fn run_install(ctx: &Ctx) -> Result<()> {
    crate::cmd::install::install(ctx, false, false)
}

/// Counts of doctor checks by status, plus the lines worth reading.
pub struct DoctorSummary {
    pub ok: usize,
    pub warn: usize,
    pub fail: usize,
    pub lines: Vec<String>,
}

pub fn doctor_summary(ctx: &Ctx, host: &HostEnv) -> DoctorSummary {
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

/// What `setup` would add without prompting: the flag count, else the default.
fn planned(ctx: &Ctx, host: &HostEnv, args: &SetupArgs, infos: &[HarnessInfo]) -> Vec<Addition> {
    infos
        .iter()
        .map(|i| {
            let n = asked(args, i.harness).unwrap_or_else(|| default_count(i));
            cap_for_platform(i, host.platform, n, &ctx.paths)
        })
        .collect()
}

fn print_plan(
    ctx: &Ctx,
    infos: &[HarnessInfo],
    candidates: &[ImportCandidate],
    adds: &[Addition],
    args: &SetupArgs,
    host: &HostEnv,
) -> Result<()> {
    let value = json!({
        "ok": true,
        "dry_run": true,
        "harnesses": infos.iter().map(|i| json!({
            "harness": i.harness.as_str(), "installed": i.binary.is_some(), "accounts": i.accounts,
        })).collect::<Vec<_>>(),
        "import": candidates.iter().map(|c| c.name.clone()).collect::<Vec<_>>(),
        "add": adds.iter().map(|a| json!({"harness": a.harness.as_str(), "count": a.count})).collect::<Vec<_>>(),
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
        for a in adds {
            write!(w, "       {:<9} {}", a.harness.as_str(), a.count)?;
            if let Some(n) = &a.note {
                write!(w, "  ({n})")?;
            }
            writeln!(w)?;
        }
        let login = if args.no_login {
            "skipped (--no-login)"
        } else {
            "print the sign-in commands (no terminal)"
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
    let host = ctx.host()?;
    let tty = interactive();

    // Read-only first: the plan is also what a run without a terminal prints.
    let candidates = scan_candidates(ctx, &host)?;
    let infos = detect_harnesses(ctx, &host, &candidates)?;
    if !ctx.yes && !tty {
        let adds = planned(ctx, &host, &args, &infos);
        print_plan(ctx, &infos, &candidates, &adds, &args, &host)?;
        return Ok(ExitCode::SUCCESS);
    }
    if ctx.json {
        bail!("`setup --json` only prints the plan; run it without a terminal and without --yes");
    }

    if step_init(ctx)? > 0 {
        println!("created the config dir {}", ctx.paths.config_dir.display());
    }
    let cfg = ctx.config()?;

    println!("harnesses:");
    for i in &infos {
        match &i.binary {
            Some(p) => println!("  {:<9} {}", i.harness, p.display()),
            None => println!("  {:<9} not installed", i.harness),
        }
    }

    // Import before counting, so a harness that has a login of its own gets no extra account.
    let mut chosen = Vec::new();
    for c in &candidates {
        let take = ctx.yes
            || Confirm::new()
                .with_prompt(format!(
                    "import {} ({}) at {}?",
                    c.name,
                    c.harness,
                    host.tildify(&c.home)
                ))
                .default(true)
                .interact()?;
        if take {
            chosen.push(c.clone());
        }
    }
    for name in import_candidates(ctx, &cfg, &host, &chosen) {
        println!("imported {name}");
    }
    let infos = detect_harnesses(ctx, &host, &[])?;

    let mut created = Vec::new();
    let mut failed = false;
    for i in &infos {
        let default = asked(&args, i.harness).unwrap_or_else(|| default_count(i));
        let want = if i.binary.is_none() && asked(&args, i.harness).is_none() {
            0
        } else if ctx.yes || asked(&args, i.harness).is_some() {
            default
        } else {
            Input::<u32>::new()
                .with_prompt(format!(
                    "how many {} accounts to add ({} now)?",
                    i.harness,
                    i.accounts.len()
                ))
                .default(default)
                .interact_text()?
        };
        if want == 0 {
            continue;
        }
        if i.binary.is_none() {
            println!(
                "skipping {}: the binary is not on PATH; install it and re-run setup",
                i.harness
            );
            continue;
        }
        let add = cap_for_platform(i, host.platform, want, &ctx.paths);
        if let Some(n) = &add.note {
            println!("{}: {n}", i.harness);
        }
        if add.count == 0 {
            continue;
        }
        let (accounts, error) = add_accounts(ctx, &cfg, &host, i.harness, add.count);
        for a in &accounts {
            println!("added account {} ({}) at {}", a.name, a.harness, a.home);
        }
        if !accounts.is_empty() && i.harness == Harness::Claude {
            println!("{CLAUDE_NOTE}");
        }
        if let Some(e) = error {
            eprintln!("could not add every {} account: {e}", i.harness);
            failed = true;
        }
        created.extend(accounts);
    }
    if created.is_empty() {
        println!("no accounts added");
    } else {
        ctx.refresh_skill();
        if !args.no_login && !login_accounts(ctx, &host, &created, tty)? {
            failed = true;
        }
    }

    if !args.no_install {
        println!("\ninstall:");
        if let Err(e) = run_install(ctx) {
            eprintln!("install failed: {e:#}");
            failed = true;
        }
    }

    let d = doctor_summary(ctx, &host);
    println!(
        "\ndoctor: {} ok, {} warning(s), {} failure(s)",
        d.ok, d.warn, d.fail
    );
    d.lines.iter().for_each(|l| println!("  {l}"));

    println!("\nnext: open Claude Code and ask it to delegate a task");
    Ok(if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

//! `account add|import|login|list|remove|enable|disable|flag`.

use std::io::Write;
use std::os::fd::AsFd;
use std::process::{Command, ExitCode, Stdio};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Duration, Utc};
use dialoguer::{Confirm, Input};
use serde_json::{Value, json};
use so_accounts::{HostEnv, ImportCandidate, manage};
use so_core::{AccountFile, Config, Harness, Paths};
use so_store::{FlagRow, Store};

use crate::cli::{AccountAdd, AccountCmd, AccountFlag, AccountImport, AccountRemove};
use crate::ctx::{Ctx, interactive, stdin_is_tty};
use crate::exit::CliError;
use crate::views::{collect_views, views_json, write_views_table};

pub fn run(ctx: &Ctx, cmd: AccountCmd) -> Result<ExitCode> {
    match cmd {
        AccountCmd::Add(a) => add(ctx, a),
        AccountCmd::Import(a) => import(ctx, a),
        AccountCmd::Login { name } => login(ctx, &name),
        AccountCmd::List => {
            print_accounts(ctx)?;
            Ok(ExitCode::SUCCESS)
        }
        AccountCmd::Remove(a) => remove(ctx, a),
        AccountCmd::Enable { name } => set_enabled(ctx, &name, true),
        AccountCmd::Disable { name } => set_enabled(ctx, &name, false),
        AccountCmd::Flag(a) => flag(ctx, a),
    }
}

fn parse_harness(s: &str) -> Result<Harness> {
    s.parse::<Harness>()
        .map_err(|_| anyhow::anyhow!("unknown harness `{s}` (codex, agy, opencode or claude)"))
}

fn load_account(paths: &Paths, name: &str) -> Result<AccountFile> {
    let file = paths.account_file(name);
    if !file.exists() {
        return Err(CliError::not_found(format!("account `{name}` not found")).into());
    }
    Ok(AccountFile::load(&file)?)
}

fn add(ctx: &Ctx, a: AccountAdd) -> Result<ExitCode> {
    let harness = parse_harness(&a.harness)?;
    let cfg = ctx.config()?;
    let host = ctx.host()?;
    let account = manage::add(&ctx.paths, &cfg, &host, harness, &a.name, a.home.as_deref())?;
    let login = login_json(&account, &host)?;
    ctx.emit(
        &json!({
            "ok": true, "name": account.name, "harness": account.harness.as_str(),
            "home": account.home, "login": login,
        }),
        |w| {
            writeln!(
                w,
                "added account {} ({}) at {}",
                account.name, account.harness, account.home
            )?;
            write_login_hint(w, &login)
        },
    )?;
    Ok(ExitCode::SUCCESS)
}

/// The login command for an account as `{ command, note }`.
fn login_json(account: &AccountFile, host: &HostEnv) -> Result<Value> {
    let plan = manage::login_command(account, host, stdin_is_tty())?;
    let env: Vec<String> = plan.env.iter().map(|(k, v)| format!("{k}={v}")).collect();
    let command = if env.is_empty() {
        plan.argv.join(" ")
    } else {
        format!("{} {}", env.join(" "), plan.argv.join(" "))
    };
    Ok(json!({ "command": command, "note": plan.note }))
}

fn write_login_hint(w: &mut dyn Write, login: &Value) -> std::io::Result<()> {
    if let Some(c) = login["command"].as_str() {
        writeln!(w, "to sign in, run: {c}")?;
    }
    if let Some(n) = login["note"].as_str() {
        writeln!(w, "note: {n}")?;
    }
    Ok(())
}

fn import(ctx: &Ctx, a: AccountImport) -> Result<ExitCode> {
    let cfg = ctx.config()?;
    let host = ctx.host()?;
    if a.scan {
        return import_scan(ctx, &cfg, &host);
    }
    let (Some(name), Some(harness), Some(home)) = (a.name, a.harness, a.home) else {
        bail!("`account import <name>` needs --harness and --home (or use --scan)");
    };
    let acc = manage::import(
        &ctx.paths,
        &cfg,
        &host,
        &name,
        parse_harness(&harness)?,
        &home,
    )?;
    ctx.emit_ok(
        json!({ "name": acc.name, "harness": acc.harness.as_str(), "home": acc.home }),
        &format!(
            "imported account {} ({}) from {}",
            acc.name, acc.harness, acc.home
        ),
    )?;
    Ok(ExitCode::SUCCESS)
}

fn describe(c: &ImportCandidate, host: &HostEnv) -> String {
    format!("{} ({}) at {}", c.name, c.harness, host.tildify(&c.home))
}

pub fn import_scan(ctx: &Ctx, cfg: &Config, host: &HostEnv) -> Result<ExitCode> {
    let found = so_accounts::scan_new(&ctx.paths, host)?;
    let candidates = |found: &[ImportCandidate]| -> Vec<Value> {
        found
            .iter()
            .map(|c| json!({ "name": c.name, "harness": c.harness.as_str(), "home": host.tildify(&c.home) }))
            .collect()
    };
    let tty = interactive();
    if found.is_empty() || (!ctx.yes && !tty) {
        ctx.emit(
            &json!({ "ok": true, "imported": [], "candidates": candidates(&found) }),
            |w| {
                if found.is_empty() {
                    return writeln!(w, "no new accounts found");
                }
                writeln!(
                    w,
                    "found {} importable account(s); pass --yes to import them:",
                    found.len()
                )?;
                found
                    .iter()
                    .try_for_each(|c| writeln!(w, "  {}", describe(c, host)))
            },
        )?;
        return Ok(ExitCode::SUCCESS);
    }
    let mut imported = Vec::new();
    let mut failed = false;
    for c in &found {
        let take = ctx.yes
            || Confirm::new()
                .with_prompt(format!("import {}?", describe(c, host)))
                .default(true)
                .interact()?;
        if !take {
            continue;
        }
        let home = host.tildify(&c.home);
        match manage::import(&ctx.paths, cfg, host, &c.name, c.harness, &home) {
            Ok(acc) => imported.push(acc.name),
            Err(e) => {
                eprintln!("could not import {}: {e}", c.name);
                failed = true;
            }
        }
    }
    ctx.emit(&json!({ "ok": !failed, "imported": imported }), |w| {
        imported
            .iter()
            .try_for_each(|n| writeln!(w, "imported {n}"))
    })?;
    Ok(if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

fn login(ctx: &Ctx, name: &str) -> Result<ExitCode> {
    let account = load_account(&ctx.paths, name)?;
    let host = ctx.host()?;
    let tty = stdin_is_tty();
    let plan = manage::login_command(&account, &host, tty)?;
    let runnable = tty || account.harness == Harness::Codex;
    if !runnable {
        let login = login_json(&account, &host)?;
        ctx.emit(&json!({ "ok": true, "name": name, "login": login }), |w| {
            write_login_hint(w, &login)
        })?;
        return Ok(ExitCode::SUCCESS);
    }
    if let Some(note) = &plan.note
        && !ctx.json
    {
        println!("note: {note}");
    }
    let (program, args) = plan.argv.split_first().context("empty login command")?;
    let mut cmd = Command::new(program);
    cmd.args(args).envs(plan.env.iter().map(|(k, v)| (k, v)));
    if ctx.json {
        // Keep stdout for the JSON result; the harness talks on stderr.
        cmd.stdout(Stdio::from(std::io::stderr().as_fd().try_clone_to_owned()?));
    }
    let status = cmd
        .status()
        .with_context(|| format!("could not run `{program}`"))?;
    if !status.success() {
        return Ok(ExitCode::FAILURE);
    }
    ctx.emit_ok(
        json!({ "name": name }),
        &format!("login finished for {name}"),
    )?;
    Ok(ExitCode::SUCCESS)
}

fn remove(ctx: &Ctx, a: AccountRemove) -> Result<ExitCode> {
    let cfg = ctx.config()?;
    let host = ctx.host()?;
    load_account(&ctx.paths, &a.name)?;
    if !ctx.yes {
        if !interactive() {
            bail!(
                "refusing to remove `{}` without confirmation; pass --yes",
                a.name
            );
        }
        let typed: String = Input::new()
            .with_prompt(format!("type the account name `{}` to confirm", a.name))
            .interact_text()?;
        if typed.trim() != a.name {
            bail!("name did not match; nothing removed");
        }
    }
    manage::remove(&ctx.paths, &cfg, &host, &a.name, a.purge)?;
    ctx.emit_ok(
        json!({ "name": a.name, "purged": a.purge }),
        &format!("removed account {}", a.name),
    )?;
    Ok(ExitCode::SUCCESS)
}

fn set_enabled(ctx: &Ctx, name: &str, enabled: bool) -> Result<ExitCode> {
    manage::set_enabled(&ctx.paths, name, enabled)?;
    let word = if enabled { "enabled" } else { "disabled" };
    ctx.emit_ok(
        json!({ "name": name, "enabled": enabled }),
        &format!("{word} {name}"),
    )?;
    Ok(ExitCode::SUCCESS)
}

fn flag(ctx: &Ctx, a: AccountFlag) -> Result<ExitCode> {
    let account = load_account(&ctx.paths, &a.name)?;
    let pools: Vec<String> = match &a.pool {
        Some(p) => vec![p.clone()],
        None => account.pools.clone(),
    };
    let store = Store::open(ctx.paths.state_db()).context("could not open state.db")?;
    if a.clear {
        for p in &pools {
            store.clear_flag(&a.name, p)?;
        }
        ctx.emit_ok(
            json!({ "name": a.name, "pools": pools, "until": null }),
            &format!("cleared cooldown on {}", a.name),
        )?;
        return Ok(ExitCode::SUCCESS);
    }
    let until: DateTime<Utc> = match (&a.until, a.hours) {
        (Some(s), _) => DateTime::parse_from_rfc3339(s)
            .with_context(|| format!("`{s}` is not an RFC 3339 time"))?
            .with_timezone(&Utc),
        (None, Some(h)) if h.is_finite() && h > 0.0 => {
            Utc::now() + Duration::seconds((h * 3600.0) as i64)
        }
        _ => bail!("--hours must be a positive number"),
    };
    for p in &pools {
        store.set_flag(&FlagRow {
            account: a.name.clone(),
            pool: p.clone(),
            until,
            reason: "manual".into(),
        })?;
    }
    ctx.emit_ok(
        json!({ "name": a.name, "pools": pools, "until": until.to_rfc3339() }),
        &format!("{} in cooldown until {}", a.name, until.to_rfc3339()),
    )?;
    Ok(ExitCode::SUCCESS)
}

pub fn print_accounts(ctx: &Ctx) -> Result<()> {
    let views = collect_views(ctx)?;
    ctx.emit(&views_json(&views), |w| write_views_table(w, &views))
}

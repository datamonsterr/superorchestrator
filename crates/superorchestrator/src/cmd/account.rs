//! `account add|import|login|list|remove|enable|disable|flag`.

use std::process::{Command, ExitCode};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Duration, Utc};
use dialoguer::{Confirm, Input};
use serde_json::{Value, json};
use so_accounts::{HostEnv, ImportCandidate, SignIn, manage};
use so_core::{AccountFile, Config, Harness, Paths};
use so_store::{FlagRow, Store};

use crate::cli::{AccountAdd, AccountCmd, AccountFlag, AccountImport, AccountRemove};
use crate::ctx::{Ctx, interactive, print_table, stdin_is_tty};

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
        bail!("account `{name}` not found");
    }
    Ok(AccountFile::load(&file)?)
}

fn add(ctx: &Ctx, a: AccountAdd) -> Result<ExitCode> {
    let harness = parse_harness(&a.harness)?;
    let cfg = ctx.config()?;
    let host = ctx.host()?;
    let account = manage::add(&ctx.paths, &cfg, &host, harness, &a.name, a.home.as_deref())?;
    println!(
        "added account {} ({}) at {}",
        account.name, account.harness, account.home
    );
    print_login_hint(&account, &host)?;
    Ok(ExitCode::SUCCESS)
}

fn print_login_hint(account: &AccountFile, host: &HostEnv) -> Result<()> {
    let plan = manage::login_command(account, host, stdin_is_tty())?;
    let env: Vec<String> = plan.env.iter().map(|(k, v)| format!("{k}={v}")).collect();
    let cmd = if env.is_empty() {
        plan.argv.join(" ")
    } else {
        format!("{} {}", env.join(" "), plan.argv.join(" "))
    };
    println!("to sign in, run: {cmd}");
    if let Some(note) = plan.note {
        println!("note: {note}");
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
    println!(
        "imported account {} ({}) from {}",
        acc.name, acc.harness, acc.home
    );
    Ok(ExitCode::SUCCESS)
}

fn describe(c: &ImportCandidate, host: &HostEnv) -> String {
    format!("{} ({}) at {}", c.name, c.harness, host.tildify(&c.home))
}

pub fn import_scan(ctx: &Ctx, cfg: &Config, host: &HostEnv) -> Result<ExitCode> {
    let found = so_accounts::scan_new(&ctx.paths, host)?;
    if found.is_empty() {
        println!("no new accounts found");
        return Ok(ExitCode::SUCCESS);
    }
    let tty = interactive();
    if !ctx.yes && !tty {
        println!(
            "found {} importable account(s); pass --yes to import them:",
            found.len()
        );
        for c in &found {
            println!("  {}", describe(c, host));
        }
        return Ok(ExitCode::SUCCESS);
    }
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
            Ok(acc) => println!("imported {} ({})", acc.name, acc.harness),
            Err(e) => {
                eprintln!("could not import {}: {e}", c.name);
                failed = true;
            }
        }
    }
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
        print_login_hint(&account, &host)?;
        return Ok(ExitCode::SUCCESS);
    }
    if let Some(note) = &plan.note {
        println!("note: {note}");
    }
    let (program, args) = plan.argv.split_first().context("empty login command")?;
    let status = Command::new(program)
        .args(args)
        .envs(plan.env.iter().map(|(k, v)| (k, v)))
        .status()
        .with_context(|| format!("could not run `{program}`"))?;
    Ok(if status.success() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
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
    println!("removed account {}", a.name);
    Ok(ExitCode::SUCCESS)
}

fn set_enabled(ctx: &Ctx, name: &str, enabled: bool) -> Result<ExitCode> {
    manage::set_enabled(&ctx.paths, name, enabled)?;
    println!("{} {name}", if enabled { "enabled" } else { "disabled" });
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
        println!("cleared cooldown on {}", a.name);
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
    println!("{} in cooldown until {}", a.name, until.to_rfc3339());
    Ok(ExitCode::SUCCESS)
}

/// One account as shown by `account list` and `status`.
pub struct AccountView {
    pub account: AccountFile,
    pub signed_in: SignIn,
    pub active_runs: Option<i64>,
    pub headroom: Vec<(String, String)>,
}

pub fn sign_in_text(s: SignIn) -> &'static str {
    match s {
        SignIn::SignedIn => "yes",
        SignIn::SignedOut => "no",
        SignIn::Unknown => "unknown",
    }
}

/// Opens `state.db` only when it already exists.
pub fn open_existing_store(paths: &Paths) -> Option<Store> {
    paths
        .state_db()
        .exists()
        .then(|| Store::open(paths.state_db()).ok())
        .flatten()
}

pub fn collect_views(ctx: &Ctx) -> Result<Vec<AccountView>> {
    let host = ctx.host()?;
    let store = open_existing_store(&ctx.paths);
    let now = Utc::now();
    let mut out = Vec::new();
    for account in AccountFile::list(&ctx.paths)? {
        let signed_in = so_accounts::sign_in_state(&account, &host)?;
        let active_runs = store
            .as_ref()
            .and_then(|s| s.count_active_runs_by_account(&account.name).ok());
        let headroom = account
            .pools
            .iter()
            .map(|pool| {
                let flag = store
                    .as_ref()
                    .and_then(|s| s.get_flag(&account.name, pool).ok().flatten())
                    .filter(|f| f.until > now);
                let text = if let Some(f) = flag {
                    format!("cooldown until {}", f.until.to_rfc3339())
                } else {
                    match store
                        .as_ref()
                        .and_then(|s| s.get_headroom(&account.name, pool).ok().flatten())
                    {
                        Some(h) => match h.five_hour_remaining_pct {
                            Some(p) => format!("{} {p:.0}%", h.status.as_str()),
                            None => h.status.as_str().to_string(),
                        },
                        None => "unknown".to_string(),
                    }
                };
                (pool.clone(), text)
            })
            .collect();
        out.push(AccountView {
            account,
            signed_in,
            active_runs,
            headroom,
        });
    }
    Ok(out)
}

pub fn views_json(views: &[AccountView]) -> Value {
    Value::Array(
        views
            .iter()
            .map(|v| {
                json!({
                    "name": v.account.name,
                    "harness": v.account.harness.as_str(),
                    "signed_in": sign_in_text(v.signed_in),
                    "enabled": v.account.enabled,
                    "cap": v.account.cap,
                    "active_runs": v.active_runs.unwrap_or(0),
                    "headroom": v.headroom.iter()
                        .map(|(p, t)| (p.clone(), Value::String(t.clone())))
                        .collect::<serde_json::Map<_, _>>(),
                })
            })
            .collect(),
    )
}

pub fn print_views_table(views: &[AccountView]) {
    if views.is_empty() {
        println!("no accounts; add one with `superorchestrator account add`");
        return;
    }
    let rows: Vec<Vec<String>> = views
        .iter()
        .map(|v| {
            vec![
                v.account.name.clone(),
                v.account.harness.as_str().to_string(),
                sign_in_text(v.signed_in).to_string(),
                if v.account.enabled { "yes" } else { "no" }.to_string(),
                v.account.cap.to_string(),
                v.active_runs.unwrap_or(0).to_string(),
                v.headroom
                    .iter()
                    .map(|(p, t)| format!("{p}: {t}"))
                    .collect::<Vec<_>>()
                    .join(", "),
            ]
        })
        .collect();
    print_table(
        &[
            "NAME",
            "HARNESS",
            "SIGNED IN",
            "ENABLED",
            "CAP",
            "ACTIVE",
            "HEADROOM",
        ],
        &rows,
    );
}

pub fn print_accounts(ctx: &Ctx) -> Result<()> {
    let views = collect_views(ctx)?;
    if ctx.json {
        println!("{}", serde_json::to_string_pretty(&views_json(&views))?);
    } else {
        print_views_table(&views);
    }
    Ok(())
}

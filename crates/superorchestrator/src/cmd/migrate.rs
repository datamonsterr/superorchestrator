//! `migrate`: cut over from the legacy tooling (SPEC 19.2).
//!
//! One code path serves the dry run and `--apply`: every step goes through a shared
//! [`Writer`] whose `dry_run` flag turns writes, removals and backups into no-ops, so the
//! printed plan is exactly what `--apply` does. Every step checks its own precondition,
//! so a second `--apply` finds nothing to do and writes nothing (not even a backup dir).
//! Credential files are never opened: accounts are imported by linking their homes.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Utc};
use dialoguer::Confirm;
use serde::Serialize;
use serde_json::{Value, json};
use so_accounts::{HostEnv, SELF_EXE, is_managed, manage, scan, scan_new, write_wrapper};
use so_core::{AccountFile, Config, Harness};
use so_store::Store;
use so_supervisor::services::accounts::{FlagAction, apply_flag};
use toml_edit::{Array, ArrayOfTables, DocumentMut, Item as TomlItem, Table, value};

use super::init;
use super::install::{self, FileTarget, Kind, Writer};
use crate::ctx::{Ctx, interactive};
use crate::register::{codex_remove_key, json_remove_key};

/// The legacy router's MCP server name in the harness configs.
const JEV_KEY: &str = "jev-router-mcp";

/// One thing the plan found, and what happens to it.
#[derive(Debug, Serialize)]
struct Item {
    step: &'static str,
    /// `import`, `rewrite`, `flag`, `remove`, `update`, `create`, `skip` or `error`.
    action: String,
    target: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
}

#[derive(Default)]
struct Report {
    items: Vec<Item>,
    diff: Option<String>,
}

impl Report {
    fn add(&mut self, step: &'static str, action: &str, target: impl Into<String>) {
        self.items.push(Item {
            step,
            action: action.to_string(),
            target: target.into(),
            note: None,
        });
    }

    fn add_note(&mut self, step: &'static str, action: &str, target: String, note: String) {
        self.items.push(Item {
            step,
            action: action.to_string(),
            target,
            note: Some(note),
        });
    }

    fn error(&mut self, step: &'static str, target: impl Into<String>, e: &anyhow::Error) {
        self.add_note(step, "error", target.into(), format!("{e:#}"));
    }

    fn changed(&self) -> bool {
        self.items
            .iter()
            .any(|i| !matches!(i.action.as_str(), "skip" | "unchanged"))
    }

    fn failures(&self) -> usize {
        self.items.iter().filter(|i| i.action == "error").count()
    }
}

pub fn run(ctx: &Ctx, apply: bool) -> Result<ExitCode> {
    let plan = collect(ctx, true)?;
    if !apply {
        print(ctx, &plan, true, None)?;
        return Ok(ExitCode::SUCCESS);
    }
    if !plan.changed() {
        print(ctx, &plan, false, None)?;
        return Ok(ExitCode::SUCCESS);
    }
    if !ctx.yes {
        if !interactive() {
            bail!("refusing to apply the migration without confirmation; pass --yes");
        }
        // The plan goes to stderr so `--json` keeps stdout for the result.
        eprint!("{}", render(&plan, true));
        let go = Confirm::new()
            .with_prompt("apply this migration?")
            .default(false)
            .interact()?;
        if !go {
            bail!("not confirmed; nothing was changed");
        }
    }
    let done = collect(ctx, false)?;
    let backup = backup_dir_of(ctx, &done);
    print(ctx, &done, false, backup)?;
    match done.failures() {
        0 => Ok(ExitCode::SUCCESS),
        n => Err(anyhow!("{n} step(s) failed")),
    }
}

/// The newest `legacy-backup/<ts>` directory when this run wrote backups.
fn backup_dir_of(ctx: &Ctx, report: &Report) -> Option<PathBuf> {
    if !report.changed() {
        return None;
    }
    let root = ctx.paths.config_dir.join("legacy-backup");
    let mut dirs: Vec<_> = std::fs::read_dir(root)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .collect();
    dirs.sort();
    dirs.pop()
}

fn render(report: &Report, dry_run: bool) -> String {
    let mut out = String::new();
    if dry_run {
        out.push_str("dry run: nothing was changed (pass --apply to execute)\n");
    }
    if !report.changed() && report.failures() == 0 {
        out.push_str("nothing to do\n");
    }
    for i in &report.items {
        out.push_str(&format!("{:<8} {:<9} {}", i.step, i.action, i.target));
        if let Some(n) = &i.note {
            out.push_str(&format!("  ({n})"));
        }
        out.push('\n');
    }
    if let Some(d) = &report.diff {
        out.push('\n');
        out.push_str(d);
    }
    out
}

fn print(ctx: &Ctx, report: &Report, dry_run: bool, backup: Option<PathBuf>) -> Result<()> {
    let value = json!({
        "ok": report.failures() == 0,
        "dry_run": dry_run,
        "changed": report.changed(),
        "backup_dir": backup,
        "items": report.items,
        "diff": report.diff,
    });
    ctx.emit(&value, |w| {
        write!(w, "{}", render(report, dry_run))?;
        if let Some(b) = &backup {
            writeln!(w, "\nbackups: {}", b.display())?;
        }
        Ok(())
    })
}

/// Runs every step against a shared writer; `dry_run` only plans.
fn collect(ctx: &Ctx, dry_run: bool) -> Result<Report> {
    let host = ctx.host()?;
    let w = Writer::new(&ctx.paths, dry_run);
    let mut r = Report::default();

    step_init(ctx, dry_run, &mut r);
    let cfg = ctx.config()?;
    let accounts = import_accounts(ctx, &cfg, &host, &w, &mut r);
    cooldowns(ctx, &host, &w, &accounts, &mut r);
    seed_choices(ctx, &host, &w, &mut r);
    remove_legacy(&host, &w, &mut r);
    remove_jev_entries(ctx, &host, &w, &mut r);
    install_step(ctx, &w, &mut r);
    claude_md(&host, &w, &mut r);
    Ok(r)
}

fn step_init(ctx: &Ctx, dry_run: bool, r: &mut Report) {
    let target = ctx.paths.config_dir.display().to_string();
    if dry_run {
        if !ctx.paths.config_toml().exists() {
            r.add("init", "create", target);
        }
        return;
    }
    match init::init(&ctx.paths) {
        Ok(done) => {
            if done.iter().any(|(_, o)| *o == init::Outcome::Created) {
                r.add("init", "create", target);
            }
        }
        Err(e) => r.error("init", target, &e),
    }
}

/// Copies `path` into the backup tree: a file as is, a directory recursively, a symlink
/// as a symlink. Does nothing in a dry run.
fn backup_tree(w: &Writer, path: &Path) -> Result<()> {
    if w.dry_run {
        return Ok(());
    }
    let meta = std::fs::symlink_metadata(path)
        .with_context(|| format!("could not stat {}", path.display()))?;
    if meta.is_symlink() {
        let dest = w.backup_path(path);
        if let Some(dir) = dest.parent() {
            install::create_private_dirs(dir)?;
        }
        let target = std::fs::read_link(path)?;
        if !dest.exists() {
            std::os::unix::fs::symlink(target, &dest)
                .with_context(|| format!("could not back up link {}", path.display()))?;
        }
    } else if meta.is_dir() {
        install::create_private_dirs(&w.backup_path(path))?;
        let mut kids: Vec<_> = std::fs::read_dir(path)?
            .flatten()
            .map(|e| e.path())
            .collect();
        kids.sort();
        for k in kids {
            backup_tree(w, &k)?;
        }
    } else {
        w.backup(path)?;
    }
    Ok(())
}

/// Backs up and removes a file, link or directory tree; the parent stays.
fn remove_path(w: &Writer, path: &Path) -> Result<()> {
    if w.dry_run {
        return Ok(());
    }
    backup_tree(w, path)?;
    let meta = std::fs::symlink_metadata(path)?;
    if meta.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
    .with_context(|| format!("could not remove {}", path.display()))
}

/// Steps 2: import the scanned accounts and rewrite their wrappers. Returns the names of
/// the accounts that exist (or, in a dry run, will exist) afterwards.
fn import_accounts(
    ctx: &Ctx,
    cfg: &Config,
    host: &HostEnv,
    w: &Writer,
    r: &mut Report,
) -> Vec<String> {
    const STEP: &str = "accounts";
    let all = scan(&host.home);
    let new = match scan_new(&ctx.paths, host) {
        Ok(n) => n,
        Err(e) => {
            r.error(STEP, "scan", &e.into());
            return Vec::new();
        }
    };
    let wdir = match host.expand(&cfg.accounts.wrapper_dir) {
        Ok(d) => d,
        Err(e) => {
            r.error(STEP, "wrapper_dir", &e.into());
            return Vec::new();
        }
    };
    let mut present = Vec::new();
    for c in &all {
        let is_new = new.iter().any(|n| n.name == c.name);
        if !is_new && !ctx.paths.account_file(&c.name).exists() {
            continue; // its home already belongs to an account with another name
        }
        let wrapper = wdir.join(&c.name);
        let foreign = wrapper.symlink_metadata().is_ok() && !is_managed(&wrapper);
        let missing = wrapper.symlink_metadata().is_err();
        let desc = format!("{} ({}) at {}", c.name, c.harness, host.tildify(&c.home));
        let result = (|| -> Result<()> {
            if !is_new && !foreign && !missing {
                return Ok(());
            }
            if foreign {
                // `write_wrapper` also parks the old file in `wrappers.bak`.
                backup_tree(w, &wrapper)?;
            }
            if is_new {
                r.add(STEP, "import", desc.clone());
                if !w.dry_run {
                    manage::import(
                        &ctx.paths,
                        cfg,
                        host,
                        &c.name,
                        c.harness,
                        &host.tildify(&c.home),
                    )?;
                }
            } else {
                r.add(STEP, "rewrite", format!("wrapper {}", wrapper.display()));
                if !w.dry_run {
                    write_wrapper(&wdir, &ctx.paths.wrappers_bak_dir(), &c.name, SELF_EXE)?;
                }
            }
            Ok(())
        })();
        match result {
            Ok(()) => present.push(c.name.clone()),
            Err(e) => r.error(STEP, desc, &e),
        }
    }
    present
}

fn parse_exhausted_until(text: &str) -> Option<DateTime<Utc>> {
    let secs = text
        .lines()
        .find_map(|l| l.trim().strip_prefix("exhausted_until="))?
        .trim()
        .parse::<i64>()
        .ok()?;
    DateTime::from_timestamp(secs, 0)
}

/// Step 3: legacy cooldowns still in the future become flags.
fn cooldowns(ctx: &Ctx, host: &HostEnv, w: &Writer, accounts: &[String], r: &mut Report) {
    const STEP: &str = "cooldown";
    let root = host.home.join(".local/state/coding-agent/.state");
    let mut metas: Vec<PathBuf> = Vec::new();
    for dir in sorted_children(&root) {
        metas.extend(
            sorted_children(&dir)
                .into_iter()
                .filter(|p| p.extension().is_some_and(|x| x == "meta")),
        );
    }
    if metas.is_empty() {
        return;
    }
    let db = ctx.paths.state_db();
    let store = if !w.dry_run || db.exists() {
        match Store::open(&db) {
            Ok(s) => Some(s),
            Err(e) => {
                r.error(STEP, db.display().to_string(), &e.into());
                return;
            }
        }
    } else {
        None
    };
    let now = Utc::now();
    for meta in metas {
        let Some(name) = meta.file_stem().and_then(|s| s.to_str()).map(String::from) else {
            continue;
        };
        // Only a status line is read from the meta file, never credentials.
        let Some(until) = std::fs::read_to_string(&meta)
            .ok()
            .and_then(|t| parse_exhausted_until(&t))
        else {
            continue;
        };
        if until <= now {
            continue;
        }
        if !accounts.contains(&name) && !ctx.paths.account_file(&name).exists() {
            r.add_note(STEP, "skip", name, "no such account".to_string());
            continue;
        }
        let pools = AccountFile::load(&ctx.paths.account_file(&name))
            .map(|a| a.pools)
            .unwrap_or_default();
        let stale: Vec<String> = pools
            .iter()
            .filter(|p| {
                store
                    .as_ref()
                    .and_then(|s| s.get_flag(&name, p).ok().flatten())
                    .is_none_or(|f| f.until < until)
            })
            .cloned()
            .collect();
        if !pools.is_empty() && stale.is_empty() {
            continue; // already flagged at least this long
        }
        r.add(STEP, "flag", format!("{name} until {}", until.to_rfc3339()));
        if let (false, Some(s)) = (w.dry_run, &store)
            && let Err(e) = apply_flag(s, &name, &stale, FlagAction::Until(until), Some("migrated"))
        {
            r.error(STEP, name, &e.into());
        }
    }
}

fn sorted_children(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| rd.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    v.sort();
    v
}

/// The choice fields the old router config shares with ours; unknown keys are ignored.
fn choice_table(v: &Value) -> Result<(String, Table)> {
    let s = |k: &str| v.get(k).and_then(Value::as_str);
    let id = s("id").context("no `id`")?;
    let harness: Harness = s("harness")
        .context("no `harness`")?
        .parse()
        .map_err(|_| anyhow!("unknown harness"))?;
    let model = s("model").context("no `model`")?;
    let mut t = Table::new();
    t["id"] = value(id);
    t["harness"] = value(harness.as_str());
    t["model"] = value(model);
    if let Some(e) = s("effort") {
        t["effort"] = value(e);
    }
    let roles: Vec<&str> = v
        .get("roles")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if !roles.is_empty() {
        t["roles"] = value(roles.into_iter().collect::<Array>());
    }
    if let Some(d) = s("description") {
        t["description"] = value(d);
    }
    if let Some(en) = v.get("enabled").and_then(Value::as_bool) {
        t["enabled"] = value(en);
    }
    Ok((id.to_string(), t))
}

/// Step 6: add the old router's choices that `config.toml` does not have yet.
fn seed_choices(ctx: &Ctx, host: &HostEnv, w: &Writer, r: &mut Report) {
    const STEP: &str = "choices";
    let src = host.home.join(".config/jev-router/config.json");
    let Ok(text) = std::fs::read_to_string(&src) else {
        return;
    };
    let result = (|| -> Result<()> {
        let json: Value = serde_json::from_str(&text)
            .with_context(|| format!("{} is not valid JSON", src.display()))?;
        let list = json.get("choices").unwrap_or(&json);
        let list = list
            .as_array()
            .with_context(|| format!("{} has no choices list", src.display()))?;
        let cfg_path = ctx.paths.config_toml();
        let old = std::fs::read_to_string(&cfg_path).unwrap_or_else(|_| init::config_toml_text());
        let mut doc: DocumentMut = old.parse().context("config.toml is not valid TOML")?;
        let mut have: Vec<String> = doc
            .get("choices")
            .and_then(TomlItem::as_array_of_tables)
            .map(|a| {
                a.iter()
                    .filter_map(|t| t.get("id").and_then(TomlItem::as_str).map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let mut added = 0;
        for (n, v) in list.iter().enumerate() {
            match choice_table(v) {
                Ok((id, _)) if have.contains(&id) => {}
                Ok((id, table)) => {
                    let arr = doc
                        .entry("choices")
                        .or_insert_with(|| TomlItem::ArrayOfTables(ArrayOfTables::new()))
                        .as_array_of_tables_mut()
                        .context("`choices` is not an array of tables")?;
                    arr.push(table);
                    r.add(STEP, "create", id.clone());
                    have.push(id);
                    added += 1;
                }
                Err(e) => r.add_note(STEP, "skip", format!("entry {n}"), format!("{e:#}")),
            }
        }
        if added > 0 {
            w.write(&cfg_path, &doc.to_string())?;
        }
        Ok(())
    })();
    if let Err(e) = result {
        r.error(STEP, src.display().to_string(), &e);
    }
}

/// Step 4a: the old tooling's files and directories.
fn remove_legacy(host: &HostEnv, w: &Writer, r: &mut Report) {
    const STEP: &str = "remove";
    let h = &host.home;
    let mut targets = vec![h.join(".claude/orchestrator")];
    targets.extend(sorted_children(&h.join(".claude")).into_iter().filter(|p| {
        p.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("orchestrator-state") && n.ends_with(".json"))
    }));
    targets.extend([
        h.join(".claude/skills/coding-agent-management"),
        h.join(".local/bin/check-account-usage"),
        h.join(".local/bin/add-agy-worker"),
    ]);
    for t in targets {
        if t.symlink_metadata().is_err() {
            continue;
        }
        r.add(STEP, "remove", host.tildify(&t));
        if let Err(e) = remove_path(w, &t) {
            r.error(STEP, host.tildify(&t), &e);
        }
    }
}

/// Step 4b: the `jev-router-mcp` entries of the Codex, agy and OpenCode configs.
fn remove_jev_entries(ctx: &Ctx, host: &HostEnv, w: &Writer, r: &mut Report) {
    const STEP: &str = "remove";
    let accounts = match AccountFile::list(&ctx.paths) {
        Ok(a) => a,
        Err(e) => {
            r.error(STEP, "accounts", &e.into());
            return;
        }
    };
    let targets: Vec<FileTarget> = match install::config_files(host, &accounts) {
        Ok(t) => t,
        Err(e) => {
            r.error(STEP, "harness configs", &e);
            return;
        }
    };
    for t in targets {
        let Ok(old) = std::fs::read_to_string(&t.path) else {
            continue;
        };
        let new = match t.kind {
            Kind::Codex => codex_remove_key(&old, JEV_KEY),
            Kind::Json(k) => json_remove_key(&old, k, JEV_KEY),
        };
        let target = format!("{JEV_KEY} in {}", host.tildify(&t.path));
        match new {
            Ok(None) => {}
            Ok(Some(text)) => {
                r.add(STEP, "update", target.clone());
                if let Err(e) = w.write(&t.path, &text) {
                    r.error(STEP, target, &e);
                }
            }
            Err(e) => r.error(STEP, format!("{}", t.path.display()), &e),
        }
    }
}

/// Registers the MCP server and ships the skill, as `install` does.
fn install_step(ctx: &Ctx, w: &Writer, r: &mut Report) {
    const STEP: &str = "install";
    match install::install_steps(ctx, w, false) {
        Ok(results) => {
            for (target, res) in results {
                match res {
                    Ok(o) if o.action == "unchanged" => {}
                    Ok(o) => {
                        let path = o.path.unwrap_or_else(|| o.target.clone());
                        r.items.push(Item {
                            step: STEP,
                            action: o.action.to_string(),
                            target: format!("{} {path}", o.target),
                            note: o.note,
                        });
                    }
                    Err(e) => r.error(STEP, target, &e),
                }
            }
        }
        Err(e) => r.error(STEP, "install", &e),
    }
}

/// True when `line` names the `orchestrate` skill (not `superorchestrator`).
fn mentions_orchestrate_skill(line: &str) -> bool {
    let word = |c: Option<char>| c.is_none_or(|c| !c.is_alphanumeric() && c != '_' && c != '-');
    line.contains("skill")
        && line.match_indices("orchestrate").any(|(i, m)| {
            word(line[..i].chars().next_back()) && word(line[i + m.len()..].chars().next())
        })
}

const NEW_RULE: &str = "Delegate implementation through the superorchestrator MCP tools and skill (`route`, `spawn_agent`, `wait_agent`, `get_run`).";

/// Replaces the first line about the `orchestrate` skill with the new rule and drops later
/// duplicates; every other line is kept. `None` when nothing matches.
fn rewrite_rules(old: &str) -> Option<String> {
    let mut out = String::new();
    let mut replaced = false;
    for line in old.split_inclusive('\n') {
        let body = line.trim_end_matches('\n');
        if !mentions_orchestrate_skill(body) {
            out.push_str(line);
            continue;
        }
        if !replaced {
            replaced = true;
            let indent = &body[..body.len() - body.trim_start().len()];
            let rest = body.trim_start();
            let bullet = ["- ", "* "].into_iter().find(|b| rest.starts_with(b));
            out.push_str(indent);
            out.push_str(bullet.unwrap_or(""));
            out.push_str(NEW_RULE);
            out.push('\n');
        }
    }
    replaced.then_some(out)
}

/// A line diff of the lines `rewrite_rules` changed.
fn diff_text(path: &str, old: &str, new: &str) -> String {
    let mut out = format!("--- {path}\n+++ {path} (after migrate)\n");
    let (o, n): (Vec<&str>, Vec<&str>) = (old.lines().collect(), new.lines().collect());
    for l in o.iter().filter(|l| !n.contains(l)) {
        out.push_str(&format!("-{l}\n"));
    }
    for l in n.iter().filter(|l| !o.contains(l)) {
        out.push_str(&format!("+{l}\n"));
    }
    out
}

/// Step 5: the delegation rule in `~/.claude/CLAUDE.md`, with a diff.
fn claude_md(host: &HostEnv, w: &Writer, r: &mut Report) {
    const STEP: &str = "rules";
    let path = host.home.join(".claude/CLAUDE.md");
    let Ok(old) = std::fs::read_to_string(&path) else {
        return;
    };
    let Some(new) = rewrite_rules(&old) else {
        return;
    };
    let shown = host.tildify(&path);
    r.add(STEP, "update", shown.clone());
    r.diff = Some(diff_text(&shown, &old, &new));
    if let Err(e) = w.write(&path, &new) {
        r.error(STEP, shown, &e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_the_orchestrate_skill_only() {
        assert!(mentions_orchestrate_skill("- Use the `orchestrate` skill"));
        assert!(mentions_orchestrate_skill(
            "orchestrate skill for everything"
        ));
        assert!(!mentions_orchestrate_skill(
            "- Use the superorchestrator skill"
        ));
        assert!(!mentions_orchestrate_skill("- orchestrator skill"));
        assert!(!mentions_orchestrate_skill("- orchestrate things"));
    }

    #[test]
    fn rewrite_keeps_other_lines_and_is_idempotent() {
        let old = "- a\n  - Send work through the `orchestrate` skill\n- b\n";
        let new = rewrite_rules(old).unwrap();
        assert_eq!(new, format!("- a\n  - {NEW_RULE}\n- b\n"));
        assert_eq!(rewrite_rules(&new), None);
        assert_eq!(rewrite_rules("- nothing here\n"), None);
    }

    #[test]
    fn exhausted_until_parses() {
        assert!(parse_exhausted_until("exhausted_until=1792810402\n").is_some());
        assert!(parse_exhausted_until("x=1").is_none());
    }
}

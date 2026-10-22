//! `migrate`: cut over from the legacy tooling (SPEC 19.2).
//!
//! # How it works
//!
//! A *plan* is computed by running every step against a [`Writer`] in dry-run mode, where
//! writes, removals and backups are no-ops. `--apply` shows that plan, asks for
//! confirmation (or needs `--yes`), then **re-plans and compares** with the confirmed plan;
//! if anything changed on disk in between, nothing is touched and the new plan is shown.
//! Only then are the steps run for real. Every step checks its own precondition, so a
//! second `--apply` finds nothing to do and writes nothing (not even a backup directory).
//!
//! Registering the MCP server and skill (`install`) and creating the config dir (`init`)
//! only happen when at least one legacy artifact is found. On a clean machine `migrate`
//! says so and points at `superorchestrator install`.
//!
//! # Backups
//!
//! Everything migrate changes is first copied to `legacy-backup/<ts>/`, mirroring its
//! absolute path. Legacy trees (`~/.claude/orchestrator/`, ...) are backed up **as they
//! are**, with whatever they contain; symlinks are backed up as symlinks and never
//! followed. Credential homes are never copied: accounts are imported by linking the home,
//! and auth files are never opened. The one file not backed up is Claude Code's own
//! `~/.claude.json`, which `claude mcp add` edits itself and which holds account data.
//!
//! # The `CLAUDE.md` heuristic (SPEC 19.2 step 5)
//!
//! A line mentions the old skill when the word `orchestrate` (not `orchestrator`, not
//! `superorchestrator`; backticks and quotes ignored) is directly next to the word `skill`,
//! as in ``the `orchestrate` skill`` or `skill orchestrate`. Only the **first** such line is
//! replaced, and only when no `never`, `don't` or `do not` comes before the mention (those
//! are prohibitions that must not turn into instructions). Every other such line is left
//! unchanged and listed as a note, for the user to edit. The replaced line keeps its bullet,
//! indent and line ending (CRLF files stay CRLF).

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

/// Steps whose non-skip items mean that legacy tooling was found.
const LEGACY_STEPS: [&str; 5] = ["accounts", "cooldown", "choices", "remove", "rules"];

/// One thing the plan found, and what happens to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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
    /// Whether any legacy artifact was found.
    legacy: bool,
    /// The `legacy-backup/<ts>` directory, when this run wrote backups.
    backup: Option<PathBuf>,
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

    fn add_note(
        &mut self,
        step: &'static str,
        action: &str,
        target: impl Into<String>,
        note: impl Into<String>,
    ) {
        self.items.push(Item {
            step,
            action: action.to_string(),
            target: target.into(),
            note: Some(note.into()),
        });
    }

    fn error(&mut self, step: &'static str, target: impl Into<String>, e: &anyhow::Error) {
        self.add_note(step, "error", target, format!("{e:#}"));
    }

    fn changed(&self) -> bool {
        self.items
            .iter()
            .any(|i| !matches!(i.action.as_str(), "skip" | "unchanged"))
    }

    fn failures(&self) -> usize {
        self.items.iter().filter(|i| i.action == "error").count()
    }

    fn found_legacy(&self) -> bool {
        self.items
            .iter()
            .any(|i| LEGACY_STEPS.contains(&i.step) && i.action != "skip")
    }
}

/// True when the two plans would do the same thing.
fn same_plan(a: &Report, b: &Report) -> bool {
    a.items == b.items && a.diff == b.diff
}

pub fn run(ctx: &Ctx, apply_it: bool) -> Result<ExitCode> {
    let plan = plan(ctx)?;
    if !apply_it {
        print(ctx, &plan, true)?;
        return Ok(ExitCode::SUCCESS);
    }
    if !plan.changed() {
        print(ctx, &plan, false)?;
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
    let done = match apply(ctx, &plan)? {
        Applied::Done(r) => r,
        Applied::PlanChanged(now) => {
            let msg = "the system changed since the plan was shown, so nothing was changed; \
                       re-run to confirm the new plan";
            print_with(ctx, &now, true, Some(msg))?;
            if !ctx.json {
                eprint!("{}", render(&now, true));
            }
            bail!(msg);
        }
    };
    ctx.refresh_skill();
    print(ctx, &done, false)?;
    match done.failures() {
        0 => Ok(ExitCode::SUCCESS),
        n => Err(anyhow!("{n} step(s) failed")),
    }
}

fn render(report: &Report, dry_run: bool) -> String {
    let mut out = String::new();
    if dry_run {
        out.push_str("dry run: nothing was changed (pass --apply to execute)\n");
    }
    if !report.legacy && !report.changed() && report.failures() == 0 {
        out.push_str(
            "nothing to migrate; run `superorchestrator install` to register the MCP server and skill\n",
        );
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

fn print(ctx: &Ctx, report: &Report, dry_run: bool) -> Result<()> {
    print_with(ctx, report, dry_run, None)
}

/// Prints the report; with `error` set, `ok` is false and the JSON has an `error` field.
/// Human output is skipped for an error: the caller prints it on stderr.
fn print_with(ctx: &Ctx, report: &Report, dry_run: bool, error: Option<&str>) -> Result<()> {
    if error.is_some() && !ctx.json {
        return Ok(());
    }
    let value = json!({
        "ok": report.failures() == 0 && error.is_none(),
        "error": error,
        "dry_run": dry_run,
        "changed": report.changed(),
        "legacy": report.legacy,
        "backup_dir": report.backup,
        "items": report.items,
        "diff": report.diff,
    });
    ctx.emit(&value, |w| {
        write!(w, "{}", render(report, dry_run))?;
        if let Some(b) = &report.backup {
            writeln!(w, "\nbackups: {}", b.display())?;
        }
        Ok(())
    })
}

/// The dry-run report: what `--apply` would do right now.
fn plan(ctx: &Ctx) -> Result<Report> {
    let host = ctx.host()?;
    let cfg = ctx.config()?;
    let w = Writer::new(&ctx.paths, true);
    let mut r = Report::default();
    steps(ctx, &cfg, &host, &w, &mut r);
    r.legacy = r.found_legacy();
    if r.legacy {
        if !ctx.paths.config_toml().exists() {
            r.items.insert(
                0,
                Item {
                    step: "init",
                    action: "create".into(),
                    target: host.tildify(&ctx.paths.config_dir),
                    note: None,
                },
            );
        }
        install_step(ctx, &host, &w, &mut r);
    }
    Ok(r)
}

enum Applied {
    Done(Report),
    /// The system changed since the confirmed plan; nothing was done. Carries the new plan.
    PlanChanged(Report),
}

/// Re-plans, stops when the plan differs from the confirmed one, then executes.
fn apply(ctx: &Ctx, confirmed: &Report) -> Result<Applied> {
    let now = plan(ctx)?;
    if !same_plan(&now, confirmed) {
        return Ok(Applied::PlanChanged(now));
    }
    let host = ctx.host()?;
    let w = Writer::new(&ctx.paths, false);
    let mut r = Report {
        legacy: now.legacy,
        ..Report::default()
    };
    if now.legacy {
        step_init(ctx, &host, &mut r);
    }
    let cfg = ctx.config()?;
    steps(ctx, &cfg, &host, &w, &mut r);
    if now.legacy {
        install_step(ctx, &host, &w, &mut r);
    }
    if w.backup_root().exists() {
        r.backup = Some(w.backup_root().to_path_buf());
    }
    Ok(Applied::Done(r))
}

fn step_init(ctx: &Ctx, host: &HostEnv, r: &mut Report) {
    let target = host.tildify(&ctx.paths.config_dir);
    match init::init(&ctx.paths) {
        Ok(done) => {
            if done.iter().any(|(_, o)| *o == init::Outcome::Created) {
                r.add("init", "create", target);
            }
        }
        Err(e) => r.error("init", target, &e),
    }
}

/// SPEC 19.2 steps 2 to 6, against `w`.
fn steps(ctx: &Ctx, cfg: &Config, host: &HostEnv, w: &Writer, r: &mut Report) {
    let accounts = import_accounts(ctx, cfg, host, w, r);
    cooldowns(ctx, host, w, &accounts, r);
    seed_choices(ctx, host, w, r);
    remove_legacy(host, w, r);
    remove_jev_entries(ctx, host, w, r);
    claude_md(host, w, r);
}

/// Copies `path` into the backup tree: a file as is, a directory recursively, a symlink
/// as a symlink (never followed). Returns the entries that could not be backed up because
/// they are neither file, directory nor link. Does nothing in a dry run.
fn backup_tree(w: &Writer, path: &Path) -> Result<Vec<PathBuf>> {
    let mut skipped = Vec::new();
    if w.dry_run {
        return Ok(skipped);
    }
    let meta = std::fs::symlink_metadata(path)
        .with_context(|| format!("could not stat {}", path.display()))?;
    if meta.is_symlink() {
        let dest = w.backup_path(path);
        if let Some(dir) = dest.parent() {
            install::create_private_dirs(dir)?;
        }
        if dest.symlink_metadata().is_err() {
            let target = std::fs::read_link(path)?;
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
            skipped.extend(backup_tree(w, &k)?);
        }
    } else if meta.is_file() {
        w.backup(path)?;
    } else {
        skipped.push(path.to_path_buf());
    }
    Ok(skipped)
}

/// Backs up and removes a file, link or directory tree (the parent stays). Returns the
/// entries that were removed without a backup. Does nothing in a dry run.
fn remove_path(w: &Writer, path: &Path) -> Result<Vec<PathBuf>> {
    if w.dry_run {
        return Ok(Vec::new());
    }
    let skipped = backup_tree(w, path)?;
    let meta = std::fs::symlink_metadata(path)?;
    if meta.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
    .with_context(|| format!("could not remove {}", path.display()))?;
    Ok(skipped)
}

/// Step 2: import the scanned accounts and rewrite their wrappers. Returns the names of
/// the accounts that exist (or, in a dry run, will exist) afterwards.
fn import_accounts(
    ctx: &Ctx,
    cfg: &Config,
    host: &HostEnv,
    w: &Writer,
    r: &mut Report,
) -> Vec<String> {
    const STEP: &str = "accounts";
    let all = scan(host);
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
        let result = (|| -> Result<Option<(&'static str, String)>> {
            if !is_new && !foreign && !missing {
                return Ok(None);
            }
            if foreign {
                // `write_wrapper` also parks the old file in `wrappers.bak`.
                backup_tree(w, &wrapper)?;
            }
            if is_new {
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
                Ok(Some(("import", desc.clone())))
            } else {
                if !w.dry_run {
                    write_wrapper(&wdir, &ctx.paths.wrappers_bak_dir(), &c.name, SELF_EXE)?;
                }
                Ok(Some((
                    "rewrite",
                    format!("wrapper {}", host.tildify(&wrapper)),
                )))
            }
        })();
        match result {
            Ok(done) => {
                if let Some((action, target)) = done {
                    r.add(STEP, action, target);
                }
                present.push(c.name.clone());
            }
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
                r.error(STEP, host.tildify(&db), &e.into());
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
        let file = ctx.paths.account_file(&name);
        // `None` pools: the account is about to be imported (dry run only).
        let pools: Option<Vec<String>> = if file.exists() {
            match AccountFile::load(&file) {
                Ok(a) => Some(a.pools),
                Err(e) => {
                    r.error(STEP, name, &e.into());
                    continue;
                }
            }
        } else if accounts.contains(&name) && w.dry_run {
            None
        } else {
            r.add_note(STEP, "skip", name, "no such account");
            continue;
        };
        if pools.as_ref().is_some_and(Vec::is_empty) {
            r.add_note(STEP, "skip", name, "no pools");
            continue;
        }
        let stale: Vec<String> = pools
            .iter()
            .flatten()
            .filter(|p| {
                store
                    .as_ref()
                    .and_then(|s| s.get_flag(&name, p).ok().flatten())
                    .is_none_or(|f| f.until < until)
            })
            .cloned()
            .collect();
        if pools.is_some() && stale.is_empty() {
            continue; // already flagged at least this long
        }
        let target = format!("{name} until {}", until.to_rfc3339());
        if let (false, Some(s)) = (w.dry_run, &store) {
            let flagged = apply_flag(s, &name, &stale, FlagAction::Until(until), Some("migrated"));
            if let Err(e) = flagged {
                r.error(STEP, name, &e.into());
                continue;
            }
        }
        r.add(STEP, "flag", target);
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

/// What merging the old choices into `config.toml` text produced.
#[derive(Debug, Default)]
struct Merged {
    /// The new text; `None` when nothing was added.
    text: Option<String>,
    added: Vec<String>,
    /// `(entry description, reason)` for entries that were not usable.
    skipped: Vec<(String, String)>,
}

/// Appends the choices of `list` that `base` does not have. The result must parse and
/// validate as a [`Config`], otherwise nothing is produced.
fn merge_choices(base: &str, list: &[Value], path: &Path) -> Result<Merged> {
    let mut doc: DocumentMut = base.parse().context("config.toml is not valid TOML")?;
    let mut have: Vec<String> = doc
        .get("choices")
        .and_then(TomlItem::as_array_of_tables)
        .map(|a| {
            a.iter()
                .filter_map(|t| t.get("id").and_then(TomlItem::as_str).map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let mut out = Merged::default();
    for (n, v) in list.iter().enumerate() {
        match choice_table(v) {
            Ok((id, _)) if have.contains(&id) => {}
            Ok((id, table)) => {
                doc.entry("choices")
                    .or_insert_with(|| TomlItem::ArrayOfTables(ArrayOfTables::new()))
                    .as_array_of_tables_mut()
                    .context("`choices` is not an array of tables")?
                    .push(table);
                have.push(id.clone());
                out.added.push(id);
            }
            Err(e) => out.skipped.push((format!("entry {n}"), format!("{e:#}"))),
        }
    }
    if !out.added.is_empty() {
        let text = doc.to_string();
        Config::parse(&text, path).context("the merged choices make config.toml invalid")?;
        out.text = Some(text);
    }
    Ok(out)
}

/// Step 6: add the old router's choices that `config.toml` does not have yet.
fn seed_choices(ctx: &Ctx, host: &HostEnv, w: &Writer, r: &mut Report) {
    const STEP: &str = "choices";
    let src = host.home.join(".config/jev-router/config.json");
    let Ok(text) = std::fs::read_to_string(&src) else {
        return;
    };
    let shown = host.tildify(&src);
    let cfg_path = ctx.paths.config_toml();
    let result = (|| -> Result<Merged> {
        let json: Value =
            serde_json::from_str(&text).with_context(|| format!("{shown} is not valid JSON"))?;
        let list = json.get("choices").unwrap_or(&json);
        let list = list
            .as_array()
            .with_context(|| format!("{shown} has no choices list"))?;
        let base = match std::fs::read_to_string(&cfg_path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => init::config_toml_text(),
            Err(e) => {
                return Err(anyhow!(e).context(format!("could not read {}", cfg_path.display())));
            }
        };
        let merged = merge_choices(&base, list, &cfg_path)?;
        if let Some(t) = &merged.text {
            w.write(&cfg_path, t)?;
        }
        Ok(merged)
    })();
    match result {
        Ok(m) => {
            for id in m.added {
                r.add(STEP, "create", id);
            }
            for (entry, why) in m.skipped {
                r.add_note(STEP, "skip", entry, why);
            }
        }
        Err(e) => r.error(STEP, shown, &e),
    }
}

/// `orchestrator-state.json` or `orchestrator-state-<digits>.json`.
fn is_state_file_name(n: &str) -> bool {
    let Some(rest) = n
        .strip_prefix("orchestrator-state")
        .and_then(|r| r.strip_suffix(".json"))
    else {
        return false;
    };
    rest.is_empty()
        || rest
            .strip_prefix('-')
            .is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()))
}

/// Step 4a: the old tooling's files and directories.
fn remove_legacy(host: &HostEnv, w: &Writer, r: &mut Report) {
    const STEP: &str = "remove";
    let h = &host.home;
    let mut targets = vec![h.join(".claude/orchestrator")];
    targets.extend(sorted_children(&h.join(".claude")).into_iter().filter(|p| {
        p.symlink_metadata().is_ok_and(|m| m.is_file())
            && p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(is_state_file_name)
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
        let shown = host.tildify(&t);
        match remove_path(w, &t) {
            Ok(unsaved) => {
                r.add(STEP, "remove", shown);
                for p in unsaved {
                    r.add_note(
                        STEP,
                        "skip",
                        host.tildify(&p),
                        "not a file, directory or link; removed without a backup",
                    );
                }
            }
            Err(e) => r.error(STEP, shown, &e),
        }
    }
}

/// True when `mcp_servers` is an inline table holding `key`, which the line-preserving
/// editor cannot remove.
fn codex_inline_entry(text: &str, key: &str) -> bool {
    text.parse::<DocumentMut>().is_ok_and(|d| {
        d.get("mcp_servers")
            .and_then(TomlItem::as_inline_table)
            .is_some_and(|t| t.contains_key(key))
    })
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
        let shown = host.tildify(&t.path);
        let target = format!("{JEV_KEY} in {shown}");
        if matches!(t.kind, Kind::Codex) && codex_inline_entry(&old, JEV_KEY) {
            r.add_note(
                STEP,
                "skip",
                target,
                "`mcp_servers` is an inline table; remove the entry by hand",
            );
            continue;
        }
        let new = match t.kind {
            Kind::Codex => codex_remove_key(&old, JEV_KEY),
            Kind::Json(k) => json_remove_key(&old, k, JEV_KEY),
        };
        match new {
            Ok(None) => {}
            Ok(Some(text)) => match w.write(&t.path, &text) {
                Ok(()) => r.add(STEP, "update", target),
                Err(e) => r.error(STEP, target, &e),
            },
            Err(e) => r.error(STEP, shown, &e),
        }
    }
}

/// Registers the MCP server and ships the skill, as `install` does.
fn install_step(ctx: &Ctx, host: &HostEnv, w: &Writer, r: &mut Report) {
    const STEP: &str = "install";
    match install::install_steps(ctx, w, false, false) {
        Ok(results) => {
            for (target, res) in results {
                match res {
                    Ok(o) if o.action == "unchanged" => {}
                    Ok(o) => {
                        let path = o.path.as_deref().map_or(String::new(), |p| {
                            format!(" {}", host.tildify(Path::new(p)))
                        });
                        r.items.push(Item {
                            step: STEP,
                            action: o.action.to_string(),
                            target: format!("{}{path}", o.target),
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

/// How a line relates to the old `orchestrate` skill.
#[derive(Debug, PartialEq, Eq)]
enum Mention {
    None,
    /// A reference that can be rewritten.
    Rule,
    /// A reference after a prohibition (`never`, `don't`, `do not`).
    Prohibition,
}

fn is_word_char(c: Option<char>) -> bool {
    c.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '-')
}

/// True when `text` (lower case, quotes removed) has the word `never` or `dont`, or the
/// words `do not`.
fn has_prohibition(text: &str) -> bool {
    let words: Vec<&str> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    words.iter().any(|w| matches!(*w, "never" | "dont"))
        || words.windows(2).any(|p| p == ["do", "not"])
}

/// Classifies `line` per the heuristic in the module docs.
fn mention(line: &str) -> Mention {
    let n: String = line
        .chars()
        .filter(|c| !matches!(c, '`' | '"' | '\'' | '\u{2019}'))
        .collect::<String>()
        .to_ascii_lowercase();
    for (i, m) in n.match_indices("orchestrate") {
        let (before, after) = (&n[..i], &n[i + m.len()..]);
        if is_word_char(before.chars().next_back()) || is_word_char(after.chars().next()) {
            continue;
        }
        let skill_after = after
            .trim_start()
            .strip_prefix("skill")
            .is_some_and(|r| !is_word_char(r.chars().next()));
        let skill_before = before
            .trim_end()
            .strip_suffix("skill")
            .is_some_and(|r| !is_word_char(r.chars().next_back()));
        if !(skill_after || skill_before) {
            continue;
        }
        return if has_prohibition(before) {
            Mention::Prohibition
        } else {
            Mention::Rule
        };
    }
    Mention::None
}

const NEW_RULE: &str = "Delegate implementation through the superorchestrator MCP tools and skill (`route`, `spawn_agent`, `wait_agent`, `get_run`).";

/// The rewritten text (when a rule was replaced) and notes `(line number, text)` for the
/// other lines that mention the skill.
#[derive(Debug, Default, PartialEq, Eq)]
struct Rewrite {
    text: Option<String>,
    notes: Vec<(usize, &'static str)>,
}

fn rewrite_rules(old: &str) -> Rewrite {
    let mut out = String::new();
    let mut rw = Rewrite::default();
    // Already migrated: the rule is in place, so further mentions are only noted.
    let mut replaced = old.lines().any(|l| l.contains(NEW_RULE));
    let mut rewrote = false;
    for (idx, line) in old.split_inclusive('\n').enumerate() {
        let (body, eol) = if let Some(b) = line.strip_suffix("\r\n") {
            (b, "\r\n")
        } else if let Some(b) = line.strip_suffix('\n') {
            (b, "\n")
        } else {
            (line, "")
        };
        match mention(body) {
            Mention::Rule if !replaced => {
                replaced = true;
                rewrote = true;
                let indent = &body[..body.len() - body.trim_start().len()];
                let rest = body.trim_start();
                let bullet = ["- ", "* "].into_iter().find(|b| rest.starts_with(b));
                out.push_str(&format!("{indent}{}{NEW_RULE}{eol}", bullet.unwrap_or("")));
            }
            Mention::Rule => {
                rw.notes.push((
                    idx + 1,
                    "also mentions the orchestrate skill; edit it by hand",
                ));
                out.push_str(line);
            }
            Mention::Prohibition => {
                rw.notes.push((
                    idx + 1,
                    "a prohibition about the orchestrate skill; left unchanged, edit it by hand if needed",
                ));
                out.push_str(line);
            }
            Mention::None => out.push_str(line),
        }
    }
    rw.text = rewrote.then_some(out);
    rw
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
    let shown = host.tildify(&path);
    let rw = rewrite_rules(&old);
    for (line, note) in &rw.notes {
        r.add_note(STEP, "skip", format!("{shown} line {line}"), *note);
    }
    let Some(new) = rw.text else {
        return;
    };
    match w.write(&path, &new) {
        Ok(()) => {
            r.add(STEP, "update", shown.clone());
            r.diff = Some(diff_text(&shown, &old, &new));
        }
        Err(e) => r.error(STEP, shown, &e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mention_needs_the_skill_next_to_the_word() {
        assert_eq!(mention("- Use the `orchestrate` skill"), Mention::Rule);
        assert_eq!(mention("orchestrate skill for everything"), Mention::Rule);
        assert_eq!(mention("- the skill \"orchestrate\" rules"), Mention::Rule);
        assert_eq!(mention("- Use the superorchestrator skill"), Mention::None);
        assert_eq!(mention("- orchestrator skill"), Mention::None);
        assert_eq!(mention("- orchestrate things"), Mention::None);
        assert_eq!(mention("- the orchestrate-state skill"), Mention::None);
        assert_eq!(
            mention("- Never use the orchestrate skill"),
            Mention::Prohibition
        );
        assert_eq!(
            mention("- Don't use the `orchestrate` skill"),
            Mention::Prohibition
        );
        assert_eq!(
            mention("- do not call the orchestrate skill"),
            Mention::Prohibition
        );
    }

    #[test]
    fn prohibitions_are_whole_words_and_case_is_ignored() {
        assert_eq!(
            mention("- Whenever coding, use the orchestrate skill"),
            Mention::Rule
        );
        assert_eq!(mention("- Orchestrate skill for all work"), Mention::Rule);
        assert_eq!(
            mention("- Use the ORCHESTRATE skill, nevertheless"),
            Mention::Rule
        );
        assert_eq!(
            mention("- NEVER use the Orchestrate skill"),
            Mention::Prohibition
        );
        assert_eq!(
            mention("- We do   not use the orchestrate skill"),
            Mention::Prohibition
        );
        assert_eq!(
            mention("- Do nothing but use the orchestrate skill"),
            Mention::Rule
        );
    }

    #[test]
    fn rewrite_keeps_other_lines_endings_and_is_idempotent() {
        let old = "- a\r\n  - Send work through the `orchestrate` skill\r\n- b\r\n";
        let rw = rewrite_rules(old);
        assert_eq!(
            rw.text.as_deref(),
            Some(format!("- a\r\n  - {NEW_RULE}\r\n- b\r\n").as_str())
        );
        assert!(rw.notes.is_empty());
        assert_eq!(rewrite_rules(&rw.text.unwrap()), Rewrite::default());
        assert_eq!(rewrite_rules("- nothing here\n"), Rewrite::default());
    }

    #[test]
    fn rewrite_replaces_only_the_first_rule_and_notes_the_rest() {
        let old = "- Use the orchestrate skill\n- Never use the orchestrate skill\n- Then the orchestrate skill again\nlast";
        let rw = rewrite_rules(old);
        let new = rw.text.unwrap();
        assert!(new.starts_with("- Delegate implementation"));
        assert!(new.ends_with("skill again\nlast"));
        assert_eq!(rw.notes.iter().map(|n| n.0).collect::<Vec<_>>(), [2, 3]);
    }

    #[test]
    fn exhausted_until_parses() {
        assert!(parse_exhausted_until("exhausted_until=1792810402\n").is_some());
        assert!(parse_exhausted_until("x=1").is_none());
    }

    #[test]
    fn state_file_names() {
        assert!(is_state_file_name("orchestrator-state.json"));
        assert!(is_state_file_name("orchestrator-state-12.json"));
        for bad in [
            "orchestrator-state-.json",
            "orchestrator-state-x.json",
            "orchestrator-state.json.bak",
            "orchestrator-state2.json",
        ] {
            assert!(!is_state_file_name(bad), "{bad}");
        }
    }

    fn report(items: &[(&'static str, &str)], diff: Option<&str>) -> Report {
        let mut r = Report::default();
        for (step, target) in items {
            r.add(step, "remove", *target);
        }
        r.diff = diff.map(String::from);
        r
    }

    #[test]
    fn plans_are_compared_by_items_and_diff() {
        let a = report(&[("remove", "x")], None);
        assert!(same_plan(&a, &report(&[("remove", "x")], None)));
        assert!(!same_plan(
            &a,
            &report(&[("remove", "x"), ("remove", "y")], None)
        ));
        assert!(!same_plan(&a, &report(&[("remove", "z")], None)));
        assert!(!same_plan(&a, &report(&[("remove", "x")], Some("d"))));
    }

    #[test]
    fn merged_choices_must_leave_a_valid_config() {
        let path = Path::new("config.toml");
        let list = [json!({"id": "n", "harness": "codex", "model": "m"})];
        let ok = merge_choices("", &list, path).unwrap();
        assert_eq!(ok.added, ["n"]);
        let text = ok.text.unwrap();
        assert_eq!(merge_choices(&text, &list, path).unwrap().text, None);
        // An already-invalid config is never extended.
        assert!(merge_choices("limits = \"x\"\n", &list, path).is_err());
        // Unusable entries are skipped, not fatal.
        let bad = [
            json!({"nope": 1}),
            json!({"id": "z", "harness": "zzz", "model": "m"}),
        ];
        let m = merge_choices("", &bad, path).unwrap();
        assert_eq!(m.skipped.len(), 2);
        assert_eq!(m.text, None);
    }

    #[test]
    fn inline_mcp_servers_are_detected() {
        assert!(codex_inline_entry(
            "mcp_servers = { jev-router-mcp = { command = \"x\" } }\n",
            JEV_KEY
        ));
        assert!(!codex_inline_entry(
            "[mcp_servers.jev-router-mcp]\ncommand = \"x\"\n",
            JEV_KEY
        ));
    }
}

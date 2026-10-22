//! `install` and `uninstall`: register the MCP server with every harness and ship the
//! skill (SPEC 19.1).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, anyhow};
use chrono::Utc;
use serde::Serialize;
use so_accounts::HostEnv;
use so_core::{AccountFile, Harness, Paths};

use crate::ctx::Ctx;
use crate::register::{self, JsonKind, NAME};
use crate::skill::{MARKER, render_skill};

#[derive(Debug, Serialize)]
struct Outcome {
    target: String,
    action: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
}

impl Outcome {
    fn new(target: &str, action: &'static str, path: Option<&Path>, note: Option<String>) -> Self {
        Self {
            target: target.to_string(),
            action,
            path: path.map(|p| p.display().to_string()),
            note,
        }
    }
}

/// One config file to edit.
struct FileTarget {
    label: String,
    path: PathBuf,
    kind: Kind,
    /// The directory that must exist for the file to be created.
    needs_dir: PathBuf,
}

#[derive(Clone, Copy)]
enum Kind {
    Codex,
    Json(JsonKind),
}

fn config_files(host: &HostEnv, accounts: &[AccountFile]) -> Result<Vec<FileTarget>> {
    let mut out = vec![
        FileTarget {
            label: "codex".into(),
            path: host.home.join(".codex/config.toml"),
            kind: Kind::Codex,
            needs_dir: host.home.join(".codex"),
        },
        FileTarget {
            label: "opencode".into(),
            path: xdg_config_home(host).join("opencode/opencode.json"),
            kind: Kind::Json(JsonKind::Opencode),
            needs_dir: xdg_config_home(host).join("opencode"),
        },
        FileTarget {
            label: "agy".into(),
            path: host.home.join(".gemini/config/mcp_config.json"),
            kind: Kind::Json(JsonKind::Agy),
            needs_dir: host.home.join(".gemini"),
        },
    ];
    for a in accounts {
        let home = host.expand(&a.home)?;
        let label = format!("{} ({})", a.harness.as_str(), a.name);
        match a.harness {
            Harness::Codex => out.push(FileTarget {
                label,
                path: home.join("config.toml"),
                kind: Kind::Codex,
                needs_dir: home,
            }),
            Harness::Opencode => out.push(FileTarget {
                label,
                path: home.join("config/opencode/opencode.json"),
                kind: Kind::Json(JsonKind::Opencode),
                needs_dir: home,
            }),
            Harness::Agy => out.push(FileTarget {
                label,
                path: home.join(".gemini/config/mcp_config.json"),
                kind: Kind::Json(JsonKind::Agy),
                needs_dir: home.join(".gemini"),
            }),
            // Claude is registered through `claude mcp add --scope user`.
            Harness::Claude => {}
        }
    }
    let mut seen = std::collections::HashSet::new();
    out.retain(|t| seen.insert(t.path.clone()));
    Ok(out)
}

fn xdg_config_home(host: &HostEnv) -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map_or_else(|| host.home.join(".config"), PathBuf::from)
}

fn skill_files(host: &HostEnv) -> Vec<PathBuf> {
    [".claude", ".agents"]
        .iter()
        .map(|d| host.home.join(d).join("skills").join(NAME).join("SKILL.md"))
        .collect()
}

/// Writes files with a backup of what they replace.
struct Writer<'a> {
    dry_run: bool,
    backup_root: PathBuf,
    paths: &'a Paths,
}

impl<'a> Writer<'a> {
    fn new(paths: &'a Paths, dry_run: bool) -> Self {
        let ts = Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
        Self {
            dry_run,
            backup_root: paths.legacy_backup_dir(&ts),
            paths,
        }
    }

    fn backup(&self, path: &Path) -> Result<()> {
        let rel = path.strip_prefix("/").unwrap_or(path);
        let dest = self.backup_root.join(rel);
        if let Some(dir) = dest.parent() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("could not create {}", dir.display()))?;
        }
        std::fs::copy(path, &dest)
            .with_context(|| format!("could not back up {}", path.display()))?;
        let _ = self.paths; // backups live under the config dir
        Ok(())
    }

    /// Replaces `path` with `text` (backing up the old file first).
    fn write(&self, path: &Path, text: &str) -> Result<()> {
        if self.dry_run {
            return Ok(());
        }
        if path.exists() {
            self.backup(path)?;
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("could not create {}", dir.display()))?;
        }
        std::fs::write(path, text).with_context(|| format!("could not write {}", path.display()))
    }

    fn remove_file(&self, path: &Path) -> Result<()> {
        if self.dry_run {
            return Ok(());
        }
        self.backup(path)?;
        std::fs::remove_file(path)?;
        if let Some(dir) = path.parent() {
            // Only succeeds when the directory is empty.
            let _ = std::fs::remove_dir(dir);
        }
        Ok(())
    }
}

fn edit_file(w: &Writer<'_>, t: &FileTarget, exe: &str, add: bool) -> Result<Outcome> {
    let exists = t.path.exists();
    if !exists && !add {
        return Ok(Outcome::new(&t.label, "unchanged", Some(&t.path), None));
    }
    if !exists && !t.needs_dir.is_dir() {
        return Ok(Outcome::new(
            &t.label,
            "skip",
            Some(&t.path),
            Some(format!("{} does not exist", t.needs_dir.display())),
        ));
    }
    let old = if exists {
        std::fs::read_to_string(&t.path)
            .with_context(|| format!("could not read {}", t.path.display()))?
    } else {
        String::new()
    };
    let new = match (t.kind, add) {
        (Kind::Codex, true) => register::codex_add(&old, exe),
        (Kind::Codex, false) => register::codex_remove(&old),
        (Kind::Json(k), true) => register::json_add(&old, k, exe),
        (Kind::Json(k), false) => register::json_remove(&old, k),
    }
    .with_context(|| format!("{}", t.path.display()))?;
    match new {
        None => Ok(Outcome::new(&t.label, "unchanged", Some(&t.path), None)),
        Some(text) => {
            w.write(&t.path, &text)?;
            let action = match (add, exists) {
                (false, _) => "remove",
                (true, true) => "update",
                (true, false) => "create",
            };
            Ok(Outcome::new(&t.label, action, Some(&t.path), None))
        }
    }
}

fn edit_skill(w: &Writer<'_>, path: &Path, content: Option<&str>) -> Result<Outcome> {
    let existing = std::fs::read_to_string(path).ok();
    if let Some(old) = &existing
        && !old.contains(MARKER)
    {
        return Ok(Outcome::new(
            "skill",
            "skip",
            Some(path),
            Some("exists and is not managed by superorchestrator".into()),
        ));
    }
    match (content, existing) {
        (Some(new), Some(old)) if old == new => {
            Ok(Outcome::new("skill", "unchanged", Some(path), None))
        }
        (Some(new), old) => {
            w.write(path, new)?;
            let action = if old.is_some() { "update" } else { "create" };
            Ok(Outcome::new("skill", action, Some(path), None))
        }
        (None, Some(_)) => {
            w.remove_file(path)?;
            Ok(Outcome::new("skill", "remove", Some(path), None))
        }
        (None, None) => Ok(Outcome::new("skill", "unchanged", Some(path), None)),
    }
}

/// Runs `claude` with `args`; `None` when it is not on `PATH`.
fn claude(args: &[&str]) -> Result<Option<std::process::Output>> {
    match Command::new("claude")
        .args(args)
        .stdin(Stdio::null())
        .output()
    {
        Ok(o) => Ok(Some(o)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(anyhow!("could not run claude: {e}")),
    }
}

fn claude_failure(what: &str, o: &std::process::Output) -> anyhow::Error {
    let err = String::from_utf8_lossy(&o.stderr);
    anyhow!("`claude mcp {what}` failed: {}", err.trim())
}

fn claude_install(exe: &str, force: bool, dry_run: bool) -> Result<Outcome> {
    let label = "claude";
    let Some(get) = claude(&["mcp", "get", NAME])? else {
        return Ok(Outcome::new(
            label,
            "skip",
            None,
            Some("claude is not on PATH".into()),
        ));
    };
    let registered = get.status.success();
    if registered && !force && String::from_utf8_lossy(&get.stdout).contains(exe) {
        return Ok(Outcome::new(label, "unchanged", None, None));
    }
    let action = if registered { "update" } else { "create" };
    if !dry_run {
        if registered {
            let rm = claude(&["mcp", "remove", "--scope", "user", NAME])?
                .ok_or_else(|| anyhow!("claude disappeared"))?;
            // A non-user-scope entry is not ours to remove; the add below still proceeds.
            if !rm.status.success() {
                tracing::debug!(
                    "claude mcp remove failed: {}",
                    claude_failure("remove", &rm)
                );
            }
        }
        let add = claude(&["mcp", "add", "--scope", "user", NAME, "--", exe, "mcp"])?
            .ok_or_else(|| anyhow!("claude disappeared"))?;
        if !add.status.success() {
            return Err(claude_failure("add", &add));
        }
    }
    Ok(Outcome::new(label, action, None, None))
}

fn claude_uninstall(dry_run: bool) -> Result<Outcome> {
    let label = "claude";
    let Some(get) = claude(&["mcp", "get", NAME])? else {
        return Ok(Outcome::new(
            label,
            "skip",
            None,
            Some("claude is not on PATH".into()),
        ));
    };
    if !get.status.success() {
        return Ok(Outcome::new(label, "unchanged", None, None));
    }
    if !dry_run {
        let rm = claude(&["mcp", "remove", "--scope", "user", NAME])?
            .ok_or_else(|| anyhow!("claude disappeared"))?;
        if !rm.status.success() {
            return Err(claude_failure("remove", &rm));
        }
    }
    Ok(Outcome::new(label, "remove", None, None))
}

fn print(ctx: &Ctx, outcomes: &[Outcome], dry_run: bool) -> Result<()> {
    ctx.emit(&outcomes, |w| {
        if dry_run {
            writeln!(w, "dry run: nothing was changed")?;
        }
        for o in outcomes {
            let mut line = format!("{:<9} {}", o.action, o.target);
            if let Some(p) = &o.path {
                line.push_str(&format!("  {p}"));
            }
            if let Some(n) = &o.note {
                line.push_str(&format!("  ({n})"));
            }
            writeln!(w, "{line}")?;
        }
        Ok(())
    })
}

fn finish(ctx: &Ctx, results: Vec<Result<Outcome>>, dry_run: bool) -> Result<()> {
    let mut outcomes = Vec::new();
    let mut failures = 0usize;
    for r in results {
        match r {
            Ok(o) => outcomes.push(o),
            Err(e) => {
                failures += 1;
                outcomes.push(Outcome::new("error", "error", None, Some(format!("{e:#}"))));
            }
        }
    }
    print(ctx, &outcomes, dry_run)?;
    if failures > 0 {
        return Err(anyhow!("{failures} step(s) failed"));
    }
    Ok(())
}

pub fn install(ctx: &Ctx, force: bool, dry_run: bool) -> Result<()> {
    let host = ctx.host()?;
    let exe = std::env::current_exe().context("could not find this executable")?;
    let exe = exe.to_string_lossy().into_owned();
    let accounts = AccountFile::list(&ctx.paths)?;
    let choices = ctx.config().map(|c| c.choices).unwrap_or_default();
    let skill = render_skill(&accounts, &choices);
    let w = Writer::new(&ctx.paths, dry_run);

    let mut results = vec![claude_install(&exe, force, dry_run)];
    for t in config_files(&host, &accounts)? {
        results.push(edit_file(&w, &t, &exe, true));
    }
    for p in skill_files(&host) {
        results.push(edit_skill(&w, &p, Some(&skill)));
    }
    finish(ctx, results, dry_run)
}

pub fn uninstall(ctx: &Ctx, dry_run: bool) -> Result<()> {
    let host = ctx.host()?;
    let accounts = AccountFile::list(&ctx.paths)?;
    let w = Writer::new(&ctx.paths, dry_run);

    let mut results = vec![claude_uninstall(dry_run)];
    for t in config_files(&host, &accounts)? {
        results.push(edit_file(&w, &t, "", false));
    }
    for p in skill_files(&host) {
        results.push(edit_skill(&w, &p, None));
    }
    finish(ctx, results, dry_run)
}

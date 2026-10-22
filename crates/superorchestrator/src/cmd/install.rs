//! `install` and `uninstall`: register the MCP server with every harness and ship the
//! skill (SPEC 19.1).

use std::io::Write as _;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use chrono::Utc;
use serde::Serialize;
use so_accounts::{HostEnv, resolve_binary};
use so_core::{AccountFile, Harness, Paths};

use crate::ctx::Ctx;
use crate::register::{self, JsonKind, NAME};
use crate::skill::{MARKER, render_skill};

#[derive(Debug, Serialize)]
pub(crate) struct Outcome {
    pub(crate) target: String,
    pub(crate) action: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) note: Option<String>,
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
pub(crate) struct FileTarget {
    pub(crate) label: String,
    pub(crate) path: PathBuf,
    pub(crate) kind: Kind,
    /// The directory that must exist for the file to be created.
    needs_dir: PathBuf,
}

#[derive(Clone, Copy)]
pub(crate) enum Kind {
    Codex,
    Json(JsonKind),
}

pub(crate) fn config_files(host: &HostEnv, accounts: &[AccountFile]) -> Result<Vec<FileTarget>> {
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
        // The default opencode home uses the real XDG config, covered by the global entry.
        if a.harness == Harness::Opencode
            && same_dir(&home, &host.home.join(".local/share/opencode"))
        {
            continue;
        }
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
            // Claude's MCP registration is user-scope and global (`~/.claude.json`), so
            // no account home needs an entry of its own.
            Harness::Claude => {}
        }
    }
    // Dedupe by resolved path (an account may point at a default home or a symlink).
    let mut seen = std::collections::HashSet::new();
    out.retain(|t| seen.insert(resolve(&t.path)));
    Ok(out)
}

/// Canonical path, resolving the nearest existing ancestor when the file is missing.
fn resolve(p: &Path) -> PathBuf {
    if let Ok(c) = p.canonicalize() {
        return c;
    }
    match (p.parent(), p.file_name()) {
        (Some(parent), Some(name)) => resolve(parent).join(name),
        _ => p.to_path_buf(),
    }
}

fn same_dir(a: &Path, b: &Path) -> bool {
    resolve(a) == resolve(b)
}

fn xdg_config_home(host: &HostEnv) -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map_or_else(|| host.home.join(".config"), PathBuf::from)
}

fn skill_files(host: &HostEnv) -> Vec<PathBuf> {
    so_agents::skill::skill_files(&host.home)
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Private directories (0700), created recursively.
pub(crate) fn create_private_dirs(dir: &Path) -> Result<()> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .with_context(|| format!("could not create {}", dir.display()))
}

/// Writes `bytes` over `path` atomically: a temp file next to the target (`create_new`),
/// the original's mode (0600 for a new file), fsync, then rename. A symlink is followed so
/// the link itself survives.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let dir = target.parent().unwrap_or_else(|| Path::new("."));
    create_private_dirs(dir)?;
    let mode = std::fs::metadata(&target).map_or(0o600, |m| m.mode() & 0o7777);
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp = dir.join(format!(".{name}.tmp.{}.{n}", std::process::id()));
    let result = (|| -> std::io::Result<()> {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&tmp)?;
        // The umask may have cut the mode at creation.
        f.set_permissions(std::os::unix::fs::PermissionsExt::from_mode(mode))?;
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, &target)
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(anyhow!(e).context(format!("could not write {}", path.display())));
    }
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

/// Backup directory name: sortable timestamp, zero-padded millis, pid.
fn backup_id(now: chrono::DateTime<Utc>, pid: u32) -> String {
    format!(
        "{}-{:03}-{pid}",
        now.format("%Y%m%dT%H%M%SZ"),
        now.timestamp_subsec_millis()
    )
}

/// Writes files with a backup of what they replace.
pub(crate) struct Writer {
    pub(crate) dry_run: bool,
    backup_root: PathBuf,
}

impl Writer {
    pub(crate) fn new(paths: &Paths, dry_run: bool) -> Self {
        Self {
            dry_run,
            backup_root: paths.legacy_backup_dir(&backup_id(Utc::now(), std::process::id())),
        }
    }

    /// The `legacy-backup/<id>` directory this writer backs up into. It exists only once
    /// something was backed up.
    pub(crate) fn backup_root(&self) -> &Path {
        &self.backup_root
    }

    /// Where `path` lands in the backup tree.
    pub(crate) fn backup_path(&self, path: &Path) -> PathBuf {
        self.backup_root
            .join(path.strip_prefix("/").unwrap_or(path))
    }

    /// Copies `path` into the backup tree (mirroring its absolute path) without ever
    /// overwriting an existing backup.
    pub(crate) fn backup(&self, path: &Path) -> Result<()> {
        let src = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let mode = std::fs::metadata(&src).map_or(0o600, |m| m.mode() & 0o777);
        let rel = path.strip_prefix("/").unwrap_or(path);
        let mut dest = self.backup_root.join(rel);
        if let Some(dir) = dest.parent() {
            create_private_dirs(dir)?;
        }
        let mut data = std::fs::File::open(&src)
            .with_context(|| format!("could not read {}", path.display()))?;
        for attempt in 0.. {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(mode)
                .open(&dest)
            {
                Ok(mut out) => {
                    std::io::copy(&mut data, &mut out)?;
                    out.sync_all()?;
                    return Ok(());
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let mut name = dest.file_name().unwrap_or_default().to_os_string();
                    name.push(format!(".{attempt}"));
                    dest.set_file_name(name);
                }
                Err(e) => {
                    return Err(anyhow!(e).context(format!(
                        "could not back up {} to {}",
                        path.display(),
                        dest.display()
                    )));
                }
            }
        }
        unreachable!("the loop only exits by return")
    }

    /// Replaces `path` with `text` (backing up the old file first).
    pub(crate) fn write(&self, path: &Path, text: &str) -> Result<()> {
        if self.dry_run {
            return Ok(());
        }
        if path.exists() {
            self.backup(path)?;
        }
        write_atomic(path, text.as_bytes())
    }

    pub(crate) fn remove_file(&self, path: &Path) -> Result<()> {
        if self.dry_run {
            return Ok(());
        }
        self.backup(path)?;
        std::fs::remove_file(path)
            .with_context(|| format!("could not remove {}", path.display()))?;
        if let Some(dir) = path.parent() {
            // Only succeeds when the directory is empty.
            let _ = std::fs::remove_dir(dir);
        }
        Ok(())
    }
}

fn edit_file(w: &Writer, t: &FileTarget, exe: &str, add: bool) -> Result<Outcome> {
    let old = match std::fs::read_to_string(&t.path) {
        Ok(s) => Some(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(anyhow!(e).context(format!("could not read {}", t.path.display()))),
    };
    let exists = old.is_some();
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
    // opencode also reads `opencode.jsonc`; do not create a parallel `.json`.
    if !exists && t.path.extension().is_some_and(|x| x == "json") {
        let jsonc = t.path.with_extension("jsonc");
        if jsonc.exists() {
            return Ok(Outcome::new(
                &t.label,
                "skip",
                Some(&jsonc),
                Some("a .jsonc config exists; edit it by hand".into()),
            ));
        }
    }
    let old = old.unwrap_or_default();
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

fn edit_skill(w: &Writer, path: &Path, content: Option<&str>) -> Result<Outcome> {
    let existing = match std::fs::read_to_string(path) {
        Ok(s) => Some(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(anyhow!(e).context(format!("could not read {}", path.display()))),
    };
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

const CLAUDE_TIMEOUT: Duration = Duration::from_secs(30);

/// Runs the resolved `claude` with `args`, killing it after [`CLAUDE_TIMEOUT`].
fn run_claude(bin: &Path, args: &[&str]) -> Result<std::process::Output> {
    let mut child = Command::new(bin)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("could not run {}", bin.display()))?;
    let deadline = Instant::now() + CLAUDE_TIMEOUT;
    loop {
        if child.try_wait()?.is_some() {
            return Ok(child.wait_with_output()?);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(anyhow!(
                "`claude {}` timed out after {}s",
                args.join(" "),
                CLAUDE_TIMEOUT.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn stderr_of(o: &std::process::Output) -> String {
    String::from_utf8_lossy(&o.stderr).trim().to_string()
}

/// Claude Code's user-scope config: `$CLAUDE_CONFIG_DIR/.claude.json`, else `~/.claude.json`.
fn claude_user_config(host: &HostEnv) -> PathBuf {
    match std::env::var_os("CLAUDE_CONFIG_DIR").filter(|v| !v.is_empty()) {
        Some(d) => PathBuf::from(d).join(".claude.json"),
        None => host.home.join(".claude.json"),
    }
}

/// Our user-scope entry in Claude's config, read directly (`claude mcp get` health-checks
/// the server, which would spawn it).
fn claude_entry(host: &HostEnv) -> Result<Option<serde_json::Value>> {
    let path = claude_user_config(host);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(anyhow!(e).context(format!("could not read {}", path.display()))),
    };
    let v: serde_json::Value = serde_json::from_str(&text)
        .with_context(|| format!("{} is not valid JSON", path.display()))?;
    Ok(v.get("mcpServers").and_then(|m| m.get(NAME)).cloned())
}

fn entry_matches(entry: &serde_json::Value, exe: &str) -> bool {
    entry.get("command").and_then(|c| c.as_str()) == Some(exe)
        && entry.get("args") == Some(&serde_json::json!(["mcp"]))
}

fn claude_install(host: &HostEnv, exe: &str, force: bool, dry_run: bool) -> Result<Outcome> {
    let label = "claude";
    let entry = claude_entry(host)?;
    if !force && entry.as_ref().is_some_and(|e| entry_matches(e, exe)) {
        return Ok(Outcome::new(label, "unchanged", None, None));
    }
    let Ok(bin) = resolve_binary("claude", host.path_var.as_deref()) else {
        return Ok(Outcome::new(
            label,
            "skip",
            None,
            Some("claude is not on PATH".into()),
        ));
    };
    let action = if entry.is_some() { "update" } else { "create" };
    if dry_run {
        return Ok(Outcome::new(label, action, None, None));
    }
    let add_args = ["mcp", "add", "--scope", "user", NAME, "--", exe, "mcp"];
    let add = run_claude(&bin, &add_args)?;
    if add.status.success() {
        return Ok(Outcome::new(label, action, None, None));
    }
    let err = stderr_of(&add);
    if !err.contains("already exists") && entry.is_none() {
        return Err(anyhow!("`claude mcp add` failed: {err}"));
    }
    // The name is taken (a stale or forced entry): replace it.
    let rm = run_claude(&bin, &["mcp", "remove", "--scope", "user", NAME])?;
    if !rm.status.success() {
        return Err(anyhow!(
            "`claude mcp add` failed ({err}) and the existing entry could not be removed: {}",
            stderr_of(&rm)
        ));
    }
    let add = run_claude(&bin, &add_args)?;
    if !add.status.success() {
        return Err(anyhow!(
            "registration removed but re-add failed: {}",
            stderr_of(&add)
        ));
    }
    Ok(Outcome::new(label, "update", None, None))
}

fn claude_uninstall(host: &HostEnv, dry_run: bool) -> Result<Outcome> {
    let label = "claude";
    if claude_entry(host)?.is_none() {
        return Ok(Outcome::new(label, "unchanged", None, None));
    }
    let Ok(bin) = resolve_binary("claude", host.path_var.as_deref()) else {
        return Ok(Outcome::new(
            label,
            "skip",
            None,
            Some("claude is not on PATH".into()),
        ));
    };
    if !dry_run {
        let rm = run_claude(&bin, &["mcp", "remove", "--scope", "user", NAME])?;
        if !rm.status.success() {
            return Err(anyhow!("`claude mcp remove` failed: {}", stderr_of(&rm)));
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

/// One step's result, keyed by the target it acted on so errors name it.
pub(crate) type Step = (String, Result<Outcome>);

fn finish(ctx: &Ctx, results: Vec<Step>, dry_run: bool) -> Result<()> {
    let mut outcomes = Vec::new();
    let mut failures = 0usize;
    for (target, r) in results {
        match r {
            Ok(o) => outcomes.push(o),
            Err(e) => {
                failures += 1;
                outcomes.push(Outcome::new(&target, "error", None, Some(format!("{e:#}"))));
            }
        }
    }
    print(ctx, &outcomes, dry_run)?;
    if failures > 0 {
        return Err(anyhow!("{failures} step(s) failed"));
    }
    Ok(())
}

fn file_step(w: &Writer, t: &FileTarget, exe: &str, add: bool) -> Step {
    (t.label.clone(), edit_file(w, t, exe, add))
}

fn skill_step(w: &Writer, p: &Path, content: Option<&str>) -> Step {
    ("skill".to_string(), edit_skill(w, p, content))
}

pub fn install(ctx: &Ctx, force: bool, dry_run: bool) -> Result<()> {
    let w = Writer::new(&ctx.paths, dry_run);
    let results = install_steps(ctx, &w, force)?;
    finish(ctx, results, dry_run)
}

/// The MCP registrations and skill files `install` manages, written through `w`.
pub(crate) fn install_steps(ctx: &Ctx, w: &Writer, force: bool) -> Result<Vec<Step>> {
    let host = ctx.host()?;
    let exe = std::env::current_exe().context("could not find this executable")?;
    let exe = exe.to_string_lossy().into_owned();
    let accounts = AccountFile::list(&ctx.paths)?;
    let choices = ctx.config().map(|c| c.choices).unwrap_or_default();
    let agents = so_agents::discover_user(&ctx.paths).agents;
    let skill = render_skill(&accounts, &choices, &agents);

    let mut results = vec![(
        "claude".to_string(),
        claude_install(&host, &exe, force, w.dry_run),
    )];
    for t in config_files(&host, &accounts)? {
        results.push(file_step(w, &t, &exe, true));
    }
    for p in skill_files(&host) {
        results.push(skill_step(w, &p, Some(&skill)));
    }
    Ok(results)
}

pub fn uninstall(ctx: &Ctx, dry_run: bool) -> Result<()> {
    let host = ctx.host()?;
    let accounts = AccountFile::list(&ctx.paths)?;
    let w = Writer::new(&ctx.paths, dry_run);

    let mut results = vec![("claude".to_string(), claude_uninstall(&host, dry_run))];
    for t in config_files(&host, &accounts)? {
        results.push(file_step(&w, &t, "", false));
    }
    for p in skill_files(&host) {
        results.push(skill_step(&w, &p, None));
    }
    finish(ctx, results, dry_run)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn backup_id_pads_millis_so_ids_sort() {
        let t = Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 5).unwrap();
        let early = t + chrono::Duration::milliseconds(7);
        let late = t + chrono::Duration::milliseconds(120);
        assert_eq!(backup_id(early, 9), "20260102T030405Z-007-9");
        assert!(backup_id(early, 9) < backup_id(late, 9));
    }
}

//! Git worktree isolation (SPEC 7.3). Git is only ever run with an argv, never a shell.
//!
//! Everything here blocks; async callers wrap it in `spawn_blocking`.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// A git repository that contains the requested cwd.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoInfo {
    pub toplevel: PathBuf,
    /// The working tree has uncommitted or untracked changes.
    pub dirty: bool,
    /// `HEAD` points at a commit (a new repository has none).
    pub has_head: bool,
}

/// A worktree made for one run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Created {
    pub path: PathBuf,
    pub branch: String,
    /// `HEAD` of the repository when the worktree was made.
    pub base: String,
    pub repo: PathBuf,
}

/// Repository-selecting variables git exports to hooks. If a git child inherits them, `-C dir`
/// is overridden and git acts on the repository they name instead (a hook's own repo), so
/// every git child started from here removes them.
pub const INHERITED_GIT_ENV: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_PREFIX",
];

/// A `git -C dir` command with [`INHERITED_GIT_ENV`] cleared. Callers add their own args.
pub fn git_command(dir: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(dir);
    for var in INHERITED_GIT_ENV {
        cmd.env_remove(var);
    }
    // Reads must not take the index lock to refresh it behind someone's back.
    cmd.env("GIT_OPTIONAL_LOCKS", "0");
    cmd
}

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = git_command(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|e| format!("could not run git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        Err(format!(
            "git {} failed: {}",
            args.first().copied().unwrap_or(""),
            err.trim()
        ))
    }
}

/// `git -C cwd rev-parse --show-toplevel` plus dirty and HEAD checks. `None` when `cwd` is
/// not inside a git repository.
pub fn repo_info(cwd: &Path) -> Option<RepoInfo> {
    let toplevel = PathBuf::from(git(cwd, &["rev-parse", "--show-toplevel"]).ok()?);
    let has_head = git(&toplevel, &["rev-parse", "--verify", "--quiet", "HEAD"]).is_ok();
    let dirty = git(&toplevel, &["status", "--porcelain"]).is_ok_and(|s| !s.is_empty());
    Some(RepoInfo {
        toplevel,
        dirty,
        has_head,
    })
}

/// Lowercase `[a-z0-9-]`, at most 24 characters, from a run name or the prompt.
pub fn slug(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars().flat_map(char::to_lowercase) {
        let c = if c.is_ascii_alphanumeric() { c } else { '-' };
        if c == '-' && (out.is_empty() || out.ends_with('-')) {
            continue;
        }
        out.push(c);
        if out.len() >= 24 {
            break;
        }
    }
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() { "run".into() } else { out }
}

/// The part of a run id that is random in a UUID v7, so two runs started in the same
/// minute still get different worktree names.
pub fn short_id(run_id: &str) -> String {
    let hex: String = run_id.chars().filter(|c| *c != '-').collect();
    let start = hex.len().saturating_sub(8);
    hex[start..].to_string()
}

/// Expands `{repo_parent}` and `{repo}` in the `worktrees.dir` template.
pub fn expand_dir(template: &str, toplevel: &Path) -> PathBuf {
    let parent = toplevel.parent().unwrap_or(Path::new("/"));
    let repo = toplevel
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    PathBuf::from(
        template
            .replace("{repo_parent}", &parent.to_string_lossy())
            .replace("{repo}", &repo),
    )
}

/// `git worktree add -b <branch_prefix><slug>-<id8> <dir>/<slug>-<id8> HEAD`.
pub fn create(
    repo: &RepoInfo,
    template: &str,
    branch_prefix: &str,
    name_hint: &str,
    run_id: &str,
) -> Result<Created, String> {
    let leaf = format!("{}-{}", slug(name_hint), short_id(run_id));
    let path = expand_dir(template, &repo.toplevel).join(&leaf);
    let branch = format!("{branch_prefix}{leaf}");
    let base = git(&repo.toplevel, &["rev-parse", "HEAD"])?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("could not create {}: {e}", parent.display()))?;
    }
    let path_arg = path.to_string_lossy().into_owned();
    git(
        &repo.toplevel,
        &["worktree", "add", "-b", &branch, &path_arg, "HEAD"],
    )?;
    Ok(Created {
        path,
        branch,
        base,
        repo: repo.toplevel.clone(),
    })
}

/// Why a worktree was kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kept {
    NewCommits,
    Dirty,
    Missing,
    Failed(String),
}

/// Removes the worktree and its branch when the run left no commits since `base` and the
/// tree is clean. Returns why it was kept otherwise.
pub fn remove_if_unchanged(c: &Created) -> Result<(), Kept> {
    if !c.path.is_dir() {
        return Err(Kept::Missing);
    }
    let dirty = git(&c.path, &["status", "--porcelain"]).map_err(Kept::Failed)?;
    if !dirty.is_empty() {
        return Err(Kept::Dirty);
    }
    let range = format!("{}..HEAD", c.base);
    let ahead = git(&c.path, &["rev-list", "--count", &range]).map_err(Kept::Failed)?;
    if ahead != "0" {
        return Err(Kept::NewCommits);
    }
    remove(c, false).map_err(Kept::Failed)
}

/// Puts a run's worktree back at `path`. The branch is reused when it exists. A worktree
/// that was removed as untouched took its branch with it; then the branch is created again
/// at `base`, the commit the run started from. Fails with a clear message when neither is
/// available.
pub fn recreate(repo: &Path, path: &Path, branch: &str, base: Option<&str>) -> Result<(), String> {
    let _ = git(repo, &["worktree", "prune"]);
    let reference = format!("refs/heads/{branch}");
    let exists = git(repo, &["rev-parse", "--verify", "--quiet", &reference]).is_ok();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("could not create {}: {e}", parent.display()))?;
    }
    let path_arg = path.to_string_lossy().into_owned();
    if exists {
        git(repo, &["worktree", "add", "--", &path_arg, branch])?;
        return Ok(());
    }
    let Some(base) = base else {
        return Err(format!(
            "branch {branch} no longer exists and the run's base commit is unknown, so its worktree cannot be recreated"
        ));
    };
    git(
        repo,
        &["worktree", "add", "-b", branch, "--", &path_arg, base],
    )
    .map_err(|e| {
        format!("branch {branch} no longer exists and could not be recreated at {base}: {e}")
    })?;
    Ok(())
}

/// Removes the worktree directory only; its branch stays (it may hold the run's work).
pub fn remove_dir(repo: &Path, path: &Path) -> Result<(), String> {
    let path_arg = path.to_string_lossy().into_owned();
    git(repo, &["worktree", "remove", "--force", "--", &path_arg])?;
    Ok(())
}

/// True when the branch's tip is contained in `base` (merged) or in any remote branch
/// (pushed).
pub fn merged_or_pushed(c: &Created) -> bool {
    let tip = match git(&c.repo, &["rev-parse", &format!("refs/heads/{}", c.branch)]) {
        Ok(t) => t,
        Err(_) => return false,
    };
    // Merged into the repository's current HEAD (what the worktree was cut from).
    let merged = git_command(&c.repo)
        .args(["merge-base", "--is-ancestor", &tip, "HEAD"])
        .status()
        .is_ok_and(|s| s.success());
    if merged {
        return true;
    }
    git(&c.repo, &["branch", "-r", "--contains", &tip]).is_ok_and(|out| !out.is_empty())
}

/// Removes the worktree and deletes its branch. `force` also drops local changes.
pub fn remove(c: &Created, force: bool) -> Result<(), String> {
    let path_arg = c.path.to_string_lossy().into_owned();
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(&path_arg);
    git(&c.repo, &args)?;
    git(&c.repo, &["branch", "-D", &c.branch])?;
    Ok(())
}

/// The branch checked out in `dir`, or `None` for a detached `HEAD`.
pub fn current_branch(dir: &Path) -> Option<String> {
    git(dir, &["symbolic-ref", "--short", "-q", "HEAD"])
        .ok()
        .filter(|b| !b.is_empty())
}

/// Changed paths of `dir` (untracked files included): up to five of them, then a count of
/// the rest. `None` when the tree is clean.
pub fn uncommitted(dir: &Path) -> Option<String> {
    let out = git(dir, &["status", "--porcelain"]).ok()?;
    let lines: Vec<&str> = out.lines().collect();
    if lines.is_empty() {
        return None;
    }
    let mut text = lines
        .iter()
        .take(5)
        .map(|l| l.get(3..).unwrap_or(l).trim())
        .collect::<Vec<_>>()
        .join(", ");
    if lines.len() > 5 {
        text.push_str(&format!(" (+{} more)", lines.len() - 5));
    }
    Some(text)
}

/// True when the local branch exists in `repo`.
pub fn branch_exists(repo: &Path, branch: &str) -> bool {
    git(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )
    .is_ok()
}

/// `s` quoted for a POSIX shell, left bare when it only has safe characters.
pub fn sh_quote(s: &str) -> String {
    let safe = !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c));
    if safe {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// The command that merges `branch` into the checkout at `repo`.
pub fn merge_command(repo: &Path, branch: &str) -> String {
    format!(
        "git -C {} merge {}",
        sh_quote(&repo.to_string_lossy()),
        sh_quote(branch)
    )
}

/// A commit id as stored on the run row: hex, so it can never be read as an option.
fn is_sha(s: &str) -> bool {
    s.len() >= 7 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// What a successful [`merge_branch`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Merged {
    FastForward,
    MergeCommit,
}

/// The branch the base checkout was on when the run was spawned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BaseBranch {
    Named(String),
    /// `HEAD` was detached.
    Detached,
    /// Not recorded (a run from before this was kept).
    Unknown,
}

/// Longest `git merge` may run (hooks and signing can hang).
pub const MERGE_TIMEOUT: Duration = Duration::from_secs(120);

/// The operation (merge, rebase, ...) a repository is in the middle of, if any.
pub fn in_progress(repo: &Path) -> Option<&'static str> {
    const MARKERS: [(&str, &str); 6] = [
        ("MERGE_HEAD", "merge"),
        ("REBASE_HEAD", "rebase"),
        ("rebase-merge", "rebase"),
        ("rebase-apply", "rebase"),
        ("CHERRY_PICK_HEAD", "cherry-pick"),
        ("REVERT_HEAD", "revert"),
    ];
    MARKERS.into_iter().find_map(|(name, label)| {
        let path = git(repo, &["rev-parse", "--git-path", name]).ok()?;
        let path = Path::new(&path);
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            repo.join(path)
        };
        path.exists().then_some(label)
    })
}

fn merge_head_exists(repo: &Path) -> bool {
    in_progress(repo) == Some("merge")
}

struct Captured {
    status: Option<std::process::ExitStatus>,
    out: String,
}

/// Runs `cmd` with no stdin, in its own process group, and gives up after `timeout`, killing
/// the whole group. `status` is `None` on a timeout.
fn run_with_timeout(mut cmd: Command, timeout: Duration) -> Result<Captured, String> {
    use std::io::Read;
    use std::os::unix::process::CommandExt;
    use std::sync::{Arc, Mutex};
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = cmd.spawn().map_err(|e| format!("could not run git: {e}"))?;
    let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
    let mut readers = Vec::new();
    for pipe in [
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    ]
    .into_iter()
    .flatten()
    {
        let buf = Arc::clone(&buf);
        readers.push(std::thread::spawn(move || {
            let mut pipe = pipe;
            let mut chunk = [0u8; 4096];
            while let Ok(n) = pipe.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                if let Ok(mut b) = buf.lock() {
                    b.extend_from_slice(&chunk[..n]);
                }
            }
        }));
    }
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Ok(None) => break None,
            Err(e) => return Err(format!("could not wait for git: {e}")),
        }
    };
    if status.is_none() {
        let _ = Command::new("kill")
            .args(["-KILL", "--", &format!("-{}", child.id())])
            .status();
        let _ = child.kill();
        let _ = child.wait();
    } else {
        // A background process may hold the pipes open; do not wait for it forever.
        let end = Instant::now() + Duration::from_secs(2);
        for r in readers {
            while !r.is_finished() && Instant::now() < end {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }
    let out = buf
        .lock()
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default();
    Ok(Captured { status, out })
}

/// A merge commit when `HEAD` has two parents, otherwise a fast-forward.
fn merged_kind(repo: &Path) -> Result<Merged, String> {
    let parents = git(repo, &["rev-list", "--parents", "-n", "1", "HEAD"])?;
    Ok(if parents.split(' ').count() > 2 {
        Merged::MergeCommit
    } else {
        Merged::FastForward
    })
}

/// Puts the base checkout back after a merge this call started failed or was killed. Only
/// called after the checkout was verified clean (no merge in progress, no tracked
/// changes), so anything found now is ours: an unfinished merge is aborted, and tracked
/// changes a killed merge left in the index are reset.
fn undo_merge(repo: &Path) {
    if merge_head_exists(repo) {
        let _ = git(repo, &["merge", "--abort"]);
    }
    let dirty = git(repo, &["status", "--porcelain", "--untracked-files=no"]).unwrap_or_default();
    if !dirty.is_empty() {
        let _ = git(repo, &["reset", "-q", "--hard", "HEAD"]);
    }
}

/// Merges `branch` into the branch checked out in `repo` (SPEC 7.5, `accept_run` with
/// `merge`). Refuses, changing nothing, unless no merge, rebase, cherry-pick or revert is in
/// progress, the checkout has no tracked changes and it is on `base_branch` (when that is
/// unknown, on any branch that contains `base_commit`). A conflict, hook failure or timeout
/// aborts the merge. The user's hooks run. Every error is a message for the orchestrator.
pub fn merge_branch(
    repo: &Path,
    branch: &str,
    base_branch: &BaseBranch,
    base_commit: &str,
    message: &str,
) -> Result<Merged, String> {
    merge_branch_with(
        repo,
        branch,
        base_branch,
        base_commit,
        message,
        MERGE_TIMEOUT,
    )
}

pub fn merge_branch_with(
    repo: &Path,
    branch: &str,
    base_branch: &BaseBranch,
    base_commit: &str,
    message: &str,
    timeout: Duration,
) -> Result<Merged, String> {
    if !branch_exists(repo, branch) {
        return Err(format!(
            "branch {branch} no longer exists in {}; there is nothing to merge",
            repo.display()
        ));
    }
    if *base_branch == BaseBranch::Detached {
        return Err(format!(
            "the run started on a detached HEAD; merge {branch} by hand: {}",
            merge_command(repo, branch)
        ));
    }
    if let Some(op) = in_progress(repo) {
        return Err(format!(
            "finish or abort the {op} in progress in {} first",
            repo.display()
        ));
    }
    let Some(on) = current_branch(repo) else {
        return Err(format!(
            "{} has a detached HEAD; check out the branch to merge into, then accept again",
            repo.display()
        ));
    };
    match base_branch {
        BaseBranch::Named(want) if *want != on => {
            return Err(format!(
                "{} is on branch {on}, but this run was started from {want}; check out {want}, then accept again",
                repo.display()
            ));
        }
        BaseBranch::Named(_) => {}
        _ => {
            let contains = is_sha(base_commit)
                && git_command(repo)
                    .args(["merge-base", "--is-ancestor", base_commit, "HEAD"])
                    .status()
                    .is_ok_and(|s| s.success());
            if !contains {
                return Err(format!(
                    "{} is on branch {on}, which does not contain the commit this run started from ({}); check out its base branch, then accept again",
                    repo.display(),
                    &base_commit[..base_commit.len().min(12)]
                ));
            }
        }
    }
    let tracked = git(repo, &["status", "--porcelain", "--untracked-files=no"])?;
    if !tracked.is_empty() {
        return Err(format!(
            "{} has uncommitted changes to tracked files; commit or stash them, then accept again",
            repo.display()
        ));
    }
    let before = git(repo, &["rev-parse", "HEAD"])?;
    let mut cmd = git_command(repo);
    cmd.args(["merge", "--no-edit", "-m", message])
        .arg(format!("refs/heads/{branch}"))
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_MERGE_AUTOEDIT", "no")
        .env("GIT_EDITOR", "true")
        .env("GIT_ASKPASS", "true")
        .env("SSH_ASKPASS", "true");
    let ran = run_with_timeout(cmd, timeout)?;
    let Some(status) = ran.status else {
        // A hook can hang after the commit was made; then the merge itself is done.
        if git(repo, &["rev-parse", "HEAD"])? != before {
            return merged_kind(repo);
        }
        undo_merge(repo);
        return Err(format!(
            "merge timed out (hooks or signing?) after {}s in {}; it was aborted",
            timeout.as_secs(),
            repo.display()
        ));
    };
    if status.success() {
        let after = git(repo, &["rev-parse", "HEAD"])?;
        return Ok(if after == before {
            Merged::FastForward
        } else {
            merged_kind(repo)?
        });
    }
    let conflicts = git(repo, &["diff", "--name-only", "--diff-filter=U"]).unwrap_or_default();
    undo_merge(repo);
    if ran.out.contains("index.lock") {
        return Err(format!(
            "another git operation is running in {}; try again",
            repo.display()
        ));
    }
    if conflicts.is_empty() {
        return Err(format!(
            "merging {branch} into {on} failed (a hook may have refused it): {}. The merge was aborted and nothing changed",
            ran.out.trim()
        ));
    }
    let files: Vec<&str> = conflicts.lines().collect();
    Err(format!(
        "merging {branch} into {on} conflicts in {}; the merge was aborted and nothing changed. Reject the run with feedback to resolve the conflict, or merge {branch} by hand",
        files.join(", ")
    ))
}

/// What a run changed, for `get_run` with `diff`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffOut {
    pub base: String,
    pub stat: String,
    pub patch: String,
    pub truncated: bool,
}

/// Most untracked files added to a diff.
const MAX_UNTRACKED: usize = 100;
/// Most bytes read of the list of untracked files.
const LIST_CAP: usize = 1 << 20;

/// Output of one git call, read only up to `cap` bytes: the child is killed when there is
/// more. `code` is git's exit code.
struct Capped {
    bytes: Vec<u8>,
    truncated: bool,
    code: Option<i32>,
}

fn git_capped(dir: &Path, args: &[&str], cap: usize) -> Result<Capped, String> {
    use std::io::Read;
    let mut child = git_command(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("could not run git: {e}"))?;
    let mut bytes = Vec::new();
    if let Some(out) = child.stdout.take() {
        out.take(cap as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| format!("could not read git output: {e}"))?;
    }
    let truncated = bytes.len() > cap;
    if truncated {
        bytes.truncate(cap);
        let _ = child.kill();
    }
    let code = child.wait().ok().and_then(|s| s.code());
    Ok(Capped {
        bytes,
        truncated,
        code,
    })
}

fn truncate_utf8(text: &mut String, cap: usize) -> bool {
    if text.len() <= cap {
        return false;
    }
    let mut end = cap;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    true
}

const DIFF_FLAGS: [&str; 3] = ["diff", "--no-color", "--no-ext-diff"];

/// The changes of a run since the merge base of its branch: committed work plus, when the
/// worktree exists, uncommitted changes and untracked files. The patch is cut at `cap`
/// bytes; git is stopped there rather than read to the end.
pub fn diff(c: &Created, cap: usize) -> Result<DiffOut, String> {
    if !is_sha(&c.base) {
        return Err(format!(
            "the run's base commit {:?} is not a commit id",
            c.base
        ));
    }
    let tip = format!("refs/heads/{}", c.branch);
    let base = git(&c.repo, &["merge-base", &c.base, &tip]).unwrap_or_else(|_| c.base.clone());
    let live = c.path.is_dir();
    let (dir, target): (&Path, String) = if live {
        (&c.path, base.clone())
    } else {
        (&c.repo, format!("{base}..{tip}"))
    };
    let run = |extra: &[&str], limit: usize| -> Result<Capped, String> {
        let mut args: Vec<&str> = DIFF_FLAGS.to_vec();
        args.push("--no-textconv");
        args.extend_from_slice(extra);
        args.push(&target);
        args.push("--");
        let out = git_capped(dir, &args, limit)?;
        if out.code != Some(0) && !out.truncated {
            return Err(format!("could not diff {} against {base}", c.branch));
        }
        Ok(out)
    };
    let stat_out = run(&["--stat"], cap)?;
    let mut stat = String::from_utf8_lossy(&stat_out.bytes)
        .trim_end()
        .to_string();
    let patch_out = run(&[], cap)?;
    let mut patch = patch_out.bytes;
    let mut truncated = patch_out.truncated;
    if live {
        let names = git_capped(
            dir,
            &["ls-files", "--others", "--exclude-standard", "-z"],
            LIST_CAP,
        )?;
        let names = String::from_utf8_lossy(&names.bytes).into_owned();
        let all: Vec<&str> = names.split('\0').filter(|n| !n.is_empty()).collect();
        for name in all.iter().take(MAX_UNTRACKED) {
            if !stat.is_empty() {
                stat.push('\n');
            }
            stat.push_str(&format!(" {name} (untracked)"));
            if !truncated {
                let left = cap + 1 - patch.len();
                let mut args: Vec<&str> = DIFF_FLAGS.to_vec();
                args.extend(["--no-textconv", "--no-index", "--", "/dev/null", name]);
                // `--no-index` exits 1 when the files differ.
                let one = git_capped(dir, &args, left)?;
                patch.extend_from_slice(&one.bytes);
                truncated = patch.len() > cap || one.truncated;
            }
        }
        if all.len() > MAX_UNTRACKED {
            stat.push_str(&format!("\n (+{} more files)", all.len() - MAX_UNTRACKED));
        }
    }
    let mut patch = String::from_utf8_lossy(&patch).into_owned();
    truncated |= truncate_utf8(&mut patch, cap);
    Ok(DiffOut {
        base,
        stat,
        patch,
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_children_do_not_inherit_hook_repository_env() {
        // The hook's GIT_DIR etc. must not reach git: a child that inherits them ignores `-C`.
        // Env changes are recorded on the Command, so this checks the spawn without touching
        // the process environment (which would race other tests).
        let cmd = git_command(Path::new("/nonexistent/repo"));
        let cleared: Vec<String> = cmd
            .get_envs()
            .filter(|(_, v)| v.is_none())
            .map(|(k, _)| k.to_string_lossy().into_owned())
            .collect();
        for var in INHERITED_GIT_ENV {
            assert!(cleared.iter().any(|k| k == var), "{var} not cleared");
        }
    }

    #[test]
    fn shell_quoting() {
        assert_eq!(sh_quote("/tmp/repo-1"), "/tmp/repo-1");
        assert_eq!(sh_quote("/tmp/my repo"), "'/tmp/my repo'");
        assert_eq!(sh_quote("it's"), r"'it'\''s'");
        assert_eq!(sh_quote(""), "''");
        assert_eq!(
            merge_command(Path::new("/r p"), "so/x-1"),
            "git -C '/r p' merge so/x-1"
        );
    }

    fn repo_with_branch() -> (tempfile::TempDir, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().to_path_buf();
        for a in [
            vec!["init", "-q", "-b", "main"],
            vec!["config", "user.email", "t@e"],
            vec!["config", "user.name", "t"],
        ] {
            git(&p, &a).unwrap();
        }
        let commit = |file: &str| {
            std::fs::write(p.join(file), file).unwrap();
            git(&p, &["add", "."]).unwrap();
            git(&p, &["commit", "-q", "-m", file]).unwrap();
        };
        commit("a");
        git(&p, &["checkout", "-q", "-b", "feat"]).unwrap();
        commit("f");
        git(&p, &["checkout", "-q", "main"]).unwrap();
        commit("m");
        (d, p)
    }

    #[test]
    fn a_hung_merge_hook_times_out_and_leaves_the_base_clean() {
        use std::os::unix::fs::PermissionsExt;
        let (_d, p) = repo_with_branch();
        let hook = p.join(".git/hooks/pre-merge-commit");
        std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
        std::fs::write(&hook, "#!/bin/sh\nsleep 30\n").unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        let base = git(&p, &["rev-parse", "main~1"]).unwrap();
        let started = Instant::now();
        let e = merge_branch_with(
            &p,
            "feat",
            &BaseBranch::Named("main".into()),
            &base,
            "merge: t (x)",
            Duration::from_millis(500),
        )
        .unwrap_err();
        assert!(e.contains("merge timed out"), "{e}");
        assert!(started.elapsed() < Duration::from_secs(10));
        assert!(!p.join(".git/MERGE_HEAD").exists());
        assert!(git(&p, &["status", "--porcelain"]).unwrap().is_empty());
    }

    #[test]
    fn the_dirty_list_names_five_files_and_counts_the_rest() {
        let d = tempfile::tempdir().unwrap();
        git(d.path(), &["init", "-q"]).unwrap();
        assert_eq!(uncommitted(d.path()), None);
        for i in 0..7 {
            std::fs::write(d.path().join(format!("f{i}")), "x").unwrap();
        }
        let s = uncommitted(d.path()).unwrap();
        assert!(
            s.contains("f0") && s.contains("f4") && !s.contains("f5"),
            "{s}"
        );
        assert!(s.ends_with("(+2 more)"), "{s}");
    }

    #[test]
    fn truncation_keeps_whole_characters() {
        let mut s = "aé".repeat(10);
        assert!(truncate_utf8(&mut s, 4));
        assert_eq!(s, "aéa");
        let mut short = "ab".to_string();
        assert!(!truncate_utf8(&mut short, 4));
    }

    #[test]
    fn slug_rules() {
        assert_eq!(slug("Fix the Parser, now!"), "fix-the-parser-now");
        assert_eq!(slug("  --  "), "run");
        assert_eq!(slug("ÄÖ unicode"), "unicode");
        assert!(slug(&"abc ".repeat(20)).len() <= 24);
        assert!(!slug(&"abc ".repeat(20)).ends_with('-'));
    }

    #[test]
    fn short_id_uses_the_random_tail() {
        assert_eq!(short_id("019a2b3c-4d5e-7f60-8a1b-0123456789ab"), "456789ab");
    }

    #[test]
    fn dir_template() {
        let d = expand_dir("{repo_parent}/{repo}-worktrees", Path::new("/w/proj"));
        assert_eq!(d, PathBuf::from("/w/proj-worktrees"));
    }
}

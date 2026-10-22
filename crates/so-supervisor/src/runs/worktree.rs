//! Git worktree isolation (SPEC 7.3). Git is only ever run with an argv, never a shell.
//!
//! Everything here blocks; async callers wrap it in `spawn_blocking`.

use std::path::{Path, PathBuf};
use std::process::Command;

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

/// The first lines of `git status --porcelain` for `dir` when it has any change (untracked
/// files included).
pub fn uncommitted(dir: &Path) -> Option<String> {
    let out = git(dir, &["status", "--porcelain"]).ok()?;
    if out.is_empty() {
        return None;
    }
    Some(out.lines().take(5).collect::<Vec<_>>().join("; "))
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

/// What a successful [`merge_branch`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Merged {
    FastForward,
    MergeCommit,
}

/// Merges `branch` into the branch checked out in `repo` (SPEC 7.5, `accept_run` with
/// `merge`). Refuses, changing nothing, unless the checkout has no tracked changes and is on
/// `base_branch`; when that is unknown, on any branch that contains `base_commit`. A conflict
/// is aborted. Every error is a message for the orchestrator.
pub fn merge_branch(
    repo: &Path,
    branch: &str,
    base_branch: Option<&str>,
    base_commit: &str,
    message: &str,
) -> Result<Merged, String> {
    if !branch_exists(repo, branch) {
        return Err(format!(
            "branch {branch} no longer exists in {}; there is nothing to merge",
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
        Some(want) if want != on => {
            return Err(format!(
                "{} is on branch {on}, but this run was started from {want}; check out {want}, then accept again",
                repo.display()
            ));
        }
        Some(_) => {}
        None => {
            let contains = git_command(repo)
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
    let out = git_command(repo)
        .args(["merge", "--no-edit", "-m", message, branch])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_MERGE_AUTOEDIT", "no")
        .output()
        .map_err(|e| format!("could not run git: {e}"))?;
    if out.status.success() {
        let parents = git(repo, &["rev-list", "--parents", "-n", "1", "HEAD"])?;
        let after = git(repo, &["rev-parse", "HEAD"])?;
        return Ok(if after != before && parents.split(' ').count() > 2 {
            Merged::MergeCommit
        } else {
            Merged::FastForward
        });
    }
    let detail = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let conflicts = git(repo, &["diff", "--name-only", "--diff-filter=U"]).unwrap_or_default();
    // Leave the base checkout as it was found.
    let _ = git(repo, &["merge", "--abort"]);
    if conflicts.is_empty() {
        return Err(format!(
            "merging {branch} into {on} failed: {}",
            detail.trim()
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

fn git_raw(dir: &Path, args: &[&str]) -> Result<(bool, Vec<u8>), String> {
    let out = git_command(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|e| format!("could not run git: {e}"))?;
    Ok((out.status.success(), out.stdout))
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

/// The changes of a run since the merge base of its branch: committed work plus, when the
/// worktree exists, uncommitted changes and untracked files. The patch is cut at `cap`
/// bytes.
pub fn diff(c: &Created, cap: usize) -> Result<DiffOut, String> {
    let base = git(&c.repo, &["merge-base", &c.base, &c.branch]).unwrap_or_else(|_| c.base.clone());
    let live = c.path.is_dir();
    let (dir, target): (&Path, String) = if live {
        (&c.path, base.clone())
    } else {
        (&c.repo, format!("{base}..{}", c.branch))
    };
    let (ok, stat) = git_raw(dir, &["diff", "--no-color", "--stat", &target])?;
    if !ok {
        return Err(format!("could not diff {} against {base}", c.branch));
    }
    let mut stat = String::from_utf8_lossy(&stat).trim_end().to_string();
    let (ok, patch) = git_raw(dir, &["diff", "--no-color", &target])?;
    if !ok {
        return Err(format!("could not diff {} against {base}", c.branch));
    }
    let mut patch = String::from_utf8_lossy(&patch).into_owned();
    if live {
        let (_, names) = git_raw(dir, &["ls-files", "--others", "--exclude-standard", "-z"])?;
        let names = String::from_utf8_lossy(&names).into_owned();
        for name in names
            .split('\0')
            .filter(|n| !n.is_empty())
            .take(MAX_UNTRACKED)
        {
            if !stat.is_empty() {
                stat.push('\n');
            }
            stat.push_str(&format!(" {name} (untracked)"));
            if patch.len() <= cap {
                // `--no-index` exits 1 when the files differ.
                let (_, one) = git_raw(
                    dir,
                    &["diff", "--no-color", "--no-index", "--", "/dev/null", name],
                )?;
                patch.push_str(&String::from_utf8_lossy(&one));
            }
        }
    }
    let truncated = truncate_utf8(&mut patch, cap);
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

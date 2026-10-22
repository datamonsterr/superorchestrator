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

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
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

#[cfg(test)]
mod tests {
    use super::*;

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

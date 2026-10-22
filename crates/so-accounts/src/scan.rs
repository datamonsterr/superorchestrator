//! Detects existing credential homes for `account import --scan` (SPEC 5.4, 19.2).

use std::path::{Path, PathBuf};

use so_core::{AccountFile, Harness, Paths};

use crate::error::Result;
use crate::host::{HostEnv, same_path};
use crate::signin::{SignIn, sign_in_state};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportCandidate {
    pub name: String,
    pub harness: Harness,
    pub home: PathBuf,
}

/// Numbers N of directory entries named `<prefix><N>`, sorted.
fn numbered(dir: &Path, prefix: &str) -> Vec<(u32, PathBuf)> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<_> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_str()?.to_string();
            let digits = name.strip_prefix(prefix)?;
            if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return None; // odd names, e.g. with spaces, are skipped
            }
            Some((digits.parse().ok()?, e.path()))
        })
        .collect();
    out.sort();
    out
}

/// Scans the host home for known layouts, keeping only homes that show a sign-in.
///
/// `~/.codex` becomes `codex-acc1`, unless `~/.codex-acc1` exists: then
/// `~/.codex` is `codex-default`, or is skipped when `~/.codex-acc1` is the
/// same directory (a symlink), and `~/.codex-accN` becomes `codex-accN`.
pub fn scan(host: &HostEnv) -> Vec<ImportCandidate> {
    let home = host.home.as_path();
    let mut out = Vec::new();
    let mut push = |name: String, harness, home: PathBuf| {
        out.push(ImportCandidate {
            name,
            harness,
            home,
        });
    };

    let codex = home.join(".codex");
    let numbered_codex = numbered(home, ".codex-acc");
    let acc1 = numbered_codex.iter().find(|(n, _)| *n == 1);
    if codex.is_dir() {
        match acc1 {
            None => push("codex-acc1".into(), Harness::Codex, codex),
            Some((_, p)) if same_path(p, &codex) => {}
            Some(_) => push("codex-default".into(), Harness::Codex, codex),
        }
    }
    for (n, p) in numbered_codex {
        if p.is_dir() {
            push(format!("codex-acc{n}"), Harness::Codex, p);
        }
    }

    let profiles = home.join(".agent-farm/profiles");
    for (n, p) in numbered(&profiles, "agy_worker_") {
        if p.join(".gemini").is_dir() {
            push(format!("agy-acc{n}"), Harness::Agy, p);
        }
    }

    let oc = home.join(".local/share/opencode");
    if oc.is_dir() {
        push("opencode-acc1".into(), Harness::Opencode, oc);
    }
    let claude = home.join(".claude");
    if claude.is_dir() {
        push("claude-acc1".into(), Harness::Claude, claude);
    }
    out.retain(|c| signed_in(c, host));
    dedupe(out)
}

/// True when the candidate home shows evidence of a sign-in: the harness's auth file per
/// [`sign_in_state`]. Claude's default home may keep credentials in the keyring (state
/// `Unknown`); there a `projects` dir (history of real use) counts. Contents are never read.
/// What `install` and harness probes leave behind (backups, logs, `CLAUDE.md`) is no evidence.
fn signed_in(c: &ImportCandidate, host: &HostEnv) -> bool {
    let Ok(acct) = AccountFile::new(
        &c.name,
        c.harness,
        &c.home.to_string_lossy(),
        c.harness.as_str(),
    ) else {
        return false;
    };
    match sign_in_state(&acct, host) {
        Ok(SignIn::SignedIn) => true,
        Ok(SignIn::Unknown) => c.home.join("projects").is_dir(),
        _ => false,
    }
}

/// Drops repeated names and candidates whose resolved home was already emitted.
fn dedupe(cands: Vec<ImportCandidate>) -> Vec<ImportCandidate> {
    let mut out: Vec<ImportCandidate> = Vec::new();
    for c in cands {
        let canon = c.home.canonicalize().unwrap_or_else(|_| c.home.clone());
        let clash = out.iter().any(|o| {
            o.name == c.name || o.home.canonicalize().unwrap_or_else(|_| o.home.clone()) == canon
        });
        if !clash {
            out.push(c);
        }
    }
    out
}

/// [`scan`] of the host home, without candidates whose account file already
/// exists or whose home an existing account already uses.
pub fn scan_new(paths: &Paths, host: &HostEnv) -> Result<Vec<ImportCandidate>> {
    let used: Vec<PathBuf> = AccountFile::list(paths)?
        .iter()
        .filter_map(|a| host.expand(&a.home).ok())
        .map(|h| h.canonicalize().unwrap_or(h))
        .collect();
    Ok(scan(host)
        .into_iter()
        .filter(|c| !paths.account_file(&c.name).exists())
        .filter(|c| !used.contains(&c.home.canonicalize().unwrap_or_else(|_| c.home.clone())))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::host;
    use so_core::Platform;

    fn mk(root: &Path, rel: &str) {
        std::fs::create_dir_all(root.join(rel)).unwrap();
    }

    fn touch(root: &Path, rel: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, "{}").unwrap();
    }

    fn names(c: &[ImportCandidate]) -> Vec<&str> {
        c.iter().map(|c| c.name.as_str()).collect()
    }

    fn scan_at(root: &Path) -> Vec<ImportCandidate> {
        scan(&host(root, Platform::Linux))
    }

    #[test]
    fn mirrors_real_layout_with_symlinked_acc1() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        touch(r, ".codex/auth.json");
        std::os::unix::fs::symlink(r.join(".codex"), r.join(".codex-acc1")).unwrap();
        for n in 2..=5 {
            touch(r, &format!(".codex-acc{n}/auth.json"));
        }
        for n in 1..=2 {
            touch(
                r,
                &format!(".agent-farm/profiles/agy_worker_{n}/.gemini/oauth_creds.json"),
            );
        }
        mk(r, ".agent-farm/profiles/agy_worker_4 SKILL=1 X=/");
        mk(r, ".agent-farm/profiles/agy_worker_9"); // no .gemini
        touch(r, ".local/share/opencode/auth.json");
        mk(r, ".claude/projects");
        let c = scan_at(r);
        assert_eq!(
            names(&c),
            [
                "codex-acc1",
                "codex-acc2",
                "codex-acc3",
                "codex-acc4",
                "codex-acc5",
                "agy-acc1",
                "agy-acc2",
                "opencode-acc1",
                "claude-acc1"
            ]
        );
        assert_eq!(c[0].home, r.join(".codex-acc1"));
        assert_eq!(c[5].home, r.join(".agent-farm/profiles/agy_worker_1"));
    }

    #[test]
    fn claude_home_left_by_install_is_not_an_account() {
        let d = tempfile::tempdir().unwrap();
        mk(d.path(), ".claude");
        assert!(scan_at(d.path()).is_empty());
        mk(d.path(), ".claude/backups");
        mk(d.path(), ".claude/skills/superorchestrator");
        touch(d.path(), ".claude/backups/.claude.json.backup.1");
        std::fs::write(d.path().join(".claude/CLAUDE.md"), "block").unwrap();
        assert!(scan_at(d.path()).is_empty());
    }

    #[test]
    fn claude_home_with_credentials_or_history_is_offered() {
        let d = tempfile::tempdir().unwrap();
        touch(d.path(), ".claude/.credentials.json");
        assert_eq!(names(&scan_at(d.path())), ["claude-acc1"]);
        // Keyring case: the default home has no file, but a projects dir shows real use.
        let e = tempfile::tempdir().unwrap();
        mk(e.path(), ".claude/projects");
        assert_eq!(names(&scan_at(e.path())), ["claude-acc1"]);
        assert_eq!(
            names(&scan(&host(e.path(), Platform::MacOs))),
            ["claude-acc1"]
        );
    }

    #[test]
    fn opencode_home_needs_auth_file() {
        let d = tempfile::tempdir().unwrap();
        mk(d.path(), ".local/share/opencode/log");
        mk(d.path(), ".local/share/opencode/repos");
        assert!(scan_at(d.path()).is_empty());
        touch(d.path(), ".local/share/opencode/auth.json");
        assert_eq!(names(&scan_at(d.path())), ["opencode-acc1"]);
    }

    #[test]
    fn codex_and_agy_homes_need_auth_files() {
        let d = tempfile::tempdir().unwrap();
        mk(d.path(), ".codex");
        mk(d.path(), ".agent-farm/profiles/agy_worker_1/.gemini");
        assert!(scan_at(d.path()).is_empty());
    }

    #[test]
    fn codex_without_acc1_is_acc1() {
        let d = tempfile::tempdir().unwrap();
        touch(d.path(), ".codex/auth.json");
        touch(d.path(), ".codex-acc2/auth.json");
        let c = scan_at(d.path());
        assert_eq!(names(&c), ["codex-acc1", "codex-acc2"]);
        assert_eq!(c[0].home, d.path().join(".codex"));
    }

    #[test]
    fn codex_default_when_acc1_is_distinct() {
        let d = tempfile::tempdir().unwrap();
        touch(d.path(), ".codex/auth.json");
        touch(d.path(), ".codex-acc1/auth.json");
        let c = scan_at(d.path());
        assert_eq!(names(&c), ["codex-default", "codex-acc1"]);
        assert_eq!(c[0].home, d.path().join(".codex"));
    }

    #[test]
    fn scan_new_skips_existing_and_empty_home_is_empty() {
        let d = tempfile::tempdir().unwrap();
        assert!(scan_at(d.path()).is_empty());
        mk(d.path(), ".claude/projects");
        touch(d.path(), ".local/share/opencode/auth.json");
        let paths = Paths::from_dir(d.path().join("cfg"));
        let host = host(d.path(), Platform::Linux);
        AccountFile::new("claude-acc1", Harness::Claude, "~/.claude", "claude")
            .unwrap()
            .save(&paths)
            .unwrap();
        assert_eq!(names(&scan_new(&paths, &host).unwrap()), ["opencode-acc1"]);
        // An account under another name already using the home also hides it.
        AccountFile::new(
            "my-oc",
            Harness::Opencode,
            "~/.local/share/opencode",
            "opencode",
        )
        .unwrap()
        .save(&paths)
        .unwrap();
        assert!(scan_new(&paths, &host).unwrap().is_empty());
    }
}

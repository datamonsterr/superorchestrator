//! Build identity and executable-replaced detection (issue #12).
//!
//! The version string is the same for every build of a release, so it cannot tell a running
//! daemon from a freshly installed binary. The build id is derived from the executable file
//! itself (size and mtime), so it needs no `build.rs`, no `.git` and no extra dependency, and
//! it changes on every rebuild, dirty tree or not.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// What identifies one executable file on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Ident {
    dev: u64,
    ino: u64,
    size: u64,
    mtime_ns: i128,
}

impl Ident {
    fn of(m: &std::fs::Metadata) -> Self {
        Self {
            dev: m.dev(),
            ino: m.ino(),
            size: m.size(),
            mtime_ns: i128::from(m.mtime()) * 1_000_000_000 + i128::from(m.mtime_nsec()),
        }
    }

    fn build_id(&self) -> String {
        format!("{:x}-{:x}", self.size, self.mtime_ns)
    }
}

/// The path of the running executable without the ` (deleted)` suffix Linux appends to
/// `/proc/self/exe` once the file was unlinked or replaced.
fn exe_path() -> Option<PathBuf> {
    let p = std::env::current_exe().ok()?;
    let s = p.to_string_lossy();
    Some(match s.strip_suffix(" (deleted)") {
        Some(t) => PathBuf::from(t),
        None => p,
    })
}

/// Build id of the running executable, e.g. `2a5c3f0-17f9a...`; `unknown` when it cannot be
/// read. Computed once.
pub fn build_id() -> &'static str {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| {
        ExeWatch::capture()
            .map(|w| w.start.build_id())
            .unwrap_or_else(|| "unknown".to_string())
    })
}

/// The executable as it was when the process started; [`ExeWatch::stale`] says whether the
/// file at that path has since been replaced or removed.
#[derive(Debug, Clone)]
pub struct ExeWatch {
    path: PathBuf,
    start: Ident,
}

impl ExeWatch {
    /// Watches the running process's executable.
    pub fn capture() -> Option<Self> {
        let path = exe_path()?;
        // `/proc/self/exe` still names the original inode after a replace; elsewhere the
        // path is the best available identity.
        let meta = std::fs::metadata("/proc/self/exe")
            .or_else(|_| std::fs::metadata(&path))
            .ok()?;
        Some(Self {
            path,
            start: Ident::of(&meta),
        })
    }

    /// Watches the file at `path` as it is now (tests, and callers that know the path).
    pub fn of_path(path: &Path) -> Option<Self> {
        Some(Self {
            path: path.to_path_buf(),
            start: Ident::of(&std::fs::metadata(path).ok()?),
        })
    }

    /// `Some(reason)` when the file at the watched path is no longer the one that started.
    pub fn stale(&self) -> Option<String> {
        match std::fs::metadata(&self.path) {
            Err(_) => Some(format!(
                "the executable {} was removed after this process started",
                self.path.display()
            )),
            Ok(m) if Ident::of(&m) != self.start => Some(format!(
                "the executable {} was replaced on disk after this process started",
                self.path.display()
            )),
            Ok(_) => None,
        }
    }
}

/// The fix shown wherever a stale or mismatched daemon is reported. Stopping the daemon
/// ends its active runs, so say to let them finish first.
pub fn restart_hint(pid: Option<u32>) -> String {
    let stop = match pid {
        Some(pid) => format!("`kill {pid}`"),
        None => "stop the `superorchestrator daemon` process".to_string(),
    };
    format!(
        "restart the daemon: {stop} once no run is active; the next superorchestrator command starts the new binary"
    )
}

/// Compares this process's build with the daemon's. `None` when they match; otherwise one
/// sentence with the fix. `daemon_build` is `None` for a daemon that predates build ids.
/// A `stale` daemon is reported even when the ids match.
pub fn mismatch(
    local_build: &str,
    daemon_build: Option<&str>,
    stale: bool,
    daemon_pid: Option<u32>,
) -> Option<String> {
    let what = match daemon_build {
        None => "the running daemon predates build ids, so it is an older build than this binary"
            .to_string(),
        Some(d) if d != local_build => {
            format!("the running daemon is build {d} but this binary is build {local_build}")
        }
        Some(_) if stale => {
            "the running daemon's executable was replaced on disk after it started".to_string()
        }
        Some(_) => return None,
    };
    Some(format!("{what}. {}", restart_hint(daemon_pid)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn copy_self(dir: &Path, name: &str) -> PathBuf {
        let to = dir.join(name);
        std::fs::copy(std::env::current_exe().unwrap(), &to).unwrap();
        to
    }

    #[test]
    fn build_id_is_stable_and_known() {
        assert_eq!(build_id(), build_id());
        assert_ne!(build_id(), "unknown");
    }

    #[test]
    fn mismatch_cases() {
        assert_eq!(mismatch("a", Some("a"), false, None), None);
        assert!(
            mismatch("a", Some("b"), false, Some(9))
                .unwrap()
                .contains("build b but this binary is build a")
        );
        assert!(
            mismatch("a", Some("b"), false, Some(9))
                .unwrap()
                .contains("`kill 9`")
        );
        assert!(
            mismatch("a", None, false, None)
                .unwrap()
                .contains("predates")
        );
        assert!(
            mismatch("a", Some("a"), true, None)
                .unwrap()
                .contains("replaced on disk")
        );
    }

    #[test]
    fn untouched_executable_is_not_stale() {
        let d = tempfile::tempdir().unwrap();
        let w = ExeWatch::of_path(&copy_self(d.path(), "bin")).unwrap();
        assert_eq!(w.stale(), None);
    }

    #[test]
    fn replaced_by_rename_is_stale() {
        let d = tempfile::tempdir().unwrap();
        let bin = copy_self(d.path(), "bin");
        let w = ExeWatch::of_path(&bin).unwrap();
        let new = d.path().join("new");
        std::fs::write(&new, b"a different binary").unwrap();
        std::fs::rename(&new, &bin).unwrap();
        assert!(w.stale().unwrap().contains("replaced"));
    }

    #[test]
    fn rewritten_in_place_is_stale() {
        let d = tempfile::tempdir().unwrap();
        let bin = copy_self(d.path(), "bin");
        let w = ExeWatch::of_path(&bin).unwrap();
        std::fs::write(&bin, b"short").unwrap();
        assert!(w.stale().is_some());
    }

    #[test]
    fn removed_is_stale() {
        let d = tempfile::tempdir().unwrap();
        let bin = copy_self(d.path(), "bin");
        let w = ExeWatch::of_path(&bin).unwrap();
        std::fs::remove_file(&bin).unwrap();
        assert!(w.stale().unwrap().contains("removed"));
    }
}

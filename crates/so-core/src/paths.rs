//! Filesystem layout of the superorchestrator config directory (SPEC 16) and the supervisor socket.

use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};

use crate::error::{CoreError, Result};

/// Environment lookup, injectable so tests never touch the process environment.
pub type EnvFn<'a> = &'a dyn Fn(&str) -> Option<String>;

fn process_env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

fn non_empty(env: EnvFn<'_>, key: &str) -> Option<String> {
    env(key).filter(|v| !v.is_empty())
}

/// Operating system family, for the socket location.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Linux,
    MacOs,
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Linux
        }
    }
}

/// Expands a leading `~` or `~/` using `HOME`.
pub fn expand_tilde(s: &str) -> Result<PathBuf> {
    expand_tilde_with(&process_env, s)
}

/// [`expand_tilde`] with an injected environment. Errors when the path starts
/// with `~` and `HOME` is unset. `~user` forms are left as they are.
pub fn expand_tilde_with(env: EnvFn<'_>, s: &str) -> Result<PathBuf> {
    if s != "~" && !s.starts_with("~/") {
        return Ok(PathBuf::from(s));
    }
    let home = non_empty(env, "HOME").ok_or(CoreError::NoHomeDir)?;
    Ok(match s.strip_prefix("~/") {
        Some(rest) => PathBuf::from(home).join(rest),
        None => PathBuf::from(home),
    })
}

/// Paths under the config directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub config_dir: PathBuf,
}

impl Paths {
    /// Resolves the config directory from the process environment.
    pub fn resolve() -> Result<Self> {
        Self::resolve_with(&process_env, Platform::current())
    }

    /// Resolves the config directory: `SUPERORCHESTRATOR_CONFIG_DIR`, then (Linux)
    /// `$XDG_CONFIG_HOME/superorchestrator`, then `~/.config/superorchestrator`.
    pub fn resolve_with(env: EnvFn<'_>, platform: Platform) -> Result<Self> {
        if let Some(dir) = non_empty(env, "SUPERORCHESTRATOR_CONFIG_DIR") {
            return Ok(Self::from_dir(expand_tilde_with(env, &dir)?));
        }
        if platform == Platform::Linux
            && let Some(xdg) = non_empty(env, "XDG_CONFIG_HOME")
            && Path::new(&xdg).is_absolute()
        {
            return Ok(Self::from_dir(PathBuf::from(xdg).join("superorchestrator")));
        }
        let home = non_empty(env, "HOME").ok_or(CoreError::NoHomeDir)?;
        Ok(Self::from_dir(
            PathBuf::from(home)
                .join(".config")
                .join("superorchestrator"),
        ))
    }

    pub fn from_dir(dir: PathBuf) -> Self {
        Self { config_dir: dir }
    }

    pub fn config_toml(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }
    pub fn models_toml(&self) -> PathBuf {
        self.config_dir.join("models.toml")
    }
    pub fn accounts_dir(&self) -> PathBuf {
        self.config_dir.join("accounts")
    }
    pub fn account_file(&self, name: &str) -> PathBuf {
        self.accounts_dir().join(format!("{name}.toml"))
    }
    pub fn agents_dir(&self) -> PathBuf {
        self.config_dir.join("agents")
    }
    pub fn workflows_dir(&self) -> PathBuf {
        self.config_dir.join("workflows")
    }
    pub fn state_db(&self) -> PathBuf {
        self.config_dir.join("state.db")
    }
    pub fn runs_dir(&self) -> PathBuf {
        self.config_dir.join("runs")
    }
    pub fn run_dir(&self, run_id: &str) -> PathBuf {
        self.runs_dir().join(run_id)
    }
    /// The run's private temp dir, exposed to its harness as `TMPDIR` and removed when the run ends.
    pub fn run_tmp_dir(&self, run_id: &str) -> PathBuf {
        run_tmp_dir(&self.run_dir(run_id))
    }
    pub fn logs_dir(&self) -> PathBuf {
        self.config_dir.join("logs")
    }
    pub fn wrappers_bak_dir(&self) -> PathBuf {
        self.config_dir.join("wrappers.bak")
    }
    pub fn legacy_backup_dir(&self, ts: &str) -> PathBuf {
        self.config_dir.join("legacy-backup").join(ts)
    }

    /// Supervisor socket path from the process environment.
    pub fn socket_path() -> Result<PathBuf> {
        socket_path_with(&process_env, Platform::current())
    }
}

/// The temp dir of the run whose state dir is `run_dir`.
pub fn run_tmp_dir(run_dir: &Path) -> PathBuf {
    run_dir.join("tmp")
}

/// Socket path: `SUPERORCHESTRATOR_SOCKET`; else on Linux `$XDG_RUNTIME_DIR/superorchestrator.sock`
/// (or `/run/user/<uid>/superorchestrator.sock` when the env is absent and safe, with
/// fallback `/tmp/superorchestrator-<uid>/superorchestrator.sock`); on macOS
/// `~/Library/Caches/superorchestrator/sock`.
pub fn socket_path_with(env: EnvFn<'_>, platform: Platform) -> Result<PathBuf> {
    if let Some(p) = non_empty(env, "SUPERORCHESTRATOR_SOCKET") {
        return expand_tilde_with(env, &p);
    }
    match platform {
        Platform::MacOs => {
            let home = non_empty(env, "HOME").ok_or(CoreError::NoHomeDir)?;
            Ok(PathBuf::from(home)
                .join("Library/Caches/superorchestrator")
                .join("sock"))
        }
        Platform::Linux => {
            if let Some(rt) = non_empty(env, "XDG_RUNTIME_DIR") {
                if Path::new(&rt).is_absolute() {
                    return Ok(PathBuf::from(rt).join("superorchestrator.sock"));
                }
                let uid = rustix::process::geteuid().as_raw();
                return Ok(PathBuf::from(format!("/tmp/superorchestrator-{uid}"))
                    .join("superorchestrator.sock"));
            }
            let uid = rustix::process::geteuid().as_raw();
            let standard_dir = PathBuf::from(format!("/run/user/{uid}"));
            let tmp_dir = PathBuf::from(format!("/tmp/superorchestrator-{uid}"));
            Ok(resolve_linux_socket(
                &standard_dir,
                &tmp_dir,
                |dir| is_safe_runtime_dir(dir, uid),
                socket_answers,
            ))
        }
    }
}

/// Checks that `path` exists, is an actual directory (not a symlink),
/// is owned by `expected_uid`, and is not group- or world-writable.
pub fn is_safe_runtime_dir(path: &Path, expected_uid: u32) -> bool {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return false;
    };
    if !meta.is_dir() {
        return false;
    }
    if meta.uid() != expected_uid {
        return false;
    }
    if (meta.mode() & 0o022) != 0 {
        return false;
    }
    true
}

/// Checks whether a unix domain socket exists at `path` and answers connections.
pub fn socket_answers(path: &Path) -> bool {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return false;
    };
    if !meta.file_type().is_socket() {
        return false;
    }
    std::os::unix::net::UnixStream::connect(path).is_ok()
}

/// Selects between the standard runtime directory and a `/tmp` fallback directory
/// on Linux when `XDG_RUNTIME_DIR` is absent.
///
/// If `standard_dir` is safely usable:
/// - If a supervisor is already answering on `standard_sock`, returns it.
/// - If a supervisor is already answering on `tmp_sock`, returns it to preserve
///   an already-running `/tmp` daemon.
/// - Otherwise returns `standard_sock`.
///
/// If `standard_dir` is not safely usable, falls back to `tmp_sock`.
pub fn resolve_linux_socket(
    standard_dir: &Path,
    tmp_dir: &Path,
    is_safe_dir: impl Fn(&Path) -> bool,
    socket_answers: impl Fn(&Path) -> bool,
) -> PathBuf {
    let standard_sock = standard_dir.join("superorchestrator.sock");
    let tmp_sock = tmp_dir.join("superorchestrator.sock");

    if is_safe_dir(standard_dir) {
        if socket_answers(&standard_sock) {
            return standard_sock;
        }
        if socket_answers(&tmp_sock) {
            return tmp_sock;
        }
        return standard_sock;
    }

    tmp_sock
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let m: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| m.get(k).cloned()
    }

    #[test]
    fn run_tmp_dir_is_under_the_run_dir() {
        let p = Paths::from_dir(PathBuf::from("/c"));
        assert_eq!(p.run_tmp_dir("r1"), PathBuf::from("/c/runs/r1/tmp"));
        assert_eq!(run_tmp_dir(&p.run_dir("r1")), p.run_tmp_dir("r1"));
    }

    #[test]
    fn config_dir_env_override_wins() {
        let env = env_of(&[
            ("SUPERORCHESTRATOR_CONFIG_DIR", "/x/y"),
            ("XDG_CONFIG_HOME", "/xdg"),
            ("HOME", "/home/u"),
        ]);
        let p = Paths::resolve_with(&env, Platform::Linux).unwrap();
        assert_eq!(p.config_dir, PathBuf::from("/x/y"));
    }

    #[test]
    fn config_dir_xdg_on_linux_only() {
        let env = env_of(&[("XDG_CONFIG_HOME", "/xdg"), ("HOME", "/home/u")]);
        assert_eq!(
            Paths::resolve_with(&env, Platform::Linux)
                .unwrap()
                .config_dir,
            PathBuf::from("/xdg/superorchestrator")
        );
        assert_eq!(
            Paths::resolve_with(&env, Platform::MacOs)
                .unwrap()
                .config_dir,
            PathBuf::from("/home/u/.config/superorchestrator")
        );
    }

    #[test]
    fn config_dir_home_default_and_missing_home() {
        let env = env_of(&[("HOME", "/home/u")]);
        assert_eq!(
            Paths::resolve_with(&env, Platform::Linux)
                .unwrap()
                .config_dir,
            PathBuf::from("/home/u/.config/superorchestrator")
        );
        let none = env_of(&[]);
        assert!(Paths::resolve_with(&none, Platform::Linux).is_err());
    }

    #[test]
    fn layout() {
        let p = Paths::from_dir(PathBuf::from("/c"));
        assert_eq!(p.config_toml(), PathBuf::from("/c/config.toml"));
        assert_eq!(p.models_toml(), PathBuf::from("/c/models.toml"));
        assert_eq!(
            p.account_file("codex-acc2"),
            PathBuf::from("/c/accounts/codex-acc2.toml")
        );
        assert_eq!(p.agents_dir(), PathBuf::from("/c/agents"));
        assert_eq!(p.workflows_dir(), PathBuf::from("/c/workflows"));
        assert_eq!(p.state_db(), PathBuf::from("/c/state.db"));
        assert_eq!(p.run_dir("r1"), PathBuf::from("/c/runs/r1"));
        assert_eq!(p.logs_dir(), PathBuf::from("/c/logs"));
        assert_eq!(p.wrappers_bak_dir(), PathBuf::from("/c/wrappers.bak"));
        assert_eq!(
            p.legacy_backup_dir("T"),
            PathBuf::from("/c/legacy-backup/T")
        );
    }

    #[test]
    fn tilde_expansion() {
        let env = env_of(&[("HOME", "/home/u")]);
        assert_eq!(
            expand_tilde_with(&env, "~/.codex").unwrap(),
            PathBuf::from("/home/u/.codex")
        );
        assert_eq!(
            expand_tilde_with(&env, "~").unwrap(),
            PathBuf::from("/home/u")
        );
        assert_eq!(
            expand_tilde_with(&env, "/abs/~/x").unwrap(),
            PathBuf::from("/abs/~/x")
        );
        assert_eq!(
            expand_tilde_with(&env, "~other/x").unwrap(),
            PathBuf::from("~other/x")
        );
        let none = env_of(&[]);
        assert!(expand_tilde_with(&none, "~/x").is_err());
        assert!(expand_tilde_with(&none, "~").is_err());
        assert_eq!(expand_tilde_with(&none, "/x").unwrap(), PathBuf::from("/x"));
    }

    #[test]
    fn socket_selection() {
        let env = env_of(&[
            ("SUPERORCHESTRATOR_SOCKET", "/s.sock"),
            ("XDG_RUNTIME_DIR", "/run/u"),
        ]);
        assert_eq!(
            socket_path_with(&env, Platform::Linux).unwrap(),
            PathBuf::from("/s.sock")
        );

        let env = env_of(&[("XDG_RUNTIME_DIR", "/run/user/1000"), ("HOME", "/h")]);
        assert_eq!(
            socket_path_with(&env, Platform::Linux).unwrap(),
            PathBuf::from("/run/user/1000/superorchestrator.sock")
        );

        let env = env_of(&[("XDG_RUNTIME_DIR", "relative/dir")]);
        let p = socket_path_with(&env, Platform::Linux).unwrap();
        assert!(
            p.to_string_lossy().starts_with("/tmp/superorchestrator-"),
            "{p:?}"
        );

        let env = env_of(&[("HOME", "/home/u")]);
        assert_eq!(
            socket_path_with(&env, Platform::MacOs).unwrap(),
            PathBuf::from("/home/u/Library/Caches/superorchestrator/sock")
        );

        let env = env_of(&[]);
        let p = socket_path_with(&env, Platform::Linux).unwrap();
        let uid = rustix::process::geteuid().as_raw();
        let standard_sock = PathBuf::from(format!("/run/user/{uid}/superorchestrator.sock"));
        let tmp_sock = PathBuf::from(format!(
            "/tmp/superorchestrator-{uid}/superorchestrator.sock"
        ));
        let standard_dir = PathBuf::from(format!("/run/user/{uid}"));
        if is_safe_runtime_dir(&standard_dir, uid) && !socket_answers(&tmp_sock) {
            assert_eq!(p, standard_sock);
        } else {
            assert_eq!(p, tmp_sock);
        }
    }

    #[test]
    fn resolve_linux_socket_prefers_active_standard_socket() {
        let standard_dir = PathBuf::from("/test/run/user/1000");
        let tmp_dir = PathBuf::from("/test/tmp/superorchestrator-1000");
        let standard_sock = standard_dir.join("superorchestrator.sock");
        let tmp_sock = tmp_dir.join("superorchestrator.sock");

        let p = resolve_linux_socket(
            &standard_dir,
            &tmp_dir,
            |_| true,
            |sock| sock == standard_sock || sock == tmp_sock,
        );
        assert_eq!(p, standard_sock);
    }

    #[test]
    fn resolve_linux_socket_preserves_active_tmp_daemon() {
        let standard_dir = PathBuf::from("/test/run/user/1000");
        let tmp_dir = PathBuf::from("/test/tmp/superorchestrator-1000");
        let standard_sock = standard_dir.join("superorchestrator.sock");
        let tmp_sock = tmp_dir.join("superorchestrator.sock");

        let p = resolve_linux_socket(
            &standard_dir,
            &tmp_dir,
            |_| true,
            |sock| sock != standard_sock && sock == tmp_sock,
        );
        assert_eq!(p, tmp_sock);
    }

    #[test]
    fn resolve_linux_socket_uses_safe_standard_dir_when_neither_answers() {
        let standard_dir = PathBuf::from("/test/run/user/1000");
        let tmp_dir = PathBuf::from("/test/tmp/superorchestrator-1000");
        let standard_sock = standard_dir.join("superorchestrator.sock");

        let p = resolve_linux_socket(&standard_dir, &tmp_dir, |_| true, |_| false);
        assert_eq!(p, standard_sock);
    }

    #[test]
    fn resolve_linux_socket_falls_back_to_tmp_when_standard_dir_unsafe() {
        let standard_dir = PathBuf::from("/test/run/user/1000");
        let tmp_dir = PathBuf::from("/test/tmp/superorchestrator-1000");
        let tmp_sock = tmp_dir.join("superorchestrator.sock");

        // Even if a socket at standard_sock answers, an unsafe dir rejects it.
        let p = resolve_linux_socket(&standard_dir, &tmp_dir, |_| false, |_| true);
        assert_eq!(p, tmp_sock);
    }

    #[test]
    fn is_safe_runtime_dir_validates_dir_ownership_and_mode() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let uid = rustix::process::geteuid().as_raw();

        // Valid private dir owned by current user (default 0700)
        assert!(is_safe_runtime_dir(tmp.path(), uid));

        // Wrong expected uid
        assert!(!is_safe_runtime_dir(tmp.path(), uid.wrapping_add(1)));

        // World-writable dir
        std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(!is_safe_runtime_dir(tmp.path(), uid));

        // Group-writable dir
        std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o770)).unwrap();
        assert!(!is_safe_runtime_dir(tmp.path(), uid));

        // Restore safe mode 0700
        std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(is_safe_runtime_dir(tmp.path(), uid));

        // Symlink pointing to the directory is rejected
        let link = tmp.path().join("link_to_dir");
        std::os::unix::fs::symlink(tmp.path(), &link).unwrap();
        assert!(!is_safe_runtime_dir(&link, uid));

        // Regular file is rejected
        let file = tmp.path().join("regular_file");
        std::fs::write(&file, b"data").unwrap();
        assert!(!is_safe_runtime_dir(&file, uid));

        // Non-existent path is rejected
        let missing = tmp.path().join("missing");
        assert!(!is_safe_runtime_dir(&missing, uid));
    }

    #[test]
    fn socket_answers_detects_live_listener_and_stale_or_missing_sockets() {
        let tmp = tempfile::tempdir().unwrap();
        let sock_path = tmp.path().join("test.sock");

        // Non-existent
        assert!(!socket_answers(&sock_path));

        // Regular file
        std::fs::write(&sock_path, b"not a socket").unwrap();
        assert!(!socket_answers(&sock_path));
        std::fs::remove_file(&sock_path).unwrap();

        // Live listening unix socket
        let listener = std::os::unix::net::UnixListener::bind(&sock_path).unwrap();
        assert!(socket_answers(&sock_path));

        // Dropping listener leaves socket file on disk, but nothing answers (stale socket)
        drop(listener);
        assert!(!socket_answers(&sock_path));
    }
}

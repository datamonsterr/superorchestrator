//! Filesystem layout of the superorchestrator config directory (SPEC 16) and the supervisor socket.

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
/// (fallback `/tmp/superorchestrator-<uid>/superorchestrator.sock`); on macOS
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
            if let Some(rt) = non_empty(env, "XDG_RUNTIME_DIR")
                && Path::new(&rt).is_absolute()
            {
                return Ok(PathBuf::from(rt).join("superorchestrator.sock"));
            }
            let uid = rustix::process::geteuid().as_raw();
            Ok(PathBuf::from(format!("/tmp/superorchestrator-{uid}"))
                .join("superorchestrator.sock"))
        }
    }
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
        let s = p.to_string_lossy();
        assert!(s.starts_with("/tmp/superorchestrator-"), "{s}");
        assert!(s.ends_with("/superorchestrator.sock"), "{s}");
    }
}

//! Launch environment per harness (SPEC 5.2, 6.4, ADR-0003).

use std::path::{Path, PathBuf};

use so_core::{AccountFile, Harness, Platform};

use crate::error::{AccountsError, Result};
use crate::host::{HostEnv, same_path};

/// Everything needed to start a harness for one account. The full command is
/// `program argv_prefix... extra_args... <caller args>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub program: PathBuf,
    pub vars: Vec<(String, String)>,
    pub argv_prefix: Vec<String>,
    pub extra_args: Vec<String>,
}

/// Resolves `name` on `path_var`; an absolute (or `/`-containing) name must
/// itself be an executable file.
pub fn resolve_binary(name: &str, path_var: Option<&str>) -> Result<PathBuf> {
    if name.contains('/') {
        let p = PathBuf::from(name);
        return if is_executable(&p) {
            Ok(p)
        } else {
            Err(AccountsError::BinaryNotFound(name.to_string()))
        };
    }
    path_var
        .unwrap_or_default()
        .split(':')
        .filter(|d| !d.is_empty())
        .map(|d| Path::new(d).join(name))
        .find(|p| is_executable(p))
        .ok_or_else(|| AccountsError::BinaryNotFound(name.to_string()))
}

fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// Default opencode data dir; an account whose home is this directory uses the
/// real XDG defaults and sets no variables.
pub fn opencode_default_home(host: &HostEnv) -> PathBuf {
    host.home.join(".local/share/opencode")
}

pub fn claude_default_home(host: &HostEnv) -> PathBuf {
    host.home.join(".claude")
}

/// Environment variables for the harness, without resolving any binary.
pub(crate) fn build_launch_vars(
    account: &AccountFile,
    host: &HostEnv,
) -> Result<Vec<(String, String)>> {
    let home = host.expand(&account.home)?;
    Ok(harness_vars(account, host, &home))
}

fn harness_vars(account: &AccountFile, host: &HostEnv, home: &Path) -> Vec<(String, String)> {
    let s = |p: &Path| p.to_string_lossy().into_owned();
    match account.harness {
        Harness::Codex => vec![("CODEX_HOME".into(), s(home))],
        // The default install keeps the real XDG layout; everything else
        // gets an isolated XDG triple.
        Harness::Opencode if !same_path(home, &opencode_default_home(host)) => vec![
            ("XDG_CONFIG_HOME".into(), s(&home.join("config"))),
            ("XDG_DATA_HOME".into(), s(&home.join("data"))),
            ("XDG_STATE_HOME".into(), s(&home.join("state"))),
        ],
        Harness::Claude if !same_path(home, &claude_default_home(host)) => {
            vec![("CLAUDE_CONFIG_DIR".into(), s(home))]
        }
        _ => Vec::new(),
    }
}

/// Builds the launch recipe, the same for supervised runs and interactive wrapper
/// launches. No recipe needs bwrap (ADR-0005): agy reads its account from
/// `--gemini_dir=<home>/.gemini` and never binds it over `~/.gemini`; the agy driver
/// adds its own bwrap wrapper for supervised runs.
pub fn build_launch(account: &AccountFile, host: &HostEnv) -> Result<Launch> {
    let home = host.expand(&account.home)?;
    let binary = resolve_binary(&account.binary, host.path_var.as_deref())?;
    let mut launch = Launch {
        program: binary,
        vars: Vec::new(),
        argv_prefix: Vec::new(),
        extra_args: Vec::new(),
    };
    launch.vars = harness_vars(account, host, &home);
    match account.harness {
        Harness::Codex | Harness::Opencode | Harness::Claude => {}
        Harness::Agy => {
            let gemini = home.join(".gemini");
            let real = host.home.join(".gemini");
            if !same_path(&gemini, &real) {
                // Multi-account agy is Linux-only until checked on a Mac
                // (SPEC 6.4).
                if host.platform == Platform::MacOs {
                    return Err(AccountsError::AgyMultiAccountUnsupported(
                        account.name.clone(),
                    ));
                }
                if !gemini.is_dir() {
                    return Err(AccountsError::HomeMissing(gemini));
                }
            }
            launch
                .extra_args
                .push(format!("--gemini_dir={}", gemini.to_string_lossy()));
        }
    }
    Ok(launch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{host, stub_bin};

    fn acct(name: &str, h: Harness, home: &str) -> AccountFile {
        AccountFile::new(name, h, home, h.as_str()).unwrap()
    }

    #[test]
    fn codex_sets_codex_home() {
        let d = tempfile::tempdir().unwrap();
        stub_bin(d.path(), "codex");
        let l = build_launch(
            &acct("codex-acc2", Harness::Codex, "~/.codex-acc2"),
            &host(d.path(), Platform::Linux),
        )
        .unwrap();
        assert_eq!(l.program, d.path().join("bin/codex"));
        assert_eq!(
            l.vars,
            vec![(
                "CODEX_HOME".to_string(),
                d.path().join(".codex-acc2").display().to_string()
            )]
        );
        assert!(l.argv_prefix.is_empty() && l.extra_args.is_empty());
    }

    #[test]
    fn opencode_xdg_triple_and_default() {
        let d = tempfile::tempdir().unwrap();
        stub_bin(d.path(), "opencode");
        let h = host(d.path(), Platform::Linux);
        let l = build_launch(&acct("oc-two", Harness::Opencode, "~/.oc-two"), &h).unwrap();
        let keys: Vec<_> = l.vars.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME"]);
        assert_eq!(l.vars[1].1, format!("{}/.oc-two/data", d.path().display()));
        let l = build_launch(
            &acct(
                "opencode-acc1",
                Harness::Opencode,
                "~/.local/share/opencode",
            ),
            &h,
        )
        .unwrap();
        assert!(l.vars.is_empty());
    }

    #[test]
    fn claude_config_dir_and_default() {
        let d = tempfile::tempdir().unwrap();
        stub_bin(d.path(), "claude");
        let h = host(d.path(), Platform::Linux);
        let l = build_launch(&acct("claude-two", Harness::Claude, "~/.claude-two"), &h).unwrap();
        assert_eq!(l.vars[0].0, "CLAUDE_CONFIG_DIR");
        let l = build_launch(&acct("claude-acc1", Harness::Claude, "~/.claude"), &h).unwrap();
        assert!(l.vars.is_empty());
    }

    #[test]
    fn agy_never_binds_gemini_over_home() {
        let d = tempfile::tempdir().unwrap();
        let agy = stub_bin(d.path(), "agy");
        let h = host(d.path(), Platform::Linux);
        let a = acct("agy-acc2", Harness::Agy, "~/.agy2");
        assert!(matches!(
            build_launch(&a, &h),
            Err(AccountsError::HomeMissing(_))
        ));
        std::fs::create_dir_all(d.path().join(".agy2/.gemini")).unwrap();
        let l = build_launch(&a, &h).unwrap();
        assert_eq!(l.program, agy);
        assert!(l.argv_prefix.is_empty(), "no bwrap, no --bind");
        assert!(l.vars.is_empty(), "HOME must stay unchanged");
        assert_eq!(
            l.extra_args,
            [format!("--gemini_dir={}/.agy2/.gemini", d.path().display())]
        );
    }

    #[test]
    fn agy_real_gemini_needs_no_bwrap_and_macos_errors() {
        let d = tempfile::tempdir().unwrap();
        let agy = stub_bin(d.path(), "agy");
        let real = acct("agy-acc1", Harness::Agy, "~");
        for p in [Platform::Linux, Platform::MacOs] {
            let l = build_launch(&real, &host(d.path(), p)).unwrap();
            assert_eq!(l.program, agy);
            assert!(l.argv_prefix.is_empty());
            assert_eq!(
                l.extra_args,
                [format!("--gemini_dir={}/.gemini", d.path().display())]
            );
        }
        let other = acct("agy-acc2", Harness::Agy, "~/.agy2");
        assert!(matches!(
            build_launch(&other, &host(d.path(), Platform::MacOs)),
            Err(AccountsError::AgyMultiAccountUnsupported(_))
        ));
    }

    #[test]
    fn resolve_binary_path_and_absolute() {
        let d = tempfile::tempdir().unwrap();
        let p = stub_bin(d.path(), "tool");
        let path = format!("/nonexistent:{}", d.path().join("bin").display());
        assert_eq!(resolve_binary("tool", Some(&path)).unwrap(), p);
        assert_eq!(resolve_binary(p.to_str().unwrap(), None).unwrap(), p);
        assert!(resolve_binary("nope", Some(&path)).is_err());
        assert!(resolve_binary("tool", None).is_err());
        // Present but not executable does not count.
        std::fs::write(d.path().join("bin/plain"), "x").unwrap();
        assert!(resolve_binary("plain", Some(&path)).is_err());
    }
}

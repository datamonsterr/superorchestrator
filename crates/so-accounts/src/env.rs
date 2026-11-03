//! Launch environment per harness (SPEC 5.2, 6.4, ADR-0003).

use std::path::{Path, PathBuf};

use so_core::{AccountFile, Endpoint, Harness, Platform, Slot};

use crate::error::{AccountsError, Result};
use crate::host::{HostEnv, same_path};

/// Everything needed to start a harness for one account. The full command is
/// `program argv_prefix... extra_args... <caller args>`.
#[derive(Clone, PartialEq, Eq)]
pub struct Launch {
    pub program: PathBuf,
    /// Extra inherited names, resolved only when the process starts.
    pub pass_env: Vec<String>,
    pub vars: Vec<(String, String)>,
    pub env_remove: Vec<String>,
    pub argv_prefix: Vec<String>,
    pub extra_args: Vec<String>,
}

impl std::fmt::Debug for Launch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let vars: Vec<_> = self
            .vars
            .iter()
            .map(|(k, v)| {
                (
                    k,
                    if matches!(k.as_str(), "ANTHROPIC_AUTH_TOKEN" | "ANTHROPIC_API_KEY") {
                        "[REDACTED]"
                    } else {
                        v.as_str()
                    },
                )
            })
            .collect();
        f.debug_struct("Launch")
            .field("program", &self.program)
            .field("pass_env", &self.pass_env)
            .field("vars", &vars)
            .field("env_remove", &self.env_remove)
            .field("argv_prefix", &self.argv_prefix)
            .field("extra_args", &self.extra_args)
            .finish()
    }
}

/// Resolve the configured endpoint token without exposing it in diagnostics.
pub fn resolve_endpoint_token(account: &AccountFile, host: &HostEnv) -> Result<String> {
    match &account.endpoint {
        Some(endpoint) => resolve_token(account, endpoint, host),
        None => Err(AccountsError::TokenEnv {
            account: account.name.clone(),
            var: "endpoint not configured".into(),
        }),
    }
}

pub(crate) fn resolve_token(
    account: &AccountFile,
    endpoint: &Endpoint,
    host: &HostEnv,
) -> Result<String> {
    use std::os::unix::fs::PermissionsExt;
    if let Some(var) = &endpoint.token_env {
        return host
            .vars
            .get(var)
            .filter(|v| !v.is_empty())
            .cloned()
            .ok_or_else(|| AccountsError::TokenEnv {
                account: account.name.clone(),
                var: var.clone(),
            });
    }
    let source = endpoint.token_file.as_deref().unwrap_or_default();
    let err = || AccountsError::TokenFile {
        account: account.name.clone(),
        path: source.to_string(),
    };
    let path = host.expand(source).map_err(|_| err())?;
    // Avoid blocking on a FIFO before opening. Recheck the opened target below.
    if !std::fs::metadata(&path).map_err(|_| err())?.is_file() {
        return Err(err());
    }
    let mut file = std::fs::File::open(path).map_err(|_| err())?;
    let metadata = file.metadata().map_err(|_| err())?;
    let mode = metadata.permissions().mode() & 0o777;
    if !metadata.is_file() || (mode != 0o600 && mode != 0o400) {
        return Err(err());
    }
    let mut token = String::new();
    std::io::Read::read_to_string(&mut file, &mut token).map_err(|_| err())?;
    let token = token.trim_end().to_string();
    if token.is_empty() {
        return Err(err());
    }
    Ok(token)
}

impl Launch {
    /// Adds global inherited names to the account's own pass-through list.
    pub fn with_default_pass_env(mut self, defaults: &[String]) -> Self {
        self.pass_env = so_core::child_env::merge_pass_env([self.pass_env.as_slice(), defaults]);
        self
    }
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

pub(crate) fn is_executable(p: &Path) -> bool {
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
    let mut vars = harness_vars(account, host, &home);
    if let Some(endpoint) = &account.endpoint {
        let token = resolve_token(account, endpoint, host)?;
        vars.extend([
            ("ANTHROPIC_BASE_URL".into(), endpoint.base_url().into()),
            (endpoint.token_var().into(), token),
            ("ANTHROPIC_MODEL".into(), endpoint.slot(Slot::Opus).into()),
            (
                "ANTHROPIC_DEFAULT_OPUS_MODEL".into(),
                endpoint.slot(Slot::Opus).into(),
            ),
            (
                "ANTHROPIC_DEFAULT_SONNET_MODEL".into(),
                endpoint.slot(Slot::Sonnet).into(),
            ),
            (
                "ANTHROPIC_DEFAULT_HAIKU_MODEL".into(),
                endpoint.slot(Slot::Haiku).into(),
            ),
        ]);
        vars.extend(
            endpoint
                .extra_env()
                .iter()
                .map(|(k, v)| ((*k).into(), (*v).into())),
        );
    }
    Ok(vars)
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
        Harness::Claude
            if account.endpoint.is_some() || !same_path(home, &claude_default_home(host)) =>
        {
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
        pass_env: account.pass_env.clone(),
        vars: Vec::new(),
        env_remove: account
            .endpoint
            .as_ref()
            .map(|e| vec![e.other_token_var().into()])
            .unwrap_or_default(),
        argv_prefix: Vec::new(),
        extra_args: Vec::new(),
    };
    launch.vars = build_launch_vars(account, host)?;
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

    #[test]
    fn endpoint_launch_reads_private_token_file() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        stub_bin(d.path(), "claude");
        let key = d.path().join("key");
        std::fs::write(&key, "secret-value\n").unwrap();
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
        let mut a = acct("endpoint", Harness::Claude, "~/.endpoint");
        a.endpoint = Some(so_core::Endpoint {
            preset: so_core::EndpointPreset::Kimi,
            base_url: None,
            token_env: None,
            token_file: Some(key.display().to_string()),
            token_var: None,
            models: vec![],
            slots: Default::default(),
        });
        let launch = build_launch(&a, &host(d.path(), Platform::Linux)).unwrap();
        assert!(
            launch
                .vars
                .contains(&("ANTHROPIC_API_KEY".into(), "secret-value".into()))
        );
        assert!(!format!("{launch:?}").contains("secret-value"));
        let link = d.path().join("key-link");
        std::os::unix::fs::symlink(&key, &link).unwrap();
        a.endpoint.as_mut().unwrap().token_file = Some(link.display().to_string());
        assert!(build_launch(&a, &host(d.path(), Platform::Linux)).is_ok());
    }

    #[test]
    fn endpoint_rejects_directory_as_token_file() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let key = d.path().join("key-directory");
        std::fs::create_dir(&key).unwrap();
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
        let mut a = acct("endpoint", Harness::Claude, "~/.endpoint");
        let endpoint = so_core::Endpoint {
            preset: so_core::EndpointPreset::Kimi,
            base_url: None,
            token_env: None,
            token_file: Some(key.display().to_string()),
            token_var: None,
            models: vec![],
            slots: Default::default(),
        };
        a.endpoint = Some(endpoint.clone());
        let error = resolve_token(&a, &endpoint, &host(d.path(), Platform::Linux)).unwrap_err();
        assert!(matches!(error, AccountsError::TokenFile { .. }));
    }

    fn acct(name: &str, h: Harness, home: &str) -> AccountFile {
        AccountFile::new(name, h, home, h.as_str()).unwrap()
    }

    #[test]
    fn endpoint_presets_and_token_errors() {
        use so_core::{Endpoint, EndpointPreset, Slots};
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        stub_bin(d.path(), "claude");
        let mut h = host(d.path(), Platform::Linux);
        h.vars
            .insert("PROVIDER_KEY".into(), "sensitive-token".into());
        for preset in [
            EndpointPreset::Zai,
            EndpointPreset::Kimi,
            EndpointPreset::Minimax,
            EndpointPreset::Custom,
        ] {
            let mut a = acct("ep", Harness::Claude, "~/.ep");
            a.endpoint = Some(Endpoint {
                preset,
                base_url: (preset == EndpointPreset::Custom).then(|| "https://example.org".into()),
                token_env: Some("PROVIDER_KEY".into()),
                token_file: None,
                token_var: None,
                models: vec!["m".into()],
                slots: if preset == EndpointPreset::Custom {
                    Slots {
                        opus: Some("o".into()),
                        sonnet: Some("s".into()),
                        haiku: Some("h".into()),
                    }
                } else {
                    Slots::default()
                },
            });
            let ep = a.endpoint.as_ref().unwrap();
            let launch = build_launch(&a, &h).unwrap();
            assert_eq!(launch.env_remove, [ep.other_token_var()]);
            assert!(
                launch
                    .vars
                    .contains(&(ep.token_var().into(), "sensitive-token".into()))
            );
            assert!(
                launch
                    .vars
                    .contains(&("ANTHROPIC_MODEL".into(), ep.slot(Slot::Opus).into()))
            );
            for (key, slot) in [
                ("ANTHROPIC_DEFAULT_OPUS_MODEL", Slot::Opus),
                ("ANTHROPIC_DEFAULT_SONNET_MODEL", Slot::Sonnet),
                ("ANTHROPIC_DEFAULT_HAIKU_MODEL", Slot::Haiku),
            ] {
                assert!(launch.vars.contains(&(key.into(), ep.slot(slot).into())));
            }
            if preset == EndpointPreset::Zai {
                assert!(
                    launch
                        .vars
                        .contains(&("API_TIMEOUT_MS".into(), "3000000".into()))
                );
                assert!(launch.vars.contains(&(
                    "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC".into(),
                    "1".into()
                )));
            }
            assert!(!format!("{launch:?}").contains("sensitive-token"));
            assert!(!so_core::redact::redact(&format!("{launch:?}")).contains("sensitive-token"));
            let joined = launch
                .vars
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join(" ");
            assert!(!so_core::redact::redact(&joined).contains("sensitive-token"));
            h.vars.clear();
            let err = build_launch(&a, &h).unwrap_err().to_string();
            assert!(
                err.contains("ep")
                    && err.contains("PROVIDER_KEY")
                    && !err.contains("sensitive-token")
            );
            h.vars
                .insert("PROVIDER_KEY".into(), "sensitive-token".into());
        }
        let key = d.path().join("key");
        std::fs::write(&key, "sensitive-token").unwrap();
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o644)).unwrap();
        let mut a = acct("ep", Harness::Claude, "~/.ep");
        a.endpoint = Some(Endpoint {
            preset: EndpointPreset::Kimi,
            base_url: None,
            token_env: None,
            token_file: Some(key.display().to_string()),
            token_var: None,
            models: vec![],
            slots: Slots::default(),
        });
        let error = build_launch(&a, &h).unwrap_err().to_string();
        assert!(
            error.contains("ep") && error.contains("key") && !error.contains("sensitive-token")
        );
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
        assert!(l.env_remove.is_empty());
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
        assert!(l.env_remove.is_empty());
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

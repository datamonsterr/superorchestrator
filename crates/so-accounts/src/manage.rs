//! High-level account operations used by the CLI (SPEC 5.4).

use std::path::{Path, PathBuf};

use so_core::{AccountFile, Config, Harness, Paths};

use crate::env::build_launch_vars;
use crate::error::{AccountsError, Result};
use crate::host::{HostEnv, same_path};
use crate::wrapper::{remove_wrapper, write_wrapper};

const SELF_EXE: &str = "superorchestrator";

fn wrapper_dir(cfg: &Config, host: &HostEnv) -> Result<PathBuf> {
    host.expand(&cfg.accounts.wrapper_dir)
}

fn ensure_new(paths: &Paths, name: &str) -> Result<()> {
    if paths.account_file(name).exists() {
        return Err(AccountsError::AlreadyExists(name.to_string()));
    }
    Ok(())
}

fn finish(
    paths: &Paths,
    cfg: &Config,
    host: &HostEnv,
    account: &AccountFile,
) -> Result<AccountFile> {
    account.save(paths)?;
    write_wrapper(
        &wrapper_dir(cfg, host)?,
        &paths.wrappers_bak_dir(),
        &account.name,
        SELF_EXE,
    )?;
    Ok(account.clone())
}

/// Creates the credential home (agy: with `.gemini/`), the account file and
/// the wrapper. The home defaults to `~/.<name>`.
pub fn add(
    paths: &Paths,
    cfg: &Config,
    host: &HostEnv,
    harness: Harness,
    name: &str,
    home: Option<&str>,
) -> Result<AccountFile> {
    let home_spec = home.map_or_else(|| format!("~/.{name}"), str::to_string);
    let account = AccountFile::new(name, harness, &home_spec, harness.as_str())?;
    ensure_new(paths, name)?;
    let dir = host.expand(&home_spec)?;
    let create = if harness == Harness::Agy {
        dir.join(".gemini")
    } else {
        dir
    };
    std::fs::create_dir_all(&create).map_err(|e| AccountsError::io(&create, e))?;
    finish(paths, cfg, host, &account)
}

/// Links an existing credential home without copying anything.
pub fn import(
    paths: &Paths,
    cfg: &Config,
    host: &HostEnv,
    name: &str,
    harness: Harness,
    home: &str,
) -> Result<AccountFile> {
    let account = AccountFile::new(name, harness, home, harness.as_str())?;
    ensure_new(paths, name)?;
    let dir = host.expand(home)?;
    let must_exist = if harness == Harness::Agy {
        dir.join(".gemini")
    } else {
        dir
    };
    if !must_exist.is_dir() {
        return Err(AccountsError::HomeMissing(must_exist));
    }
    finish(paths, cfg, host, &account)
}

/// Homes that belong to the harness defaults and are never purged.
fn protected_homes(host: &HostEnv) -> Vec<PathBuf> {
    [".codex", ".claude", ".gemini", ".local/share/opencode"]
        .iter()
        .map(|r| host.home.join(r))
        .collect()
}

fn check_purge(paths: &Paths, host: &HostEnv, account: &AccountFile, home: &Path) -> Result<()> {
    let unsafe_ = |reason: &str| AccountsError::UnsafePurge {
        path: home.to_path_buf(),
        reason: reason.to_string(),
    };
    if home == host.home || !home.starts_with(&host.home) {
        return Err(unsafe_("the credential home must be inside $HOME"));
    }
    if home.components().any(|c| c.as_os_str() == "..") {
        return Err(unsafe_("path contains `..`"));
    }
    for p in protected_homes(host) {
        // Equal to, or an ancestor of, a harness default home.
        if same_path(home, &p) || p.starts_with(home) {
            return Err(unsafe_("it is a harness default home"));
        }
        if let (Ok(c), Ok(h)) = (home.canonicalize(), p.canonicalize())
            && (c == h || h.starts_with(&c))
        {
            return Err(unsafe_("it resolves to a harness default home"));
        }
    }
    for other in AccountFile::list(paths)? {
        if other.name != account.name && host.expand(&other.home).is_ok_and(|h| same_path(&h, home))
        {
            return Err(unsafe_(&format!(
                "account `{}` shares this home",
                other.name
            )));
        }
    }
    Ok(())
}

/// Removes the account file and its managed wrapper. With `purge`, also
/// deletes the credential home, after safety checks that run first so a
/// refusal changes nothing.
pub fn remove(paths: &Paths, cfg: &Config, host: &HostEnv, name: &str, purge: bool) -> Result<()> {
    let file = paths.account_file(name);
    if !file.exists() {
        return Err(AccountsError::NotFound(name.to_string()));
    }
    let account = AccountFile::load(&file)?;
    let home = host.expand(&account.home)?;
    if purge {
        check_purge(paths, host, &account, &home)?;
    }
    std::fs::remove_file(&file).map_err(|e| AccountsError::io(&file, e))?;
    remove_wrapper(&wrapper_dir(cfg, host)?, name)?;
    if purge && home.symlink_metadata().is_ok() {
        let is_link = home.symlink_metadata().is_ok_and(|m| m.is_symlink());
        let r = if is_link {
            std::fs::remove_file(&home)
        } else {
            std::fs::remove_dir_all(&home)
        };
        r.map_err(|e| AccountsError::io(&home, e))?;
    }
    Ok(())
}

pub fn set_enabled(paths: &Paths, name: &str, enabled: bool) -> Result<AccountFile> {
    let file = paths.account_file(name);
    if !file.exists() {
        return Err(AccountsError::NotFound(name.to_string()));
    }
    let mut account = AccountFile::load(&file)?;
    account.enabled = enabled;
    account.save(paths)?;
    Ok(account)
}

/// How to log in to an account. The caller runs `argv` with `env` added.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginPlan {
    pub argv: Vec<String>,
    pub env: Vec<(String, String)>,
    pub needs_tty: bool,
    pub note: Option<String>,
}

pub fn login_command(account: &AccountFile, host: &HostEnv, has_tty: bool) -> Result<LoginPlan> {
    let bin = account.binary.clone();
    let wrapper_note = |msg: &str| Some(msg.to_string());
    let env = build_launch_vars(account, host)?;
    Ok(match account.harness {
        Harness::Codex => LoginPlan {
            argv: if has_tty {
                vec![bin, "login".into()]
            } else {
                vec![bin, "login".into(), "--device-auth".into()]
            },
            env,
            needs_tty: has_tty,
            note: None,
        },
        Harness::Opencode => LoginPlan {
            argv: vec![bin, "auth".into(), "login".into()],
            env,
            needs_tty: true,
            note: None,
        },
        Harness::Claude => LoginPlan {
            argv: vec![account.name.clone()],
            env: Vec::new(),
            needs_tty: true,
            note: wrapper_note("run `/login` inside the session"),
        },
        Harness::Agy => LoginPlan {
            argv: vec![account.name.clone()],
            env: Vec::new(),
            needs_tty: true,
            note: wrapper_note("sign in from inside the agy session"),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::host;
    use so_core::Platform;

    struct Fx {
        d: tempfile::TempDir,
        paths: Paths,
        cfg: Config,
        host: HostEnv,
    }

    fn fx() -> Fx {
        let d = tempfile::tempdir().unwrap();
        let paths = Paths::from_dir(d.path().join("cfg"));
        let host = host(d.path(), Platform::Linux);
        let mut cfg = Config::default();
        cfg.accounts.wrapper_dir = "~/.local/bin".into();
        Fx {
            d,
            paths,
            cfg,
            host,
        }
    }

    #[test]
    fn add_creates_home_toml_wrapper() {
        let f = fx();
        let a = add(
            &f.paths,
            &f.cfg,
            &f.host,
            Harness::Codex,
            "codex-acc7",
            None,
        )
        .unwrap();
        assert_eq!(a.home, "~/.codex-acc7");
        assert!(f.d.path().join(".codex-acc7").is_dir());
        assert!(f.paths.account_file("codex-acc7").exists());
        assert!(crate::is_managed(&f.d.path().join(".local/bin/codex-acc7")));
        let g = add(
            &f.paths,
            &f.cfg,
            &f.host,
            Harness::Agy,
            "agy-acc7",
            Some("~/.agy7"),
        )
        .unwrap();
        assert_eq!(g.pools, vec!["gemini", "claude"]);
        assert!(f.d.path().join(".agy7/.gemini").is_dir());
        assert!(matches!(
            add(
                &f.paths,
                &f.cfg,
                &f.host,
                Harness::Codex,
                "codex-acc7",
                None
            ),
            Err(AccountsError::AlreadyExists(_))
        ));
        assert!(add(&f.paths, &f.cfg, &f.host, Harness::Codex, "codex", None).is_err());
        assert!(add(&f.paths, &f.cfg, &f.host, Harness::Claude, "Bad Name", None).is_err());
    }

    #[test]
    fn import_requires_existing_home_and_copies_nothing() {
        let f = fx();
        assert!(matches!(
            import(
                &f.paths,
                &f.cfg,
                &f.host,
                "codex-acc2",
                Harness::Codex,
                "~/.codex-acc2"
            ),
            Err(AccountsError::HomeMissing(_))
        ));
        std::fs::create_dir_all(f.d.path().join(".codex-acc2")).unwrap();
        import(
            &f.paths,
            &f.cfg,
            &f.host,
            "codex-acc2",
            Harness::Codex,
            "~/.codex-acc2",
        )
        .unwrap();
        assert_eq!(
            std::fs::read_dir(f.d.path().join(".codex-acc2"))
                .unwrap()
                .count(),
            0
        );
        // agy needs <home>/.gemini
        std::fs::create_dir_all(f.d.path().join("p1")).unwrap();
        assert!(import(&f.paths, &f.cfg, &f.host, "agy-acc1", Harness::Agy, "~/p1").is_err());
        std::fs::create_dir_all(f.d.path().join("p1/.gemini")).unwrap();
        import(&f.paths, &f.cfg, &f.host, "agy-acc1", Harness::Agy, "~/p1").unwrap();
    }

    #[test]
    fn remove_keeps_home_unless_purged_and_keeps_foreign_wrapper() {
        let f = fx();
        add(
            &f.paths,
            &f.cfg,
            &f.host,
            Harness::Codex,
            "codex-acc7",
            None,
        )
        .unwrap();
        remove(&f.paths, &f.cfg, &f.host, "codex-acc7", false).unwrap();
        assert!(!f.paths.account_file("codex-acc7").exists());
        assert!(!f.d.path().join(".local/bin/codex-acc7").exists());
        assert!(f.d.path().join(".codex-acc7").is_dir());

        add(
            &f.paths,
            &f.cfg,
            &f.host,
            Harness::Codex,
            "codex-acc8",
            None,
        )
        .unwrap();
        std::fs::write(f.d.path().join(".codex-acc8/auth.json"), "x").unwrap();
        // Replace the wrapper with a foreign file: it must survive remove.
        std::fs::write(f.d.path().join(".local/bin/codex-acc8"), "foreign").unwrap();
        remove(&f.paths, &f.cfg, &f.host, "codex-acc8", true).unwrap();
        assert!(!f.d.path().join(".codex-acc8").exists());
        assert!(f.d.path().join(".local/bin/codex-acc8").exists());
        assert!(matches!(
            remove(&f.paths, &f.cfg, &f.host, "codex-acc8", false),
            Err(AccountsError::NotFound(_))
        ));
    }

    #[test]
    fn purge_refuses_unsafe_homes_and_changes_nothing() {
        let f = fx();
        let r = f.d.path();
        std::fs::create_dir_all(r.join(".codex")).unwrap();
        std::fs::create_dir_all(r.join(".local/share/opencode")).unwrap();
        std::fs::create_dir_all(r.join("shared")).unwrap();
        std::os::unix::fs::symlink(r.join(".codex"), r.join(".codex-acc1")).unwrap();
        let outside = tempfile::tempdir().unwrap();
        let outside_s = outside.path().to_str().unwrap().to_string();
        let cases = [
            ("codex-a", Harness::Codex, "~/.codex"),
            ("claude-a", Harness::Claude, "~/.claude"),
            ("oc-a", Harness::Opencode, "~/.local/share/opencode"),
            ("oc-b", Harness::Opencode, "~/.local/share"),
            ("codex-b", Harness::Codex, "~/.codex-acc1"), // symlink to default
            ("codex-c", Harness::Codex, "~"),
            ("codex-d", Harness::Codex, outside_s.as_str()),
        ];
        for (name, h, home) in cases {
            std::fs::create_dir_all(r.join(".claude")).unwrap();
            let acc = AccountFile::new(name, h, home, h.as_str()).unwrap();
            acc.save(&f.paths).unwrap();
            let e = remove(&f.paths, &f.cfg, &f.host, name, true).unwrap_err();
            assert!(
                matches!(e, AccountsError::UnsafePurge { .. }),
                "{name}: {e}"
            );
            assert!(f.paths.account_file(name).exists(), "{name} toml kept");
        }
        assert!(r.join(".codex").is_dir() && r.join(".local/share/opencode").is_dir());
        assert!(outside.path().is_dir());

        // Two accounts on one home: purge refused for either.
        for n in ["codex-e", "codex-f"] {
            AccountFile::new(n, Harness::Codex, "~/shared", "codex")
                .unwrap()
                .save(&f.paths)
                .unwrap();
        }
        assert!(remove(&f.paths, &f.cfg, &f.host, "codex-e", true).is_err());
        assert!(r.join("shared").is_dir());
    }

    #[test]
    fn enable_disable() {
        let f = fx();
        add(
            &f.paths,
            &f.cfg,
            &f.host,
            Harness::Codex,
            "codex-acc7",
            None,
        )
        .unwrap();
        assert!(!set_enabled(&f.paths, "codex-acc7", false).unwrap().enabled);
        assert!(
            !AccountFile::load(&f.paths.account_file("codex-acc7"))
                .unwrap()
                .enabled
        );
        assert!(set_enabled(&f.paths, "nope-x", true).is_err());
    }

    #[test]
    fn login_plans() {
        let f = fx();
        let mk = |n, h: Harness, home| AccountFile::new(n, h, home, h.as_str()).unwrap();
        let c = mk("codex-acc2", Harness::Codex, "~/.codex-acc2");
        let p = login_command(&c, &f.host, false).unwrap();
        assert_eq!(p.argv, ["codex", "login", "--device-auth"]);
        assert!(!p.needs_tty);
        assert_eq!(p.env[0].0, "CODEX_HOME");
        let p = login_command(&c, &f.host, true).unwrap();
        assert_eq!(p.argv, ["codex", "login"]);
        assert!(p.needs_tty);
        let o = mk("oc-two", Harness::Opencode, "~/.oc-two");
        let p = login_command(&o, &f.host, true).unwrap();
        assert_eq!(p.argv, ["opencode", "auth", "login"]);
        assert_eq!(p.env.len(), 3);
        let cl = mk("claude-two", Harness::Claude, "~/.claude-two");
        let p = login_command(&cl, &f.host, true).unwrap();
        assert_eq!(p.argv, ["claude-two"]);
        assert!(p.note.unwrap().contains("/login"));
        let a = mk("agy-acc2", Harness::Agy, "~/.agy2");
        let p = login_command(&a, &f.host, true).unwrap();
        assert_eq!(p.argv, ["agy-acc2"]);
        assert!(p.note.is_some());
    }
}

//! High-level account operations used by the CLI (SPEC 5.4).

use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use so_core::{AccountFile, Config, Harness, Paths, Platform};

use crate::env::{build_launch_vars, is_executable};
use crate::error::{AccountsError, Result};
use crate::host::{HostEnv, canon, same_path};
use crate::wrapper::{is_managed, remove_wrapper, write_wrapper};

/// Command name wrappers `exec` into.
pub const SELF_EXE: &str = "superorchestrator";

fn wrapper_dir(cfg: &Config, host: &HostEnv) -> Result<PathBuf> {
    host.expand(&cfg.accounts.wrapper_dir)
}

/// Creates and tightens directories owned by superorchestrator, not credential homes.
/// If `dir` is a symlink (including config_dir), its target is tightened to 0700.
fn ensure_private_dir(dir: &Path) -> Result<()> {
    so_core::fsutil::create_private_dirs(dir)
        .and_then(|()| std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)))
        .map_err(|e| AccountsError::io(dir, e))
}

/// Writes the account file with create-new semantics: an existing file, even
/// one created concurrently, yields `AlreadyExists`.
fn create_account_file(paths: &Paths, account: &AccountFile) -> Result<()> {
    account.validate()?;
    let text = toml::to_string_pretty(account).map_err(so_core::CoreError::from)?;
    let dir = paths.accounts_dir();
    ensure_private_dir(&paths.config_dir)?;
    ensure_private_dir(&dir)?;
    let target = paths.account_file(&account.name);
    let mut n = 0u32;
    let (tmp, mut f) = loop {
        let tmp = dir.join(format!(".{}.tmp.{}.{n}", account.name, std::process::id()));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)
        {
            Ok(f) => break (tmp, f),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => n += 1,
            Err(e) => return Err(AccountsError::io(&tmp, e)),
        }
    };
    let written = f.write_all(text.as_bytes()).and_then(|()| f.sync_all());
    // hard_link fails when the target exists, which makes this create-new.
    let linked = written.and_then(|()| std::fs::hard_link(&tmp, &target));
    let _ = std::fs::remove_file(&tmp);
    match linked {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            Err(AccountsError::AlreadyExists(account.name.clone()))
        }
        Err(e) => Err(AccountsError::io(&target, e)),
    }
}

/// Saves the toml, then the wrapper; a wrapper failure removes the toml again.
fn finish(
    paths: &Paths,
    cfg: &Config,
    host: &HostEnv,
    account: &AccountFile,
) -> Result<AccountFile> {
    create_account_file(paths, account)?;
    let wrote = wrapper_dir(cfg, host)
        .and_then(|dir| write_wrapper(&dir, &paths.wrappers_bak_dir(), &account.name, SELF_EXE));
    if let Err(e) = wrote {
        let _ = std::fs::remove_file(paths.account_file(&account.name));
        return Err(e);
    }
    Ok(account.clone())
}

/// On macOS only one agy account may be enabled (SPEC 6.4, ADR-0005): refuse making
/// `name` an enabled agy account while another one is. The platform comes from the host,
/// so tests can run this on Linux.
pub fn check_single_agy(paths: &Paths, host: &HostEnv, name: &str, harness: Harness) -> Result<()> {
    if host.platform != Platform::MacOs || harness != Harness::Agy {
        return Ok(());
    }
    // A broken account file is reported elsewhere; it must not block this check.
    let existing = AccountFile::list(paths).unwrap_or_default();
    match existing
        .iter()
        .find(|a| a.enabled && a.harness == Harness::Agy && a.name != name)
    {
        Some(other) => Err(AccountsError::AgySecondAccount {
            name: name.to_string(),
            existing: other.name.clone(),
        }),
        None => Ok(()),
    }
}

fn ensure_new(paths: &Paths, name: &str) -> Result<()> {
    if paths.account_file(name).exists() {
        return Err(AccountsError::AlreadyExists(name.to_string()));
    }
    Ok(())
}

/// Refuses a name that already exists as a file in a PATH directory other than the
/// wrapper directory and is not one of our wrappers: the new wrapper would shadow
/// that command. Files inside the wrapper directory are left to `write_wrapper`,
/// which backs a foreign file up.
fn ensure_no_shadow(cfg: &Config, host: &HostEnv, name: &str) -> Result<()> {
    let wrapper_dir = wrapper_dir(cfg, host)?;
    for dir in host.path_var.as_deref().unwrap_or_default().split(':') {
        // An empty PATH entry means the current directory; resolution elsewhere in this
        // crate ignores it too, and a wrapper is never run from the cwd.
        if dir.is_empty() || same_path(Path::new(dir), &wrapper_dir) {
            continue;
        }
        let path = Path::new(dir).join(name);
        // A symlink to one of our wrappers counts as ours.
        let ours = is_managed(&path) || path.canonicalize().is_ok_and(|p| is_managed(&p));
        if is_executable(&path) && !ours {
            return Err(AccountsError::ShadowsCommand {
                name: name.to_string(),
                path,
            });
        }
    }
    Ok(())
}

/// Creates the credential home (agy: with `.gemini/`, mode 0700), the account
/// file and the wrapper. The home defaults to `~/.<name>`.
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
    ensure_no_shadow(cfg, host, name)?;
    check_single_agy(paths, host, name, harness)?;
    let dir = host.expand(&home_spec)?;
    ensure_disjoint_home(paths, host, name, &dir)?;
    let credential_dir = if harness == Harness::Agy {
        dir.join(".gemini")
    } else {
        dir
    };
    // Existing user-selected homes retain their permissions, including symlink targets.
    so_core::fsutil::create_private_dirs(&credential_dir)
        .map_err(|e| AccountsError::io(&credential_dir, e))?;
    finish(paths, cfg, host, &account)
}

/// Highest `n` tried by [`add_numbered`] before giving up.
const MAX_NUMBERED: u32 = 999;

/// Result of [`add_numbered`]: the accounts created, and the error that stopped the
/// batch early, if any. Accounts created before the error stay created.
#[derive(Debug)]
pub struct AddBatch {
    pub created: Vec<AccountFile>,
    pub error: Option<AccountsError>,
}

/// Creates `count` accounts named `<harness>-<n>`, each with the lowest `n` (from 1) whose
/// name is free: a name is skipped when an account file exists or when the shadow check
/// refuses it. Any other failure stops the batch.
pub fn add_numbered(
    paths: &Paths,
    cfg: &Config,
    host: &HostEnv,
    harness: Harness,
    count: u32,
) -> AddBatch {
    let mut created = Vec::new();
    let mut n = 1u32;
    while (created.len() as u32) < count {
        if n > MAX_NUMBERED {
            let error = Some(AccountsError::InvalidName(format!(
                "no free {harness}-<n> name up to {MAX_NUMBERED}"
            )));
            return AddBatch { created, error };
        }
        let name = format!("{harness}-{n}");
        n += 1;
        match add(paths, cfg, host, harness, &name, None) {
            Ok(a) => created.push(a),
            Err(AccountsError::AlreadyExists(_) | AccountsError::ShadowsCommand { .. }) => {}
            Err(e) => {
                return AddBatch {
                    created,
                    error: Some(e),
                };
            }
        }
    }
    AddBatch {
        created,
        error: None,
    }
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
    ensure_no_shadow(cfg, host, name)?;
    check_single_agy(paths, host, name, harness)?;
    let dir = host.expand(home)?;
    ensure_disjoint_home(paths, host, name, &dir)?;
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

/// Homes of the harness defaults; never purged, nor anything inside or above.
fn default_homes(host: &HostEnv) -> Vec<PathBuf> {
    [".codex", ".claude", ".gemini", ".local/share/opencode"]
        .iter()
        .map(|r| host.home.join(r))
        .collect()
}

fn related(a: &Path, b: &Path) -> bool {
    a.starts_with(b) || b.starts_with(a)
}

/// Shared overlap check; purge alone exempts existing accounts on harness defaults.
fn overlapping_home(
    paths: &Paths,
    host: &HostEnv,
    name: &str,
    home: &Path,
    ignored: &[PathBuf],
) -> Result<Option<String>> {
    for other in AccountFile::list(paths)? {
        if other.name == name {
            continue;
        }
        let oh = host.expand(&other.home)?;
        let oc = canon(&oh).map_err(|e| AccountsError::io(&oh, e))?;
        if !ignored.contains(&oc) && related(home, &oc) {
            return Ok(Some(other.name));
        }
    }
    Ok(None)
}

fn ensure_disjoint_home(paths: &Paths, host: &HostEnv, name: &str, home: &Path) -> Result<()> {
    let canonical = canon(home).map_err(|e| AccountsError::io(home, e))?;
    if let Some(existing) = overlapping_home(paths, host, name, &canonical, &[])? {
        return Err(AccountsError::OverlappingHome {
            path: home.to_path_buf(),
            existing,
        });
    }
    Ok(())
}

/// Fail-closed purge checks. Returns the canonical home to delete, or `None`
/// when the home does not exist.
fn check_purge(
    paths: &Paths,
    cfg: &Config,
    host: &HostEnv,
    account: &AccountFile,
    home: &Path,
) -> Result<Option<PathBuf>> {
    let refuse = |reason: String| AccountsError::UnsafePurge {
        path: home.to_path_buf(),
        reason,
    };
    let manual = " delete it manually if you really want it gone";
    let Ok(meta) = std::fs::symlink_metadata(home) else {
        return Ok(None); // nothing there to purge
    };
    if meta.is_symlink() {
        let target = std::fs::read_link(home)
            .map(|t| t.display().to_string())
            .unwrap_or_else(|_| "?".into());
        return Err(refuse(format!(
            "the home is a symlink to {target}; remove the link and its target manually"
        )));
    }
    let io = |what: &str, e: std::io::Error| refuse(format!("cannot resolve {what}: {e}"));
    let chome = canon(home).map_err(|e| io("the home", e))?;
    let chost = canon(&host.home).map_err(|e| io("$HOME", e))?;
    let rel = chome
        .strip_prefix(&chost)
        .map_err(|_| refuse("the home is outside $HOME".into()))?
        .to_path_buf();
    let names: Vec<_> = rel.components().map(|c| c.as_os_str().to_owned()).collect();
    let direct = names.len() == 1;
    let profile = names.len() == 3 && names[0] == ".agent-farm" && names[1] == "profiles";
    if !(direct || profile) {
        return Err(refuse(format!(
            "it is not a direct child of $HOME or an agent-farm profile;{manual}"
        )));
    }
    // Protected paths the home must not equal or contain.
    let deny: Vec<(String, PathBuf)> = vec![
        ("$HOME".into(), host.home.clone()),
        ("~/.ssh".into(), host.home.join(".ssh")),
        ("~/.config".into(), host.home.join(".config")),
        ("~/.local".into(), host.home.join(".local")),
        ("~/.local/share".into(), host.home.join(".local/share")),
        ("the config directory".into(), paths.config_dir.clone()),
        ("the wrappers backup".into(), paths.wrappers_bak_dir()),
        ("the wrapper directory".into(), wrapper_dir(cfg, host)?),
    ];
    for (what, p) in deny {
        let cp = canon(&p).map_err(|e| io(what.as_str(), e))?;
        if cp.starts_with(&chome) {
            return Err(refuse(format!("it is or contains {what}")));
        }
    }
    // Harness default homes are also protected from anything inside them.
    for p in default_homes(host) {
        let cp = canon(&p).map_err(|e| io("a harness default home", e))?;
        if related(&chome, &cp) {
            return Err(refuse(
                "it is, contains or lies inside a harness default home".into(),
            ));
        }
    }
    // Preserve the purge exception for legacy accounts on shared default homes.
    let mut shared = vec![chost];
    for p in default_homes(host) {
        shared.push(canon(&p).map_err(|e| io("a harness default home", e))?);
    }
    if let Some(other) = overlapping_home(paths, host, &account.name, &chome, &shared)? {
        return Err(refuse(format!("account `{other}` has an overlapping home")));
    }
    Ok(Some(chome))
}

/// Removes the account's credential home (with `purge`, after safety checks),
/// then its managed wrapper, then the account file last. Safety checks run
/// first, so a refusal changes nothing.
pub fn remove(paths: &Paths, cfg: &Config, host: &HostEnv, name: &str, purge: bool) -> Result<()> {
    let file = paths.account_file(name);
    if !file.exists() {
        return Err(AccountsError::NotFound(name.to_string()));
    }
    let account = AccountFile::load(&file)?;
    let home = host.expand(&account.home)?;
    let victim = if purge {
        check_purge(paths, cfg, host, &account, &home)?
    } else {
        None
    };
    if let Some(v) = victim {
        std::fs::remove_dir_all(&v).map_err(|e| AccountsError::io(&v, e))?;
    }
    remove_wrapper(&wrapper_dir(cfg, host)?, name)?;
    std::fs::remove_file(&file).map_err(|e| AccountsError::io(&file, e))?;
    Ok(())
}

pub fn set_enabled(
    paths: &Paths,
    host: &HostEnv,
    name: &str,
    enabled: bool,
) -> Result<AccountFile> {
    let file = paths.account_file(name);
    if !file.exists() {
        return Err(AccountsError::NotFound(name.to_string()));
    }
    let mut account = AccountFile::load(&file)?;
    if enabled {
        check_single_agy(paths, host, name, account.harness)?;
    }
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

    fn mac_fx() -> Fx {
        let mut f = fx();
        f.host = host(f.d.path(), Platform::MacOs);
        f
    }

    #[test]
    fn add_preserves_existing_credential_home_permissions() {
        for home in ["~", "~/credentials", "~/linked"] {
            let f = fx();
            let target = if home == "~/linked" {
                let target = f.d.path().join("credentials");
                std::fs::create_dir_all(&target).unwrap();
                std::os::unix::fs::symlink(&target, f.d.path().join("linked")).unwrap();
                target
            } else {
                f.host.expand(home).unwrap()
            };
            std::fs::create_dir_all(&target).unwrap();
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).unwrap();
            add(&f.paths, &f.cfg, &f.host, Harness::Codex, "new", Some(home)).unwrap();
            assert_eq!(
                std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
                0o755
            );
        }
    }

    #[test]
    fn account_creation_tightens_existing_config_directories() {
        use std::os::unix::fs::PermissionsExt;
        for importing in [false, true] {
            let f = fx();
            std::fs::create_dir_all(f.paths.accounts_dir()).unwrap();
            for dir in [&f.paths.config_dir, &f.paths.accounts_dir()] {
                std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            if importing {
                std::fs::create_dir_all(f.d.path().join("credentials")).unwrap();
                import(
                    &f.paths,
                    &f.cfg,
                    &f.host,
                    "new",
                    Harness::Codex,
                    "~/credentials",
                )
                .unwrap();
            } else {
                add(&f.paths, &f.cfg, &f.host, Harness::Codex, "new", None).unwrap();
            }
            for dir in [&f.paths.config_dir, &f.paths.accounts_dir()] {
                assert_eq!(
                    std::fs::metadata(dir).unwrap().permissions().mode() & 0o777,
                    0o700,
                    "{}",
                    dir.display()
                );
            }
        }
    }

    #[test]
    fn add_and_import_refuse_overlapping_homes() {
        for importing in [false, true] {
            for candidate in [
                "~/shared",
                "~/shared/inner",
                "~",
                "~/alias",
                "~/alias/new",
                "~/other/../shared/new",
            ] {
                let f = fx();
                std::fs::create_dir_all(f.d.path().join("shared/inner")).unwrap();
                std::fs::create_dir_all(f.d.path().join("other")).unwrap();
                std::os::unix::fs::symlink(f.d.path().join("shared"), f.d.path().join("alias"))
                    .unwrap();
                AccountFile::new("existing", Harness::Codex, "~/shared", "codex")
                    .unwrap()
                    .save(&f.paths)
                    .unwrap();
                if importing {
                    std::fs::create_dir_all(f.host.expand(candidate).unwrap()).unwrap();
                }
                let result = if importing {
                    import(&f.paths, &f.cfg, &f.host, "new", Harness::Codex, candidate)
                } else {
                    add(
                        &f.paths,
                        &f.cfg,
                        &f.host,
                        Harness::Codex,
                        "new",
                        Some(candidate),
                    )
                };
                let err = result.expect_err(candidate);
                assert!(err.to_string().contains("overlapping home"), "{err}");
                assert!(!f.paths.account_file("new").exists());
                assert!(!f.d.path().join(".local/bin/new").exists());
                if !importing {
                    assert!(!f.d.path().join("shared/new").exists());
                }
            }
        }
    }

    #[test]
    fn add_numbered_takes_the_lowest_free_numbers() {
        let f = fx();
        add(&f.paths, &f.cfg, &f.host, Harness::Codex, "codex-2", None).unwrap();
        let b = add_numbered(&f.paths, &f.cfg, &f.host, Harness::Codex, 2);
        assert!(b.error.is_none());
        let names: Vec<_> = b.created.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["codex-1", "codex-3"]);
        assert!(f.paths.account_file("codex-3").is_file());
    }

    #[test]
    fn add_numbered_skips_names_that_shadow_a_command() {
        let f = fx();
        crate::testutil::stub_bin(f.d.path(), "codex-1");
        let b = add_numbered(&f.paths, &f.cfg, &f.host, Harness::Codex, 1);
        assert!(b.error.is_none());
        assert_eq!(b.created[0].name, "codex-2");
    }

    #[test]
    fn add_numbered_reports_the_error_that_stops_the_batch() {
        let f = mac_fx();
        let b = add_numbered(&f.paths, &f.cfg, &f.host, Harness::Agy, 2);
        assert_eq!(b.created.len(), 1);
        assert!(matches!(
            b.error,
            Some(AccountsError::AgySecondAccount { .. })
        ));
    }

    #[test]
    fn add_and_import_refuse_a_name_that_shadows_a_command() {
        let f = fx();
        let real = crate::testutil::stub_bin(f.d.path(), "ls");
        let err = add(&f.paths, &f.cfg, &f.host, Harness::Codex, "ls", None).unwrap_err();
        let msg = err.to_string();
        assert!(matches!(err, AccountsError::ShadowsCommand { .. }), "{msg}");
        assert!(msg.contains(&real.display().to_string()), "{msg}");
        assert!(msg.contains("pick another name"), "{msg}");
        assert!(!f.paths.account_file("ls").exists());
        std::fs::create_dir_all(f.d.path().join("h")).unwrap();
        let err = import(&f.paths, &f.cfg, &f.host, "ls", Harness::Codex, "~/h").unwrap_err();
        assert!(matches!(err, AccountsError::ShadowsCommand { .. }));
    }

    #[test]
    fn a_foreign_file_in_the_wrapper_dir_is_backed_up_not_refused() {
        let mut f = fx();
        f.cfg.accounts.wrapper_dir = "~/bin".into();
        let old = crate::testutil::stub_bin(f.d.path(), "work");
        add(&f.paths, &f.cfg, &f.host, Harness::Codex, "work", None).unwrap();
        assert!(is_managed(&old));
        let baks = std::fs::read_dir(f.paths.wrappers_bak_dir())
            .unwrap()
            .count();
        assert_eq!(baks, 1);
    }

    #[test]
    fn a_same_named_file_in_another_path_dir_is_refused() {
        let mut f = fx();
        f.cfg.accounts.wrapper_dir = "~/wrappers".into();
        crate::testutil::stub_bin(f.d.path(), "work");
        let err = add(&f.paths, &f.cfg, &f.host, Harness::Codex, "work", None).unwrap_err();
        assert!(matches!(err, AccountsError::ShadowsCommand { .. }));
    }

    #[test]
    fn a_non_executable_file_does_not_shadow() {
        use std::os::unix::fs::PermissionsExt;
        let mut f = fx();
        f.cfg.accounts.wrapper_dir = "~/wrappers".into();
        let p = crate::testutil::stub_bin(f.d.path(), "work");
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        add(&f.paths, &f.cfg, &f.host, Harness::Codex, "work", None).unwrap();
    }

    #[test]
    fn a_symlink_to_our_wrapper_in_another_path_dir_is_not_a_shadow() {
        let mut f = fx();
        f.cfg.accounts.wrapper_dir = "~/wrappers".into();
        add(&f.paths, &f.cfg, &f.host, Harness::Codex, "work", None).unwrap();
        std::fs::remove_file(f.paths.account_file("work")).unwrap();
        let bin = f.d.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::os::unix::fs::symlink(f.d.path().join("wrappers/work"), bin.join("work")).unwrap();
        add(&f.paths, &f.cfg, &f.host, Harness::Codex, "work", None).unwrap();
    }

    #[test]
    fn our_own_wrapper_on_path_is_not_a_shadow() {
        let mut f = fx();
        f.cfg.accounts.wrapper_dir = "~/bin".into();
        add(&f.paths, &f.cfg, &f.host, Harness::Codex, "work", None).unwrap();
        assert!(is_managed(&f.d.path().join("bin/work")));
        // Re-adding after removing only the toml must not trip the check.
        std::fs::remove_file(f.paths.account_file("work")).unwrap();
        add(&f.paths, &f.cfg, &f.host, Harness::Codex, "work", None).unwrap();
    }

    #[test]
    fn macos_refuses_a_second_enabled_agy_account() {
        let f = mac_fx();
        add(&f.paths, &f.cfg, &f.host, Harness::Agy, "agy-acc1", None).unwrap();
        // add
        let e = add(&f.paths, &f.cfg, &f.host, Harness::Agy, "agy-acc2", None).unwrap_err();
        assert!(matches!(e, AccountsError::AgySecondAccount { .. }), "{e}");
        assert!(!f.paths.account_file("agy-acc2").exists());
        assert!(
            !f.d.path().join(".agy-acc2").exists(),
            "no home left behind"
        );
        // import
        std::fs::create_dir_all(f.d.path().join("p/.gemini")).unwrap();
        let e = import(&f.paths, &f.cfg, &f.host, "agy-acc3", Harness::Agy, "~/p").unwrap_err();
        assert!(matches!(e, AccountsError::AgySecondAccount { .. }), "{e}");
        // other harnesses are unaffected
        add(
            &f.paths,
            &f.cfg,
            &f.host,
            Harness::Codex,
            "codex-acc1",
            None,
        )
        .unwrap();
        // enable: a second account that exists but is disabled cannot be turned on
        let linux = host(f.d.path(), Platform::Linux);
        add(&f.paths, &f.cfg, &linux, Harness::Agy, "agy-acc4", None).unwrap();
        set_enabled(&f.paths, &linux, "agy-acc4", false).unwrap();
        let e = set_enabled(&f.paths, &f.host, "agy-acc4", true).unwrap_err();
        assert!(matches!(e, AccountsError::AgySecondAccount { .. }), "{e}");
        // disabling the first frees the slot; disabling is always allowed
        set_enabled(&f.paths, &f.host, "agy-acc1", false).unwrap();
        set_enabled(&f.paths, &f.host, "agy-acc4", true).unwrap();
        // the same account may be re-enabled while it is the only one
        set_enabled(&f.paths, &f.host, "agy-acc4", true).unwrap();
    }

    #[test]
    fn linux_allows_several_agy_accounts() {
        let f = fx();
        add(&f.paths, &f.cfg, &f.host, Harness::Agy, "agy-acc1", None).unwrap();
        add(&f.paths, &f.cfg, &f.host, Harness::Agy, "agy-acc2", None).unwrap();
        set_enabled(&f.paths, &f.host, "agy-acc2", true).unwrap();
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
    fn homes_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let f = fx();
        add(&f.paths, &f.cfg, &f.host, Harness::Agy, "agy-acc7", None).unwrap();
        for p in [".agy-acc7", ".agy-acc7/.gemini"] {
            let m = std::fs::metadata(f.d.path().join(p)).unwrap();
            assert_eq!(m.permissions().mode() & 0o777, 0o700, "{p}");
        }
    }

    #[test]
    fn create_new_semantics_and_wrapper_failure_rolls_back() {
        let f = fx();
        let acc = AccountFile::new("codex-r", Harness::Codex, "~/.r", "codex").unwrap();
        create_account_file(&f.paths, &acc).unwrap();
        assert!(matches!(
            create_account_file(&f.paths, &acc),
            Err(AccountsError::AlreadyExists(_))
        ));
        // Wrapper dir is a regular file: the wrapper cannot be written, and
        // the toml written just before must be gone again.
        std::fs::create_dir_all(f.d.path().join(".local")).unwrap();
        std::fs::write(f.d.path().join(".local/bin"), "not a dir").unwrap();
        let e = add(&f.paths, &f.cfg, &f.host, Harness::Codex, "codex-w", None).unwrap_err();
        assert!(matches!(e, AccountsError::Io { .. }), "{e}");
        assert!(!f.paths.account_file("codex-w").exists());
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
        std::fs::write(f.d.path().join(".local/bin/codex-acc8"), "foreign").unwrap();
        remove(&f.paths, &f.cfg, &f.host, "codex-acc8", true).unwrap();
        assert!(!f.d.path().join(".codex-acc8").exists());
        assert!(f.d.path().join(".local/bin/codex-acc8").exists());
        assert!(matches!(
            remove(&f.paths, &f.cfg, &f.host, "codex-acc8", false),
            Err(AccountsError::NotFound(_))
        ));
    }

    fn refuses_purge(f: &Fx, name: &str, h: Harness, home: &str) -> String {
        AccountFile::new(name, h, home, h.as_str())
            .unwrap()
            .save(&f.paths)
            .unwrap();
        let e = remove(&f.paths, &f.cfg, &f.host, name, true).unwrap_err();
        assert!(
            matches!(e, AccountsError::UnsafePurge { .. }),
            "{name}: {e}"
        );
        assert!(f.paths.account_file(name).exists(), "{name} toml kept");
        e.to_string()
    }

    #[test]
    fn purge_refuses_default_homes_and_relatives() {
        let f = fx();
        let r = f.d.path();
        for d in [
            ".codex",
            ".claude",
            ".gemini",
            ".local/share/opencode",
            ".local/bin",
            ".ssh",
            ".config",
        ] {
            std::fs::create_dir_all(r.join(d)).unwrap();
        }
        std::fs::create_dir_all(r.join(".codex/sessions")).unwrap();
        let cases = [
            ("codex-a", Harness::Codex, "~/.codex"),
            ("claude-a", Harness::Claude, "~/.claude"),
            ("agy-a", Harness::Agy, "~/.gemini"),
            ("oc-a", Harness::Opencode, "~/.local/share/opencode"),
            ("oc-b", Harness::Opencode, "~/.local/share"), // contains default
            ("oc-c", Harness::Opencode, "~/.local"),
            ("codex-b", Harness::Codex, "~/.codex/sessions"), // descendant
            ("codex-c", Harness::Codex, "~"),
            ("codex-d", Harness::Codex, "~/.ssh"),
            ("codex-e", Harness::Codex, "~/.config"),
            ("codex-f", Harness::Codex, "~/.local/bin"), // wrapper dir
        ];
        for (name, h, home) in cases {
            refuses_purge(&f, name, h, home);
        }
        assert!(r.join(".codex/sessions").is_dir() && r.join(".local/share/opencode").is_dir());
    }

    #[test]
    fn purge_refuses_outside_home_deep_and_config_dirs() {
        let f = fx();
        let r = f.d.path();
        let outside = tempfile::tempdir().unwrap();
        let outside_s = outside.path().to_str().unwrap().to_string();
        refuses_purge(&f, "codex-a", Harness::Codex, &outside_s);
        assert!(outside.path().is_dir());
        // Deeper than a direct child, not an agent-farm profile.
        std::fs::create_dir_all(r.join("work/deep")).unwrap();
        let m = refuses_purge(&f, "codex-b", Harness::Codex, "~/work/deep");
        assert!(m.contains("manually"), "{m}");
        // Config dir and wrappers.bak live inside the temp HOME's cfg dir.
        std::fs::create_dir_all(f.paths.wrappers_bak_dir()).unwrap();
        let cfg_spec = format!(
            "~/{}",
            f.paths.config_dir.strip_prefix(r).unwrap().display()
        );
        refuses_purge(&f, "codex-c", Harness::Codex, &cfg_spec);
        assert!(f.paths.wrappers_bak_dir().is_dir());
    }

    #[test]
    fn purge_refuses_symlinked_home_naming_target() {
        let f = fx();
        let r = f.d.path();
        std::fs::create_dir_all(r.join("real-home")).unwrap();
        std::os::unix::fs::symlink(r.join("real-home"), r.join(".codex-link")).unwrap();
        let m = refuses_purge(&f, "codex-l", Harness::Codex, "~/.codex-link");
        assert!(m.contains("symlink") && m.contains("real-home"), "{m}");
        assert!(r.join("real-home").is_dir());
        assert!(r.join(".codex-link").symlink_metadata().is_ok());
    }

    #[test]
    fn purge_refuses_overlapping_account_homes_and_allows_agy_profile() {
        let f = fx();
        let r = f.d.path();
        std::fs::create_dir_all(r.join("shared/inner")).unwrap();
        AccountFile::new("codex-in", Harness::Codex, "~/shared/inner", "codex")
            .unwrap()
            .save(&f.paths)
            .unwrap();
        refuses_purge(&f, "codex-out", Harness::Codex, "~/shared"); // other nested inside
        AccountFile::new("codex-twin", Harness::Codex, "~/shared", "codex")
            .unwrap()
            .save(&f.paths)
            .unwrap();
        assert!(remove(&f.paths, &f.cfg, &f.host, "codex-twin", true).is_err());
        assert!(r.join("shared/inner").is_dir());

        std::fs::create_dir_all(r.join(".agent-farm/profiles/agy_worker_1/.gemini")).unwrap();
        AccountFile::new(
            "agy-acc1",
            Harness::Agy,
            "~/.agent-farm/profiles/agy_worker_1",
            "agy",
        )
        .unwrap()
        .save(&f.paths)
        .unwrap();
        remove(&f.paths, &f.cfg, &f.host, "agy-acc1", true).unwrap();
        assert!(!r.join(".agent-farm/profiles/agy_worker_1").exists());
        assert!(r.join(".agent-farm/profiles").is_dir());
    }

    #[test]
    fn purge_allowed_alongside_home_dir_agy_account() {
        let f = fx();
        let r = f.d.path();
        std::fs::create_dir_all(r.join(".gemini")).unwrap();
        AccountFile::new("agy-acc1", Harness::Agy, "~", "agy")
            .unwrap()
            .save(&f.paths)
            .unwrap();
        AccountFile::new("codex-acc3", Harness::Codex, "~/.codex-acc3", "codex")
            .unwrap()
            .save(&f.paths)
            .unwrap();
        std::fs::create_dir_all(r.join(".codex-acc3")).unwrap();
        remove(&f.paths, &f.cfg, &f.host, "codex-acc3", true).unwrap();
        assert!(!r.join(".codex-acc3").exists());
        assert!(f.paths.account_file("agy-acc1").exists());
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
        assert!(
            !set_enabled(&f.paths, &f.host, "codex-acc7", false)
                .unwrap()
                .enabled
        );
        assert!(
            !AccountFile::load(&f.paths.account_file("codex-acc7"))
                .unwrap()
                .enabled
        );
        assert!(set_enabled(&f.paths, &f.host, "nope-x", true).is_err());
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

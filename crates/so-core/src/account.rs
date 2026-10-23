//! Account definition files (SPEC 5.1).

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::paths::Paths;
use crate::types::Harness;

/// Names an account must not take: they would shadow a real binary.
const RESERVED: [&str; 5] = ["codex", "agy", "opencode", "claude", "superorchestrator"];

/// Checks `^[a-z][a-z0-9-]{1,31}$` and that the name is not reserved.
pub fn validate_account_name(name: &str) -> Result<()> {
    let b = name.as_bytes();
    let shape_ok = (2..=32).contains(&b.len())
        && b[0].is_ascii_lowercase()
        && b[1..]
            .iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-');
    if shape_ok && !RESERVED.contains(&name) {
        Ok(())
    } else {
        Err(CoreError::InvalidAccountName(name.to_string()))
    }
}

fn default_cap() -> u32 {
    2
}

fn default_true() -> bool {
    true
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAccount {
    name: String,
    harness: Harness,
    home: String,
    binary: String,
    #[serde(default)]
    priority: i32,
    #[serde(default = "default_cap")]
    cap: u32,
    #[serde(default = "default_true")]
    enabled: bool,
    #[serde(default)]
    pools: Option<Vec<String>>,
}

impl From<RawAccount> for AccountFile {
    fn from(r: RawAccount) -> Self {
        let pools = r.pools.unwrap_or_else(|| default_pools(r.harness));
        AccountFile {
            name: r.name,
            harness: r.harness,
            home: r.home,
            binary: r.binary,
            priority: r.priority,
            cap: r.cap,
            enabled: r.enabled,
            pools,
        }
    }
}

fn default_pools(h: Harness) -> Vec<String> {
    if h == Harness::Agy {
        vec!["gemini".to_string(), "claude".to_string()]
    } else {
        vec!["default".to_string()]
    }
}

/// One account: a named login for one harness, backed by a credential home.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(from = "RawAccount")]
pub struct AccountFile {
    pub name: String,
    pub harness: Harness,
    /// Credential home, `~` allowed.
    pub home: String,
    /// Binary resolved on `PATH` at launch; an absolute path is allowed.
    pub binary: String,
    pub priority: i32,
    pub cap: u32,
    pub enabled: bool,
    pub pools: Vec<String>,
}

impl AccountFile {
    /// New account with spec defaults for the harness.
    pub fn new(name: &str, harness: Harness, home: &str, binary: &str) -> Result<Self> {
        validate_account_name(name)?;
        Ok(Self {
            name: name.to_string(),
            harness,
            home: home.to_string(),
            binary: binary.to_string(),
            priority: 0,
            cap: default_cap(),
            enabled: true,
            pools: default_pools(harness),
        })
    }

    /// Checks name, `cap >= 1` and non-empty `pools`.
    pub fn validate(&self) -> Result<()> {
        validate_account_name(&self.name)?;
        let invalid = |reason: &str| CoreError::InvalidValue {
            key: format!("account {}", self.name),
            reason: reason.to_string(),
        };
        if self.cap == 0 {
            return Err(invalid("cap must be at least 1"));
        }
        if self.pools.is_empty() {
            return Err(invalid("pools must not be empty"));
        }
        Ok(())
    }

    /// Loads one account file. The `name` must equal the file stem.
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).map_err(|e| CoreError::io(path, e))?;
        let acc: Self = toml::from_str(&text).map_err(|source| CoreError::TomlDe {
            path: path.to_path_buf(),
            source,
        })?;
        let bad = |reason: String| CoreError::InvalidAccountFile {
            path: path.to_path_buf(),
            reason,
        };
        acc.validate().map_err(|e| bad(e.to_string()))?;
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        if stem != acc.name {
            return Err(bad(format!(
                "name `{}` does not match the file name",
                acc.name
            )));
        }
        Ok(acc)
    }

    /// Writes `accounts/<name>.toml`, creating the directory.
    pub fn save(&self, paths: &Paths) -> Result<()> {
        self.validate()?;
        let path = paths.account_file(&self.name);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| CoreError::io(dir, e))?;
        }
        let text = toml::to_string_pretty(self)?;
        crate::fsutil::write_atomic(&path, text.as_bytes())
    }

    /// All account files, sorted by name. A missing directory yields an empty list.
    pub fn list(paths: &Paths) -> Result<Vec<Self>> {
        let dir = paths.accounts_dir();
        let rd = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(CoreError::io(&dir, e)),
        };
        let mut out = Vec::new();
        for entry in rd {
            let path = entry.map_err(|e| CoreError::io(&dir, e))?.path();
            if path.extension().is_some_and(|x| x == "toml") {
                out.push(Self::load(&path)?);
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_validation() {
        for ok in ["codex-acc2", "ab", "a1", "agy-work", &"a".repeat(32)] {
            assert!(validate_account_name(ok).is_ok(), "{ok}");
        }
        for bad in [
            "a",
            "",
            "Codex-acc",
            "1abc",
            "-ab",
            "ab_c",
            "ab c",
            &"a".repeat(33),
        ] {
            assert!(validate_account_name(bad).is_err(), "{bad}");
        }
        for reserved in ["codex", "agy", "opencode", "claude", "superorchestrator"] {
            assert!(validate_account_name(reserved).is_err(), "{reserved}");
        }
    }

    #[test]
    fn superorchestrator_name_is_reserved() {
        assert!(validate_account_name("superorchestrator").is_err());
        assert!(AccountFile::new("superorchestrator", Harness::Codex, "h", "codex").is_err());
        // Longer names that merely contain it are still fine.
        assert!(validate_account_name("superorchestrator-acc1").is_ok());
    }

    #[test]
    fn defaults_when_fields_omitted() {
        let a: AccountFile = toml::from_str(
            "name = \"codex-acc2\"\nharness = \"codex\"\nhome = \"~/.codex-acc2\"\nbinary = \"codex\"",
        )
        .unwrap();
        assert_eq!(a.priority, 0);
        assert_eq!(a.cap, 2);
        assert!(a.enabled);
        assert_eq!(a.pools, vec!["default"]);
    }

    #[test]
    fn agy_default_pools() {
        let a: AccountFile = toml::from_str(
            "name = \"agy-acc1\"\nharness = \"agy\"\nhome = \"~/.agy1\"\nbinary = \"agy\"",
        )
        .unwrap();
        assert_eq!(a.pools, vec!["gemini", "claude"]);
        let b: AccountFile = toml::from_str(
            "name = \"agy-acc1\"\nharness = \"agy\"\nhome = \"h\"\nbinary = \"agy\"\npools = [\"gemini\"]",
        )
        .unwrap();
        assert_eq!(b.pools, vec!["gemini"]);
    }

    #[test]
    fn rejects_zero_cap_empty_pools_and_name_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::from_dir(dir.path().to_path_buf());
        let mut a = AccountFile::new("codex-a", Harness::Codex, "~/.a", "codex").unwrap();
        a.cap = 0;
        assert!(a.save(&paths).is_err());
        a.cap = 1;
        a.pools.clear();
        assert!(a.save(&paths).is_err());
        a.pools = vec!["default".into()];
        a.save(&paths).unwrap();

        // File renamed so the stem no longer matches the name.
        std::fs::rename(paths.account_file("codex-a"), paths.account_file("codex-z")).unwrap();
        let err = AccountFile::load(&paths.account_file("codex-z")).unwrap_err();
        assert!(matches!(err, CoreError::InvalidAccountFile { .. }), "{err}");
        assert!(err.to_string().contains("codex-z.toml"));
        // list names the offending file.
        let err = AccountFile::list(&paths).unwrap_err();
        assert!(err.to_string().contains("codex-z.toml"), "{err}");

        std::fs::write(
            paths.account_file("codex-z"),
            "name = \"codex-z\"\nharness = \"codex\"\nhome = \"h\"\nbinary = \"codex\"\ncap = 0",
        )
        .unwrap();
        assert!(AccountFile::load(&paths.account_file("codex-z")).is_err());
    }

    #[test]
    fn save_load_list() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::from_dir(dir.path().to_path_buf());
        assert!(AccountFile::list(&paths).unwrap().is_empty());
        let mut b = AccountFile::new("codex-b", Harness::Codex, "~/.b", "codex").unwrap();
        b.priority = 3;
        let a = AccountFile::new("agy-a", Harness::Agy, "~/.a", "agy").unwrap();
        b.save(&paths).unwrap();
        a.save(&paths).unwrap();
        let listed = AccountFile::list(&paths).unwrap();
        assert_eq!(listed, vec![a, b.clone()]);
        assert_eq!(
            AccountFile::load(&paths.account_file("codex-b")).unwrap(),
            b
        );
        assert!(AccountFile::new("codex", Harness::Codex, "h", "codex").is_err());
    }
}

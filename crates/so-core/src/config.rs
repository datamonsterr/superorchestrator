//! `config.toml` (SPEC 16): defaults, load, save and dotted-key get/set.

use serde::{Deserialize, Serialize};

use crate::duration::HumanDuration;
use crate::error::{CoreError, Result};
use crate::paths::Paths;
use crate::types::{Choice, Harness};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Limits {
    pub global_cap: u32,
    pub max_children: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            global_cap: 16,
            max_children: 8,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Runs {
    pub stall_ms: u64,
    pub ask_timeout: HumanDuration,
}

impl Default for Runs {
    fn default() -> Self {
        Self {
            stall_ms: 300_000,
            ask_timeout: HumanDuration::from_secs(30 * 60),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Usage {
    pub probe_ttl: HumanDuration,
    pub low_pct: u32,
    pub default_cooldown: HumanDuration,
}

impl Default for Usage {
    fn default() -> Self {
        Self {
            probe_ttl: HumanDuration::from_secs(5 * 60),
            low_pct: 15,
            default_cooldown: HumanDuration::from_secs(5 * 3600),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Weights {
    pub headroom: f64,
    pub load: f64,
    pub cost: f64,
}

impl Default for Weights {
    fn default() -> Self {
        Self {
            headroom: 1.0,
            load: 0.5,
            cost: 0.3,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Jev {
    pub endpoint: String,
    pub model: String,
    pub key_env: String,
    /// Optional dotenv file holding the key.
    pub key_file: Option<String>,
    pub timeout: HumanDuration,
}

impl Default for Jev {
    fn default() -> Self {
        Self {
            endpoint: "https://openrouter.ai/api/alpha/decisions".to_string(),
            model: "~typesafe/jev-latest".to_string(),
            key_env: "OPENROUTER_API_KEY".to_string(),
            key_file: None,
            timeout: HumanDuration::from_secs(20),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Policy {
    pub min_confidence: f64,
    pub min_margin: f64,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            min_confidence: 0.6,
            min_margin: 0.1,
        }
    }
}

/// Default routing instruction handed to the decider.
pub const DEFAULT_ROUTER_INSTRUCTION: &str = "Choose the least costly eligible choice that can reliably complete task_content. \
Prefer free and included capacity, pick a stronger choice only when the task needs it, \
and never answer with a choice outside the eligible set.";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Router {
    pub priority: Vec<Harness>,
    pub weights: Weights,
    pub instruction: String,
    pub jev: Jev,
    pub policy: Policy,
}

impl Default for Router {
    fn default() -> Self {
        Self {
            priority: vec![
                Harness::Agy,
                Harness::Opencode,
                Harness::Codex,
                Harness::Claude,
            ],
            weights: Weights::default(),
            instruction: DEFAULT_ROUTER_INSTRUCTION.to_string(),
            jev: Jev::default(),
            policy: Policy::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Worktrees {
    pub dir: String,
    pub branch_prefix: String,
}

impl Default for Worktrees {
    fn default() -> Self {
        Self {
            dir: "{repo_parent}/{repo}-worktrees".to_string(),
            branch_prefix: "so/".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AccountsCfg {
    pub wrapper_dir: String,
}

impl Default for AccountsCfg {
    fn default() -> Self {
        Self {
            wrapper_dir: "~/.local/bin".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Ui {
    pub bind: String,
    pub port: u16,
    pub token: Option<String>,
}

impl Default for Ui {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1".to_string(),
            port: 7878,
            token: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Supervisor {
    pub idle_exit: HumanDuration,
}

impl Default for Supervisor {
    fn default() -> Self {
        Self {
            idle_exit: HumanDuration::from_secs(10 * 60),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Notifications {
    pub discord_webhook_env: String,
    pub desktop: bool,
    pub on: Vec<String>,
}

impl Default for Notifications {
    fn default() -> Self {
        Self {
            discord_webhook_env: "SO_DISCORD_WEBHOOK".to_string(),
            desktop: true,
            on: [
                "escalated",
                "question_waiting_10m",
                "session_finished",
                "account_exhausted",
            ]
            .map(String::from)
            .to_vec(),
        }
    }
}

/// Default harness, model and effort used when a spawn does not name them.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Defaults {
    pub harness: Option<Harness>,
    pub model: Option<String>,
    pub effort: Option<String>,
}

/// The whole of `config.toml`. Every section is optional in the file.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub limits: Limits,
    pub runs: Runs,
    pub usage: Usage,
    pub router: Router,
    pub worktrees: Worktrees,
    pub accounts: AccountsCfg,
    pub ui: Ui,
    pub supervisor: Supervisor,
    pub notifications: Notifications,
    pub defaults: Defaults,
    pub choices: Vec<Choice>,
}

impl Config {
    /// Loads `config.toml`; a missing file yields the defaults.
    pub fn load(paths: &Paths) -> Result<Self> {
        let path = paths.config_toml();
        match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).map_err(|source| CoreError::TomlDe { path, source }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(CoreError::io(path, e)),
        }
    }

    /// Writes `config.toml`, creating the config directory.
    pub fn save(&self, paths: &Paths) -> Result<()> {
        let path = paths.config_toml();
        std::fs::create_dir_all(&paths.config_dir)
            .map_err(|e| CoreError::io(&paths.config_dir, e))?;
        let text = toml::to_string_pretty(self)?;
        std::fs::write(&path, text).map_err(|e| CoreError::io(path, e))
    }

    fn to_value(&self) -> Result<toml::Value> {
        Ok(toml::Value::try_from(self)?)
    }

    /// Value at a dotted key such as `router.weights.load`.
    /// Unset optional keys (for example `ui.token`) are not found.
    pub fn get(&self, key_path: &str) -> Result<toml::Value> {
        let mut cur = self.to_value()?;
        for part in key_path.split('.') {
            cur = match cur {
                toml::Value::Table(mut t) => t.remove(part),
                _ => None,
            }
            .ok_or_else(|| CoreError::InvalidConfigKey(key_path.to_string()))?;
        }
        Ok(cur)
    }

    /// Sets a dotted key from its string form, validating by re-deserialising the whole config.
    /// The string is read as a TOML value (`true`, `8`, `["a"]`, `"x"`), or as a plain string.
    pub fn set(&mut self, key_path: &str, value: &str) -> Result<()> {
        let parts: Vec<&str> = key_path.split('.').collect();
        if parts.iter().any(|p| p.is_empty()) {
            return Err(CoreError::InvalidConfigKey(key_path.to_string()));
        }
        let parsed: Option<toml::Value> = toml::from_str::<toml::Table>(&format!("v = {value}"))
            .ok()
            .and_then(|mut t| t.remove("v"));
        let mut candidates = Vec::new();
        if let Some(v) = parsed {
            candidates.push(v);
        }
        candidates.push(toml::Value::String(value.to_string()));

        let mut last_err = None;
        for cand in candidates {
            match self.with_value(&parts, key_path, cand) {
                Ok(cfg) => {
                    *self = cfg;
                    return Ok(());
                }
                Err(e @ CoreError::InvalidConfigKey(_)) => return Err(e),
                Err(e) => last_err = Some(e),
            }
        }
        Err(last_err.unwrap_or_else(|| CoreError::InvalidConfigKey(key_path.to_string())))
    }

    fn with_value(&self, parts: &[&str], key_path: &str, new: toml::Value) -> Result<Config> {
        let mut root = self.to_value()?;
        let mut cur = &mut root;
        for part in &parts[..parts.len() - 1] {
            let table = cur
                .as_table_mut()
                .ok_or_else(|| CoreError::InvalidConfigKey(key_path.to_string()))?;
            cur = table
                .entry(part.to_string())
                .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        }
        let table = cur
            .as_table_mut()
            .ok_or_else(|| CoreError::InvalidConfigKey(key_path.to_string()))?;
        table.insert(parts[parts.len() - 1].to_string(), new);
        root.try_into::<Config>().map_err(|e| {
            let msg = e.message().to_string();
            // `deny_unknown_fields` reports unknown keys; surface them as such.
            if msg.starts_with("unknown field") {
                CoreError::InvalidConfigKey(key_path.to_string())
            } else {
                CoreError::InvalidValue {
                    key: key_path.to_string(),
                    reason: msg,
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> (tempfile::TempDir, Paths) {
        let d = tempfile::tempdir().unwrap();
        let p = Paths::from_dir(d.path().to_path_buf());
        (d, p)
    }

    #[test]
    fn every_spec_default() {
        let c = Config::default();
        assert_eq!((c.limits.global_cap, c.limits.max_children), (16, 8));
        assert_eq!(c.runs.stall_ms, 300_000);
        assert_eq!(c.runs.ask_timeout.to_string(), "30m");
        assert_eq!(c.usage.probe_ttl.to_string(), "5m");
        assert_eq!(c.usage.low_pct, 15);
        assert_eq!(c.usage.default_cooldown.to_string(), "5h");
        assert_eq!(
            c.router.priority,
            vec![
                Harness::Agy,
                Harness::Opencode,
                Harness::Codex,
                Harness::Claude
            ]
        );
        assert_eq!(
            (
                c.router.weights.headroom,
                c.router.weights.load,
                c.router.weights.cost
            ),
            (1.0, 0.5, 0.3)
        );
        assert!(c.router.instruction.starts_with(
            "Choose the least costly eligible choice that can reliably complete task_content"
        ));
        assert_eq!(
            c.router.jev.endpoint,
            "https://openrouter.ai/api/alpha/decisions"
        );
        assert_eq!(c.router.jev.model, "~typesafe/jev-latest");
        assert_eq!(c.router.jev.key_env, "OPENROUTER_API_KEY");
        assert_eq!(c.router.jev.key_file, None);
        assert_eq!(c.router.jev.timeout.to_string(), "20s");
        assert_eq!(
            (c.router.policy.min_confidence, c.router.policy.min_margin),
            (0.6, 0.1)
        );
        assert_eq!(c.worktrees.dir, "{repo_parent}/{repo}-worktrees");
        assert_eq!(c.worktrees.branch_prefix, "so/");
        assert_eq!(c.accounts.wrapper_dir, "~/.local/bin");
        assert_eq!(
            (c.ui.bind.as_str(), c.ui.port, c.ui.token.clone()),
            ("127.0.0.1", 7878, None)
        );
        assert_eq!(c.supervisor.idle_exit.to_string(), "10m");
        assert_eq!(c.notifications.discord_webhook_env, "SO_DISCORD_WEBHOOK");
        assert!(c.notifications.desktop);
        assert_eq!(
            c.notifications.on,
            vec![
                "escalated",
                "question_waiting_10m",
                "session_finished",
                "account_exhausted"
            ]
        );
        assert_eq!(c.defaults, Defaults::default());
        assert!(c.choices.is_empty());
    }

    #[test]
    fn missing_file_gives_defaults_and_save_round_trips() {
        let (_d, paths) = tmp();
        assert_eq!(Config::load(&paths).unwrap(), Config::default());
        let mut c = Config::default();
        c.ui.token = Some("t".into());
        c.defaults.harness = Some(Harness::Codex);
        c.choices.push(Choice {
            id: "x".into(),
            harness: Harness::Agy,
            model: "m".into(),
            effort: Some("medium".into()),
            roles: vec!["worker".into()],
            description: "d".into(),
            enabled: true,
        });
        c.save(&paths).unwrap();
        assert_eq!(Config::load(&paths).unwrap(), c);
    }

    #[test]
    fn partial_file_loads() {
        let (_d, paths) = tmp();
        std::fs::write(
            paths.config_toml(),
            "[limits]\nglobal_cap = 4\n\n[router.weights]\nload = 2.0\n\n[[choices]]\nid = \"a\"\nharness = \"codex\"\nmodel = \"m\"\n",
        )
        .unwrap();
        let c = Config::load(&paths).unwrap();
        assert_eq!(c.limits.global_cap, 4);
        assert_eq!(c.limits.max_children, 8);
        assert_eq!(c.router.weights.load, 2.0);
        assert_eq!(c.router.weights.headroom, 1.0);
        assert_eq!(c.ui.port, 7878);
        assert_eq!(c.choices.len(), 1);
    }

    #[test]
    fn get_dotted_keys() {
        let c = Config::default();
        assert_eq!(
            c.get("limits.global_cap").unwrap(),
            toml::Value::Integer(16)
        );
        assert_eq!(
            c.get("router.weights.load").unwrap(),
            toml::Value::Float(0.5)
        );
        assert_eq!(
            c.get("runs.ask_timeout").unwrap(),
            toml::Value::String("30m".into())
        );
        assert!(c.get("router.nope").is_err());
        assert!(c.get("limits.global_cap.x").is_err());
        assert!(c.get("ui.token").is_err());
    }

    #[test]
    fn set_valid_values() {
        let mut c = Config::default();
        c.set("limits.global_cap", "4").unwrap();
        assert_eq!(c.limits.global_cap, 4);
        c.set("runs.ask_timeout", "45m").unwrap();
        assert_eq!(c.runs.ask_timeout.to_string(), "45m");
        c.set("ui.bind", "0.0.0.0").unwrap();
        assert_eq!(c.ui.bind, "0.0.0.0");
        c.set("notifications.desktop", "false").unwrap();
        assert!(!c.notifications.desktop);
        c.set("router.weights.load", "0.9").unwrap();
        assert_eq!(c.router.weights.load, 0.9);
        c.set("router.priority", "[\"codex\", \"agy\"]").unwrap();
        assert_eq!(c.router.priority, vec![Harness::Codex, Harness::Agy]);
        c.set("defaults.harness", "claude").unwrap();
        assert_eq!(c.defaults.harness, Some(Harness::Claude));
        c.set("ui.token", "12345").unwrap();
        assert_eq!(c.ui.token.as_deref(), Some("12345"));
        c.set("router.jev.key_file", "~/x/.env").unwrap();
        assert_eq!(c.router.jev.key_file.as_deref(), Some("~/x/.env"));
    }

    #[test]
    fn set_rejects_bad_values_and_keys() {
        let mut c = Config::default();
        let before = c.clone();
        assert!(matches!(
            c.set("limits.global_cap", "lots"),
            Err(CoreError::InvalidValue { .. })
        ));
        assert!(c.set("limits.global_cap", "-1").is_err());
        assert!(c.set("runs.ask_timeout", "soon").is_err());
        assert!(c.set("ui.port", "70000").is_err());
        assert!(c.set("defaults.harness", "gemini").is_err());
        assert!(matches!(
            c.set("limits.nope", "1"),
            Err(CoreError::InvalidConfigKey(_))
        ));
        assert!(matches!(
            c.set("nope.x", "1"),
            Err(CoreError::InvalidConfigKey(_))
        ));
        assert!(c.set("limits..x", "1").is_err());
        assert_eq!(c, before);
    }
}

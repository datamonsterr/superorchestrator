//! Shared operations for inspecting, changing and testing the decider.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use so_core::config::{ENV_ROUTER_FALLBACK_MODEL, ENV_ROUTER_MODEL, Jev, JevModels, OnUnreachable};
use so_core::{Config, Paths};
use so_router::jev::{self, JevRequest};

/// Why a decider operation failed.
#[derive(Debug, thiserror::Error)]
pub enum DeciderError {
    /// The request was rejected; the message names the field and never echoes a value.
    #[error("{0}")]
    Invalid(String),
    /// The operation failed for a reason that is not the caller's fault.
    #[error("{0}")]
    Internal(String),
}

fn internal(error: impl std::fmt::Display) -> DeciderError {
    DeciderError::Internal(error.to_string())
}

fn invalid(message: impl Into<String>) -> DeciderError {
    DeciderError::Invalid(message.into())
}

/// An environment variable that currently overrides a configured field.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EnvOverride {
    pub name: String,
    pub field: String,
    pub value: Option<String>,
}

/// Where the decider key comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum KeySource {
    Env,
    File,
    Unset,
}

/// The decider key's origin and a masked hint; the key itself is never sent.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct KeyView {
    pub source: KeySource,
    pub name: Option<String>,
    pub masked: Option<String>,
}

/// Model ids offered as defaults in the UI.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Suggestions {
    pub model: String,
    pub fallback_model: Option<String>,
}

/// The decider settings as stored on disk, plus what is effective right now.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DeciderView {
    pub endpoint: String,
    pub model: String,
    pub fallback_model: Option<String>,
    pub on_unreachable: OnUnreachable,
    pub timeout: String,
    pub min_confidence: f64,
    pub min_margin: f64,
    pub effective: JevModels,
    pub env_overrides: Vec<EnvOverride>,
    pub key: KeyView,
    pub suggestions: Suggestions,
    pub restart_required: bool,
}

/// How a patch changes the decider key. Deliberately not `Serialize`, so a pasted secret
/// cannot be written out by accident.
#[derive(Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum KeyPatch {
    /// Read the key from this environment variable.
    Env(String),
    /// Read the key from this dotenv file.
    File(String),
    /// Store this key in `decider.env` in the config dir.
    Paste(String),
}

impl std::fmt::Debug for KeyPatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Env(v) => f.debug_tuple("Env").field(v).finish(),
            Self::File(v) => f.debug_tuple("File").field(v).finish(),
            Self::Paste(_) => f.write_str("Paste([REDACTED])"),
        }
    }
}

/// A partial update of the decider settings; absent fields are left alone.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct DeciderPatch {
    pub endpoint: Option<String>,
    pub model: Option<String>,
    pub fallback_model: Option<String>,
    pub on_unreachable: Option<OnUnreachable>,
    pub timeout: Option<String>,
    pub min_confidence: Option<f64>,
    pub min_margin: Option<f64>,
    pub key: Option<KeyPatch>,
}

const PATCH_FIELDS: [&str; 8] = [
    "endpoint",
    "model",
    "fallback_model",
    "on_unreachable",
    "timeout",
    "min_confidence",
    "min_margin",
    "key",
];

impl DeciderPatch {
    /// Parses an untrusted JSON value. Errors name the field and never include a submitted
    /// value, so a pasted secret cannot leak through a type error.
    pub fn from_value(value: serde_json::Value) -> Result<Self, DeciderError> {
        let serde_json::Value::Object(fields) = value else {
            return Err(invalid("decider patch must be a JSON object"));
        };
        for (name, value) in &fields {
            if !PATCH_FIELDS.contains(&name.as_str()) {
                return Err(invalid("unknown decider patch field"));
            }
            let mut one = serde_json::Map::new();
            one.insert(name.clone(), value.clone());
            if serde_json::from_value::<Self>(serde_json::Value::Object(one)).is_err() {
                return Err(invalid(if name == "key" {
                    "key: expected exactly one of env, file or paste, each a string".to_owned()
                } else {
                    format!("{name}: invalid value")
                }));
            }
        }
        serde_json::from_value(serde_json::Value::Object(fields))
            .map_err(|_| invalid("invalid decider patch"))
    }
}

/// Optional prompt for a live decider test.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct DeciderTestRequest {
    pub prompt: Option<String>,
}
/// The outcome of a live decider test.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DeciderTestResult {
    pub ok: bool,
    pub complexity: Option<f64>,
    pub level: Option<String>,
    pub confidence: Option<f64>,
    pub model: Option<String>,
    pub latency_ms: u64,
    pub error: Option<String>,
}

/// Masks a secret as `…` plus its last four characters.
fn mask(secret: &str) -> String {
    let chars: Vec<char> = secret.chars().collect();
    if chars.len() <= 4 {
        return "…".into();
    }
    format!("…{}", chars[chars.len() - 4..].iter().collect::<String>())
}

/// Describes where the key comes from, using `env` for process environment lookups.
pub fn key_view(cfg: &Jev, env: &impl Fn(&str) -> Option<String>) -> KeyView {
    let env_key = env(&cfg.key_env)
        .filter(|v| !v.trim().is_empty())
        .map(|v| v.trim().to_owned());
    let file_key = || {
        cfg.key_file
            .as_deref()
            .and_then(|p| so_core::expand_tilde(p).ok())
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|text| jev::parse_dotenv(&text).remove(&cfg.key_env))
            .filter(|v| !v.is_empty())
    };
    if let Some(k) = env_key {
        KeyView {
            source: KeySource::Env,
            name: Some(cfg.key_env.clone()),
            masked: Some(mask(&k)),
        }
    } else if let Some(k) = file_key() {
        KeyView {
            source: KeySource::File,
            name: cfg.key_file.clone(),
            masked: Some(mask(&k)),
        }
    } else {
        KeyView {
            source: KeySource::Unset,
            name: Some(cfg.key_env.clone()),
            masked: None,
        }
    }
}

/// The decider settings on disk against the process environment.
pub fn view(paths: &Paths, running: &Config) -> Result<DeciderView, DeciderError> {
    view_with_env(paths, running, jev::process_env)
}

/// Like [`view`], with an injected environment lookup.
pub fn view_with_env(
    paths: &Paths,
    running: &Config,
    env: impl Fn(&str) -> Option<String>,
) -> Result<DeciderView, DeciderError> {
    let disk = Config::load(paths).map_err(internal)?;
    let j = &disk.router.jev;
    let mut overrides = Vec::new();
    for (name, field) in [
        (ENV_ROUTER_MODEL, "model"),
        (ENV_ROUTER_FALLBACK_MODEL, "fallback_model"),
    ] {
        if let Some(value) = env(name).filter(|v| !v.trim().is_empty()) {
            overrides.push(EnvOverride {
                name: name.into(),
                field: field.into(),
                value: (name != j.key_env).then_some(value),
            });
        }
    }
    if j.key_file.is_some() && env(&j.key_env).is_some_and(|v| !v.trim().is_empty()) {
        overrides.push(EnvOverride {
            name: j.key_env.clone(),
            field: "key_file".into(),
            value: None,
        });
    }
    let mut effective = j.resolve_models(&env);
    if env(&j.key_env).is_some_and(|v| !v.trim().is_empty()) {
        if j.key_env == ENV_ROUTER_MODEL {
            effective.primary = "[REDACTED]".into();
        }
        if j.key_env == ENV_ROUTER_FALLBACK_MODEL && effective.fallback.is_some() {
            effective.fallback = Some("[REDACTED]".into());
        }
    }
    let default = Jev::default();
    Ok(DeciderView {
        endpoint: j.endpoint.clone(),
        model: j.model.clone(),
        fallback_model: j.fallback_model.clone(),
        on_unreachable: j.on_unreachable,
        timeout: j.timeout.to_string(),
        min_confidence: disk.router.policy.min_confidence,
        min_margin: disk.router.policy.min_margin,
        effective,
        key: key_view(j, &env),
        env_overrides: overrides,
        suggestions: Suggestions {
            model: default.model,
            fallback_model: default.fallback_model,
        },
        restart_required: disk != *running,
    })
}

fn valid_env(s: &str) -> bool {
    let mut c = s.chars();
    c.next().is_some_and(|c| c == '_' || c.is_ascii_uppercase())
        && c.all(|c| c == '_' || c.is_ascii_uppercase() || c.is_ascii_digit())
}
fn valid_model(s: &str) -> bool {
    !s.is_empty() && !s.chars().any(char::is_whitespace)
}

/// Validates and applies a patch, then returns the new view.
pub fn set(
    paths: &Paths,
    running: &Config,
    patch: DeciderPatch,
) -> Result<DeciderView, DeciderError> {
    set_with_env(paths, running, patch, jev::process_env)
}

/// Like [`set`], with an injected environment lookup for the returned view.
pub fn set_with_env(
    paths: &Paths,
    running: &Config,
    patch: DeciderPatch,
    env: impl Fn(&str) -> Option<String>,
) -> Result<DeciderView, DeciderError> {
    set_with_writer(paths, running, patch, env, |paths, refs| {
        Config::set_many_in_file(paths, refs)
            .map(|_| ())
            .map_err(|e| e.to_string())
    })
}

fn set_with_writer(
    paths: &Paths,
    running: &Config,
    patch: DeciderPatch,
    env: impl Fn(&str) -> Option<String>,
    write_config: impl FnOnce(&Paths, &[(&str, &str)]) -> Result<(), String>,
) -> Result<DeciderView, DeciderError> {
    let mut entries: Vec<(String, String)> = Vec::new();
    let mut push_string = |key: &str, value: &str| {
        entries.push((key.into(), serde_json::to_string(value).unwrap_or_default()))
    };
    if let Some(v) = &patch.endpoint {
        if !jev::endpoint_allowed(v) {
            return Err(invalid(
                "endpoint: must be https, or http only to a loopback host",
            ));
        }
        push_string("router.jev.endpoint", v);
    }
    if let Some(v) = &patch.model {
        if !valid_model(v) {
            return Err(invalid(
                "model: must be a non-empty model id without spaces",
            ));
        }
        push_string("router.jev.model", v);
    }
    if let Some(v) = &patch.fallback_model {
        if !v.is_empty() && !valid_model(v) {
            return Err(invalid(
                "fallback_model: must be empty, none, or a model id without spaces",
            ));
        }
        push_string("router.jev.fallback_model", v);
    }
    if let Some(v) = patch.on_unreachable {
        push_string(
            "router.jev.on_unreachable",
            match v {
                OnUnreachable::Error => "error",
                OnUnreachable::Heuristic => "heuristic",
            },
        );
    }
    if let Some(v) = &patch.timeout {
        let duration = v
            .parse::<so_core::duration::HumanDuration>()
            .map_err(|_| invalid("timeout: must be a duration from 1s to 120s"))?;
        if !(std::time::Duration::from_secs(1)..=std::time::Duration::from_secs(120))
            .contains(&duration.as_duration())
        {
            return Err(invalid("timeout: must be from 1s to 120s"));
        }
        push_string("router.jev.timeout", v);
    }
    let mut paste = None;
    let disk = Config::load(paths).map_err(internal)?;
    match &patch.key {
        Some(KeyPatch::Env(v)) => {
            if !valid_env(v) {
                return Err(invalid(
                    "key.env: must start with an uppercase letter or underscore and contain only uppercase letters, digits, or underscores",
                ));
            }
            if v == ENV_ROUTER_MODEL || v == ENV_ROUTER_FALLBACK_MODEL {
                return Err(invalid(format!(
                    "key.env: {v} is a model override, not a key variable"
                )));
            }
            push_string("router.jev.key_env", v);
        }
        Some(KeyPatch::File(v)) => {
            if v.is_empty() {
                return Err(invalid("key.file: must be a non-empty path"));
            }
            push_string("router.jev.key_file", v);
        }
        Some(KeyPatch::Paste(v)) => {
            if !valid_model(v) || v.starts_with(['\"', '\'']) || v.ends_with(['\"', '\'']) {
                return Err(invalid(
                    "key.paste: must be non-empty, without spaces or surrounding quotes",
                ));
            }
            let path = paths.config_dir.join("decider.env");
            let path = path
                .to_str()
                .ok_or_else(|| invalid("key.file: invalid path"))?;
            push_string("router.jev.key_file", path);
            paste = Some(v);
        }
        None => {}
    }
    for (key, value) in [
        ("min_confidence", patch.min_confidence),
        ("min_margin", patch.min_margin),
    ] {
        if let Some(v) = value {
            if !v.is_finite() || !(0.0..=1.0).contains(&v) {
                return Err(invalid(format!("{key}: must be between 0 and 1")));
            }
            entries.push((format!("router.policy.{key}"), format!("{v:?}")));
        }
    }
    let mut trial = disk;
    for (k, v) in &entries {
        trial.set(k, v).map_err(|e| invalid(e.to_string()))?;
    }
    let key_path = paths.config_dir.join("decider.env");
    let mut previous = None;
    if let Some(secret) = paste {
        so_core::fsutil::create_private_dirs(&paths.config_dir).map_err(internal)?;
        previous = Some(match std::fs::read(&key_path) {
            Ok(bytes) => Some((
                bytes,
                std::fs::metadata(&key_path)
                    .map_err(internal)?
                    .permissions(),
            )),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(internal(e)),
        });
        so_core::fsutil::write_atomic(
            &key_path,
            format!("{}={}\n", trial.router.jev.key_env, secret).as_bytes(),
        )
        .map_err(internal)?;
    }
    let refs: Vec<_> = entries
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    if let Err(error) = write_config(paths, &refs) {
        if let Some(previous) = previous {
            let rollback = match previous {
                Some((bytes, permissions)) => so_core::fsutil::write_atomic(&key_path, &bytes)
                    .map_err(|e| e.to_string())
                    .and_then(|()| {
                        std::fs::set_permissions(&key_path, permissions).map_err(|e| e.to_string())
                    }),
                None => std::fs::remove_file(&key_path).map_err(|e| e.to_string()),
            };
            if let Err(rollback_error) = rollback {
                return Err(internal(format!(
                    "{error}; failed to restore decider.env: {rollback_error}"
                )));
            }
        }
        return Err(internal(error));
    }
    view_with_env(paths, running, env)
}

/// Sends one live request to the decider using the on-disk settings.
pub async fn test_decider(
    paths: &Paths,
    http: &reqwest::Client,
    prompt: Option<String>,
) -> Result<DeciderTestResult, DeciderError> {
    let config = Config::load(paths).map_err(internal)?;
    let started = std::time::Instant::now();
    let cfg = &config.router.jev;
    let key = jev::resolve_key(cfg, jev::process_env);
    let result = if let Some(key) = key {
        let prompt =
            prompt.unwrap_or_else(|| "Add a small unit test for an existing function.".into());
        let req = JevRequest {
            task: &prompt,
            system_instruction: "",
            routing_instruction: &config.router.instruction,
            agent: "worker",
        };
        jev::decide(
            http,
            cfg,
            &cfg.resolve_models(jev::process_env),
            &config.router.policy,
            &key,
            &req,
        )
        .await
        .map_err(|e| e.reason)
    } else {
        Err("no decider key available".into())
    };
    let latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    Ok(match result {
        Ok(d) => {
            let complexity = d.validated.complexity;
            let level = so_router::levels::levels_by_distance(&config.router.levels, complexity)
                .first()
                .map(|&i| config.router.levels[i].name.clone());
            DeciderTestResult {
                ok: true,
                complexity: Some(complexity),
                level,
                confidence: Some(d.validated.confidence),
                model: Some(d.model),
                latency_ms,
                error: None,
            }
        }
        Err(error) => DeciderTestResult {
            ok: false,
            complexity: None,
            level: None,
            confidence: None,
            model: None,
            latency_ms,
            error: Some(error),
        },
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn config_write_failure_restores_previous_key_or_removes_new_key() {
        for existing in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let paths = Paths::from_dir(dir.path().to_path_buf());
            let key_path = paths.config_dir.join("decider.env");
            if existing {
                std::fs::write(&key_path, b"old key\n").unwrap();
            }
            let result = set_with_writer(
                &paths,
                &Config::default(),
                DeciderPatch {
                    key: Some(KeyPatch::Paste("new-key".into())),
                    ..Default::default()
                },
                |_| None,
                |_, _| {
                    assert!(key_path.exists());
                    Err("simulated config write failure".into())
                },
            );
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("simulated config write failure")
            );
            if existing {
                assert_eq!(std::fs::read(&key_path).unwrap(), b"old key\n");
            } else {
                assert!(!key_path.exists());
            }
        }
    }
}

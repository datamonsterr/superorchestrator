//! Shared operations for inspecting, changing and testing the decider.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use so_core::config::{ENV_ROUTER_FALLBACK_MODEL, ENV_ROUTER_MODEL, Jev, JevModels, OnUnreachable};
use so_core::{Config, Paths};
use so_router::jev::{self, JevRequest};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EnvOverride {
    pub name: String,
    pub field: String,
    pub value: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct KeyView {
    pub source: String,
    pub name: Option<String>,
    pub masked: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Suggestions {
    pub model: String,
    pub fallback_model: Option<String>,
}
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
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum KeyPatch {
    Env(String),
    File(String),
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
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct DeciderPatch {
    pub endpoint: Option<String>,
    pub model: Option<String>,
    pub fallback_model: Option<String>,
    pub on_unreachable: Option<String>,
    pub timeout: Option<String>,
    pub min_confidence: Option<f64>,
    pub min_margin: Option<f64>,
    pub key: Option<KeyPatch>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct DeciderTestRequest {
    pub prompt: Option<String>,
}
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

fn mask(secret: &str) -> String {
    let chars: Vec<char> = secret.chars().collect();
    if chars.len() <= 4 {
        return "…".into();
    }
    format!("…{}", chars[chars.len() - 4..].iter().collect::<String>())
}
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
            source: "env".into(),
            name: Some(cfg.key_env.clone()),
            masked: Some(mask(&k)),
        }
    } else if let Some(k) = file_key() {
        KeyView {
            source: "file".into(),
            name: cfg.key_file.clone(),
            masked: Some(mask(&k)),
        }
    } else {
        KeyView {
            source: "unset".into(),
            name: Some(cfg.key_env.clone()),
            masked: None,
        }
    }
}
pub fn view(paths: &Paths, running: &Config) -> Result<DeciderView, String> {
    view_with_env(paths, running, jev::process_env)
}
pub fn view_with_env(
    paths: &Paths,
    running: &Config,
    env: impl Fn(&str) -> Option<String>,
) -> Result<DeciderView, String> {
    let disk = Config::load(paths).map_err(|e| e.to_string())?;
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

pub fn set(paths: &Paths, running: &Config, patch: DeciderPatch) -> Result<DeciderView, String> {
    set_with_writer(paths, running, patch, |paths, refs| {
        Config::set_many_in_file(paths, refs)
            .map(|_| ())
            .map_err(|e| e.to_string())
    })
}

fn set_with_writer(
    paths: &Paths,
    running: &Config,
    patch: DeciderPatch,
    write_config: impl FnOnce(&Paths, &[(&str, &str)]) -> Result<(), String>,
) -> Result<DeciderView, String> {
    let mut entries: Vec<(String, String)> = Vec::new();
    let mut push_string = |key: &str, value: &str| {
        entries.push((key.into(), serde_json::to_string(value).unwrap_or_default()))
    };
    if let Some(v) = &patch.endpoint {
        if !jev::endpoint_allowed(v) {
            return Err("endpoint: must be https, or http only to a loopback host".into());
        }
        push_string("router.jev.endpoint", v);
    }
    if let Some(v) = &patch.model {
        if !valid_model(v) {
            return Err("model: must be a non-empty model id without spaces".into());
        }
        push_string("router.jev.model", v);
    }
    if let Some(v) = &patch.fallback_model {
        if !v.is_empty() && !valid_model(v) {
            return Err("fallback_model: must be empty, none, or a model id without spaces".into());
        }
        push_string("router.jev.fallback_model", v);
    }
    if let Some(v) = &patch.on_unreachable {
        if v != "error" && v != "heuristic" {
            return Err("on_unreachable: expected error or heuristic".into());
        }
        push_string("router.jev.on_unreachable", v);
    }
    if let Some(v) = &patch.timeout {
        let duration = v
            .parse::<so_core::duration::HumanDuration>()
            .map_err(|_| "timeout: must be a duration from 1s to 120s")?;
        if !(std::time::Duration::from_secs(1)..=std::time::Duration::from_secs(120))
            .contains(&duration.as_duration())
        {
            return Err("timeout: must be from 1s to 120s".into());
        }
        push_string("router.jev.timeout", v);
    }
    let mut paste = None;
    let disk = Config::load(paths).map_err(|e| e.to_string())?;
    match &patch.key {
        Some(KeyPatch::Env(v)) => {
            if !valid_env(v) {
                return Err("key.env: must start with an uppercase letter or underscore and contain only uppercase letters, digits, or underscores".into());
            }
            push_string("router.jev.key_env", v);
        }
        Some(KeyPatch::File(v)) => {
            if v.is_empty() {
                return Err("key.file: must be a non-empty path".into());
            }
            push_string("router.jev.key_file", v);
        }
        Some(KeyPatch::Paste(v)) => {
            if !valid_model(v) || v.starts_with(['\"', '\'']) || v.ends_with(['\"', '\'']) {
                return Err(
                    "key.paste: must be non-empty, without spaces or surrounding quotes".into(),
                );
            }
            let path = paths.config_dir.join("decider.env");
            let path = path.to_str().ok_or("key.file: invalid path")?;
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
                return Err(format!("{key}: must be between 0 and 1"));
            }
            entries.push((format!("router.policy.{key}"), format!("{v:?}")));
        }
    }
    let mut trial = disk;
    for (k, v) in &entries {
        trial.set(k, v).map_err(|e| e.to_string())?;
    }
    let key_path = paths.config_dir.join("decider.env");
    let mut previous = None;
    if let Some(secret) = paste {
        so_core::fsutil::create_private_dirs(&paths.config_dir).map_err(|e| e.to_string())?;
        previous = Some(match std::fs::read(&key_path) {
            Ok(bytes) => Some((
                bytes,
                std::fs::metadata(&key_path)
                    .map_err(|e| e.to_string())?
                    .permissions(),
            )),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.to_string()),
        });
        so_core::fsutil::write_atomic(
            &key_path,
            format!("{}={}\n", trial.router.jev.key_env, secret).as_bytes(),
        )
        .map_err(|e| e.to_string())?;
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
                return Err(format!(
                    "{error}; failed to restore decider.env: {rollback_error}"
                ));
            }
        }
        return Err(error);
    }
    view(paths, running)
}

pub async fn test_decider(
    paths: &Paths,
    http: &reqwest::Client,
    prompt: Option<String>,
) -> Result<DeciderTestResult, String> {
    let config = Config::load(paths).map_err(|e| e.to_string())?;
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
                |_, _| {
                    assert!(key_path.exists());
                    Err("simulated config write failure".into())
                },
            );
            assert!(
                result
                    .unwrap_err()
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

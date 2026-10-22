//! Model catalog (SPEC 8.1): known models per harness with tier, efforts, pool and cost.
//! Used only for discovery and validation; it never edits choices.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use so_accounts::{HostEnv, build_launch};
use so_core::{AccountFile, Choice, Harness, Paths};
use so_usage::{ProbeRequest, ProbeRunner, ResponseMatcher};

use crate::error::{Result, RouterError};
use crate::http::read_capped;

const DEFAULT_CATALOG: &str = include_str!("../catalog/default.toml");
const MODELS_DEV_URL: &str = "https://models.dev/api.json";
const MODELS_DEV_MAX: usize = 32 << 20;
const MODELS_DEV_TTL: chrono::Duration = chrono::Duration::hours(24);
const COLLECT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    High,
    Mid,
    Fast,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Cost {
    Free,
    Included,
    Paid,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelEntry {
    pub id: String,
    pub tier: Tier,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub efforts: Vec<String>,
    /// agy only: the usage pool (`gemini` or `claude`) the model draws on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pool: Option<String>,
    pub cost: Cost,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HarnessModels {
    #[serde(default)]
    pub models: Vec<ModelEntry>,
}

/// The parsed `models.toml`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Catalog {
    pub harnesses: BTreeMap<Harness, HarnessModels>,
}

/// Tier guessed from a model name, for models the catalog has not seen.
pub fn infer_tier(model: &str) -> Tier {
    let n = model.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| n.contains(w));
    if has(&[
        "pro", "opus", "max", "ultra", "large", "sol", "terra", "astra", "fable",
    ]) {
        Tier::High
    } else if has(&[
        "flash",
        "mini",
        "nano",
        "lite",
        "haiku",
        "lightning",
        "small",
        "spark",
    ]) {
        Tier::Fast
    } else {
        Tier::Mid
    }
}

/// agy usage pool of a model name.
pub fn infer_agy_pool(model: &str) -> &'static str {
    if model.starts_with("gemini") {
        "gemini"
    } else {
        "claude"
    }
}

fn infer_cost(harness: Harness, model: &str) -> Cost {
    if model.ends_with("-free") {
        Cost::Free
    } else if harness == Harness::Opencode {
        Cost::Paid
    } else {
        Cost::Included
    }
}

impl Catalog {
    /// The catalog that ships in the binary.
    pub fn default_catalog() -> Self {
        toml::from_str(DEFAULT_CATALOG).expect("embedded catalog is valid")
    }

    /// `models.toml`, or the shipped default when the file is missing or holds no models.
    pub fn load(paths: &Paths) -> Result<Self> {
        let path = paths.models_toml();
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default_catalog());
            }
            Err(e) => {
                return Err(RouterError::Io {
                    path,
                    reason: e.to_string(),
                });
            }
        };
        let cat: Catalog = toml::from_str(&text).map_err(|e| RouterError::Parse {
            path: path.clone(),
            reason: e.to_string(),
        })?;
        Ok(if cat.model_count() == 0 {
            Self::default_catalog()
        } else {
            cat
        })
    }

    pub fn save(&self, paths: &Paths) -> Result<()> {
        let text = toml::to_string_pretty(self).map_err(|e| RouterError::Parse {
            path: paths.models_toml(),
            reason: e.to_string(),
        })?;
        let text = format!(
            "# Model catalog (SPEC 8.1). Discovery and validation only; never edits choices.\n{text}"
        );
        write_file(&paths.models_toml(), &text)
    }

    pub fn model_count(&self) -> usize {
        self.harnesses.values().map(|h| h.models.len()).sum()
    }

    pub fn get(&self, harness: Harness, model: &str) -> Option<&ModelEntry> {
        self.harnesses
            .get(&harness)?
            .models
            .iter()
            .find(|m| m.id == model)
    }

    pub fn contains(&self, harness: Harness, model: &str) -> bool {
        self.get(harness, model).is_some()
    }

    /// Tier of a model; unknown models are `mid`.
    pub fn tier(&self, harness: Harness, model: &str) -> Tier {
        self.get(harness, model).map_or(Tier::Mid, |m| m.tier)
    }

    /// Cost class of a model; unknown models are guessed from the name.
    pub fn cost(&self, harness: Harness, model: &str) -> Cost {
        self.get(harness, model)
            .map_or_else(|| infer_cost(harness, model), |m| m.cost)
    }

    /// Usage pool a choice draws on: the catalog pool for agy models, `default` otherwise.
    pub fn pool_for(&self, harness: Harness, model: &str) -> String {
        if harness != Harness::Agy {
            return "default".into();
        }
        self.get(harness, model)
            .and_then(|m| m.pool.clone())
            .unwrap_or_else(|| infer_agy_pool(model).to_string())
    }

    fn insert(&mut self, harness: Harness, entry: ModelEntry) {
        self.harnesses
            .entry(harness)
            .or_default()
            .models
            .push(entry);
    }

    /// Flat `(harness, entry)` listing in harness then catalog order.
    pub fn entries(&self) -> Vec<(Harness, &ModelEntry)> {
        self.harnesses
            .iter()
            .flat_map(|(h, hm)| hm.models.iter().map(move |m| (*h, m)))
            .collect()
    }
}

fn write_file(path: &Path, text: &str) -> Result<()> {
    let io = |e: std::io::Error| RouterError::Io {
        path: path.to_path_buf(),
        reason: e.to_string(),
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(io)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text).map_err(io)?;
    std::fs::rename(&tmp, path).map_err(io)
}

// ---------------------------------------------------------------- collector parsers

/// `agy models`: lines of `<id>\t<display name>` after a header line.
pub fn parse_agy_models(out: &str) -> Vec<String> {
    out.lines()
        .filter_map(|l| {
            let (id, _) = l.split_once('\t')?;
            let id = id.trim();
            (!id.is_empty() && !id.contains(char::is_whitespace)).then(|| id.to_string())
        })
        .collect()
}

/// `opencode models`: one `provider/model` per line.
pub fn parse_opencode_models(out: &str) -> Vec<String> {
    out.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && l.contains('/') && !l.contains(char::is_whitespace))
        .map(String::from)
        .collect()
}

/// Codex `model/list` response line: `(model, supported efforts)`, hidden models skipped.
pub fn parse_codex_models(line: &str) -> Vec<(String, Vec<String>)> {
    let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
        return Vec::new();
    };
    let Some(data) = v.pointer("/result/data").and_then(Value::as_array) else {
        return Vec::new();
    };
    data.iter()
        .filter(|m| !m.get("hidden").and_then(Value::as_bool).unwrap_or(false))
        .filter_map(|m| {
            let id = m
                .get("model")
                .or_else(|| m.get("id"))
                .and_then(Value::as_str)?;
            let efforts = m
                .get("supportedReasoningEfforts")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|e| e.get("reasoningEffort").and_then(Value::as_str))
                        .map(String::from)
                        .collect()
                })
                .unwrap_or_default();
            Some((id.to_string(), efforts))
        })
        .collect()
}

/// Free models of the `opencode` provider in a models.dev document, as `opencode/<id>`.
pub fn parse_models_dev_free(doc: &Value) -> Vec<String> {
    let Some(models) = doc.pointer("/opencode/models").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut out: Vec<String> = models
        .iter()
        .filter(|(_, m)| {
            let c = |k: &str| m.pointer(&format!("/cost/{k}")).and_then(Value::as_f64);
            c("input") == Some(0.0) && c("output") == Some(0.0)
        })
        .map(|(id, _)| format!("opencode/{id}"))
        .collect();
    out.sort();
    out
}

// ---------------------------------------------------------------- refresh

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceStatus {
    Ok,
    Skipped,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
pub struct SourceReport {
    pub source: String,
    pub status: SourceStatus,
    pub models: usize,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelRef {
    pub harness: Harness,
    pub model: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChoiceRef {
    pub choice: String,
    pub harness: Harness,
    pub model: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RefreshReport {
    pub sources: Vec<SourceReport>,
    pub added: Vec<ModelRef>,
    /// Choices whose model the catalog does not know.
    pub unknown_choice_models: Vec<ChoiceRef>,
    /// Catalog models no choice uses.
    pub unused_models: Vec<ModelRef>,
}

pub struct RefreshEnv<'a> {
    pub paths: &'a Paths,
    pub host: &'a HostEnv,
    pub accounts: &'a [AccountFile],
    pub runner: &'a dyn ProbeRunner,
    pub http: &'a reqwest::Client,
    /// models.dev document URL; `None` uses the public one.
    pub models_dev_url: Option<&'a str>,
    pub now: DateTime<Utc>,
}

fn first_account<'a>(env: &RefreshEnv<'a>, harness: Harness) -> Option<&'a AccountFile> {
    env.accounts
        .iter()
        .find(|a| a.enabled && a.harness == harness)
}

async fn run_collector(
    env: &RefreshEnv<'_>,
    account: &AccountFile,
    subcommand: &[&str],
    stdin_lines: Vec<String>,
    wait_for: ResponseMatcher,
) -> std::result::Result<String, String> {
    let launch = build_launch(account, env.host, None, so_accounts::LaunchMode::Supervised)
        .map_err(|e| e.to_string())?;
    let mut args = launch.argv_prefix.clone();
    args.extend(launch.extra_args.clone());
    args.extend(subcommand.iter().map(|s| s.to_string()));
    let out = env
        .runner
        .run(ProbeRequest {
            program: launch.program,
            args,
            env: launch.vars,
            stdin_lines,
            wait_for,
            timeout: COLLECT_TIMEOUT,
        })
        .await
        .map_err(|e| e.to_string())?;
    Ok(out.stdout)
}

/// The codex app-server conversation that lists models.
pub fn codex_model_list_lines() -> Vec<String> {
    vec![
        json!({"id": 1, "method": "initialize", "params": {"clientInfo": {
            "name": "superorchestrator", "title": null, "version": env!("CARGO_PKG_VERSION")}}})
        .to_string(),
        json!({"method": "initialized"}).to_string(),
        json!({"id": 2, "method": "model/list", "params": {"limit": 200, "includeHidden": false}})
            .to_string(),
    ]
}

struct Collected {
    harness: Harness,
    id: String,
    efforts: Vec<String>,
    free: bool,
}

fn source(
    name: &str,
    status: SourceStatus,
    models: usize,
    detail: impl Into<String>,
) -> SourceReport {
    SourceReport {
        source: name.into(),
        status,
        models,
        detail: detail.into(),
    }
}

/// Cached or freshly fetched list of free opencode models.
async fn free_models(env: &RefreshEnv<'_>) -> std::result::Result<(Vec<String>, String), String> {
    let cache = env.paths.config_dir.join("cache/models.dev.json");
    let cached: Option<(DateTime<Utc>, Vec<String>)> = std::fs::read_to_string(&cache)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| {
            let at = v.get("fetched_at")?.as_str()?.parse().ok()?;
            let free = v
                .get("free")?
                .as_array()?
                .iter()
                .filter_map(|m| m.as_str().map(String::from))
                .collect();
            Some((at, free))
        });
    if let Some((at, free)) = &cached
        && env.now - *at < MODELS_DEV_TTL
    {
        return Ok((free.clone(), "from the 24 h cache".into()));
    }
    let url = env.models_dev_url.unwrap_or(MODELS_DEV_URL);
    let fetched = async {
        let resp = env
            .http
            .get(url)
            .timeout(COLLECT_TIMEOUT)
            .send()
            .await
            .map_err(|e| e.without_url().to_string())?;
        if !resp.status().is_success() {
            return Err(format!("HTTP {}", resp.status().as_u16()));
        }
        let body = read_capped(resp, MODELS_DEV_MAX).await?;
        serde_json::from_slice::<Value>(&body).map_err(|e| format!("bad JSON: {e}"))
    }
    .await;
    match fetched {
        Ok(doc) => {
            let free = parse_models_dev_free(&doc);
            let text = json!({"fetched_at": env.now.to_rfc3339(), "free": free}).to_string();
            let detail = match write_file(&cache, &text) {
                Ok(()) => "fetched".to_string(),
                Err(e) => format!("fetched, cache not written: {e}"),
            };
            Ok((free, detail))
        }
        Err(e) => match cached {
            Some((_, free)) => Ok((free, format!("fetch failed ({e}); using the stale cache"))),
            None => Err(e),
        },
    }
}

/// Collects models from the harness CLIs and models.dev, merges them into `catalog` (existing
/// entries keep their tier, efforts and pool) and reports against `choices`. Does not write.
pub async fn refresh(
    env: &RefreshEnv<'_>,
    mut catalog: Catalog,
    choices: &[Choice],
) -> (Catalog, RefreshReport) {
    let mut sources = Vec::new();
    let mut collected: Vec<Collected> = Vec::new();

    match first_account(env, Harness::Codex) {
        None => sources.push(source(
            "codex",
            SourceStatus::Skipped,
            0,
            "no enabled codex account",
        )),
        Some(acct) => {
            match run_collector(
                env,
                acct,
                &["app-server", "--listen", "stdio://"],
                codex_model_list_lines(),
                ResponseMatcher::JsonRpcResponses,
            )
            .await
            {
                Ok(out) => {
                    let models = parse_codex_models(&out);
                    if models.is_empty() {
                        sources.push(source(
                            "codex",
                            SourceStatus::Failed,
                            0,
                            "no models in the reply",
                        ));
                    } else {
                        sources.push(source(
                            "codex",
                            SourceStatus::Ok,
                            models.len(),
                            "model/list",
                        ));
                        collected.extend(models.into_iter().map(|(id, efforts)| Collected {
                            harness: Harness::Codex,
                            id,
                            efforts,
                            free: false,
                        }));
                    }
                }
                Err(e) => sources.push(source("codex", SourceStatus::Failed, 0, e)),
            }
        }
    }

    let mut free_set: BTreeSet<String> = BTreeSet::new();
    match first_account(env, Harness::Opencode) {
        None => sources.push(source(
            "opencode",
            SourceStatus::Skipped,
            0,
            "no enabled opencode account",
        )),
        Some(acct) => {
            match run_collector(env, acct, &["models"], Vec::new(), ResponseMatcher::Eof).await {
                Ok(out) => {
                    let models = parse_opencode_models(&out);
                    if models.is_empty() {
                        sources.push(source(
                            "opencode",
                            SourceStatus::Failed,
                            0,
                            "no models listed",
                        ));
                    } else {
                        sources.push(source(
                            "opencode",
                            SourceStatus::Ok,
                            models.len(),
                            "opencode models",
                        ));
                        collected.extend(models.into_iter().map(|id| Collected {
                            harness: Harness::Opencode,
                            id,
                            efforts: Vec::new(),
                            free: false,
                        }));
                    }
                }
                Err(e) => sources.push(source("opencode", SourceStatus::Failed, 0, e)),
            }
        }
    }
    match free_models(env).await {
        Ok((free, detail)) => {
            sources.push(source("models.dev", SourceStatus::Ok, free.len(), detail));
            free_set.extend(free);
        }
        Err(e) => sources.push(source("models.dev", SourceStatus::Failed, 0, e)),
    }

    match first_account(env, Harness::Agy) {
        None => sources.push(source(
            "agy",
            SourceStatus::Skipped,
            0,
            "no enabled agy account",
        )),
        Some(acct) => {
            match run_collector(env, acct, &["models"], Vec::new(), ResponseMatcher::Eof).await {
                Ok(out) => {
                    let models = parse_agy_models(&out);
                    if models.is_empty() {
                        sources.push(source("agy", SourceStatus::Failed, 0, "no models listed"));
                    } else {
                        sources.push(source("agy", SourceStatus::Ok, models.len(), "agy models"));
                        collected.extend(models.into_iter().map(|id| Collected {
                            harness: Harness::Agy,
                            id,
                            efforts: Vec::new(),
                            free: false,
                        }));
                    }
                }
                Err(e) => sources.push(source("agy", SourceStatus::Failed, 0, e)),
            }
        }
    }

    let mut added = Vec::new();
    for c in collected {
        let free = c.free || (c.harness == Harness::Opencode && free_set.contains(&c.id));
        if let Some(existing) = catalog
            .harnesses
            .get_mut(&c.harness)
            .and_then(|h| h.models.iter_mut().find(|m| m.id == c.id))
        {
            if free {
                existing.cost = Cost::Free;
            }
            if existing.efforts.is_empty() {
                existing.efforts = c.efforts;
            }
            continue;
        }
        let mut cost = infer_cost(c.harness, &c.id);
        if free {
            cost = Cost::Free;
        }
        added.push(ModelRef {
            harness: c.harness,
            model: c.id.clone(),
        });
        catalog.insert(
            c.harness,
            ModelEntry {
                tier: infer_tier(&c.id),
                pool: (c.harness == Harness::Agy).then(|| infer_agy_pool(&c.id).to_string()),
                efforts: c.efforts,
                cost,
                id: c.id,
            },
        );
    }

    let report = RefreshReport {
        sources,
        added,
        unknown_choice_models: unknown_choice_models(&catalog, choices),
        unused_models: unused_models(&catalog, choices),
    };
    (catalog, report)
}

/// Choices whose `(harness, model)` is not in the catalog.
pub fn unknown_choice_models(catalog: &Catalog, choices: &[Choice]) -> Vec<ChoiceRef> {
    choices
        .iter()
        .filter(|c| !catalog.contains(c.harness, &c.model))
        .map(|c| ChoiceRef {
            choice: c.id.clone(),
            harness: c.harness,
            model: c.model.clone(),
        })
        .collect()
}

/// Catalog models no choice names.
pub fn unused_models(catalog: &Catalog, choices: &[Choice]) -> Vec<ModelRef> {
    let used: BTreeSet<(Harness, &str)> = choices
        .iter()
        .map(|c| (c.harness, c.model.as_str()))
        .collect();
    catalog
        .entries()
        .into_iter()
        .filter(|(h, m)| !used.contains(&(*h, m.id.as_str())))
        .map(|(h, m)| ModelRef {
            harness: h,
            model: m.id.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FX: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../so-harness/tests/fixtures");

    #[test]
    fn default_catalog_parses_and_covers_fixtures() {
        let c = Catalog::default_catalog();
        assert!(c.model_count() > 50);
        assert_eq!(c.pool_for(Harness::Agy, "gemini-3.8-flash-low"), "gemini");
        assert_eq!(c.pool_for(Harness::Agy, "claude-sonnet-4-6"), "claude");
        assert_eq!(c.pool_for(Harness::Codex, "gpt-6.1-sol"), "default");
        assert_eq!(
            c.cost(Harness::Opencode, "opencode/deepseek-v4.1-flash-free"),
            Cost::Free
        );
        assert_eq!(c.tier(Harness::Codex, "gpt-6.1-sol"), Tier::High);
        for line in std::fs::read_to_string(format!("{FX}/agy/models.txt"))
            .unwrap()
            .lines()
        {
            if let Some((id, _)) = line.split_once('\t') {
                assert!(c.contains(Harness::Agy, id), "{id}");
            }
        }
    }

    #[test]
    fn load_falls_back_to_default_for_missing_or_empty_file() {
        let d = tempfile::tempdir().unwrap();
        let paths = Paths::from_dir(d.path().to_path_buf());
        assert_eq!(Catalog::load(&paths).unwrap(), Catalog::default_catalog());
        std::fs::write(paths.models_toml(), "# only a comment\n").unwrap();
        assert_eq!(Catalog::load(&paths).unwrap(), Catalog::default_catalog());
        let mut c = Catalog::default();
        c.insert(
            Harness::Codex,
            ModelEntry {
                id: "m".into(),
                tier: Tier::Fast,
                efforts: vec!["low".into()],
                pool: None,
                cost: Cost::Paid,
            },
        );
        c.save(&paths).unwrap();
        assert_eq!(Catalog::load(&paths).unwrap(), c);
    }

    #[test]
    fn parses_collector_outputs() {
        let agy = std::fs::read_to_string(format!("{FX}/agy/models.txt")).unwrap();
        let ids = parse_agy_models(&agy);
        assert_eq!(ids.len(), 6);
        assert!(ids.contains(&"gpt-oss-120b-medium".to_string()));
        let oc = std::fs::read_to_string(format!("{FX}/opencode/models.txt")).unwrap();
        let ids = parse_opencode_models(&oc);
        assert!(ids.contains(&"opencode/deepseek-v4.1-flash-free".to_string()));
        assert!(ids.len() > 50);
        let codex = json!({"id": 2, "result": {"data": [
            {"id": "a", "model": "gpt-x", "hidden": false,
             "supportedReasoningEfforts": [{"reasoningEffort": "low"}, {"reasoningEffort": "high"}]},
            {"id": "b", "model": "hidden-one", "hidden": true}]}})
        .to_string();
        assert_eq!(
            parse_codex_models(&codex),
            vec![(
                "gpt-x".to_string(),
                vec!["low".to_string(), "high".to_string()]
            )]
        );
        assert!(parse_codex_models("garbage").is_empty());
        let dev = json!({"opencode": {"models": {
            "a-free": {"cost": {"input": 0, "output": 0}},
            "b": {"cost": {"input": 1, "output": 2}},
            "c": {}}}});
        assert_eq!(parse_models_dev_free(&dev), vec!["opencode/a-free"]);
    }

    #[test]
    fn reports_unknown_and_unused() {
        let cat = Catalog::default_catalog();
        let mk = |id: &str, h: Harness, model: &str| Choice {
            id: id.into(),
            harness: h,
            model: model.into(),
            effort: None,
            roles: vec!["worker".into()],
            description: String::new(),
            enabled: true,
        };
        let choices = vec![
            mk("ok", Harness::Agy, "gemini-3.8-flash-low"),
            mk("bad", Harness::Agy, "no-such-model"),
        ];
        let unknown = unknown_choice_models(&cat, &choices);
        assert_eq!(unknown.len(), 1);
        assert_eq!(unknown[0].choice, "bad");
        let unused = unused_models(&cat, &choices);
        assert_eq!(unused.len(), cat.model_count() - 1);
        assert!(!unused.iter().any(|m| m.model == "gemini-3.8-flash-low"));
    }

    #[test]
    fn infers_tier_and_cost() {
        assert_eq!(infer_tier("opencode/foo-pro"), Tier::High);
        assert_eq!(infer_tier("gemini-flash"), Tier::Fast);
        assert_eq!(infer_tier("something"), Tier::Mid);
        assert_eq!(infer_cost(Harness::Opencode, "x-free"), Cost::Free);
        assert_eq!(infer_cost(Harness::Opencode, "x"), Cost::Paid);
        assert_eq!(infer_cost(Harness::Codex, "x"), Cost::Included);
    }
}

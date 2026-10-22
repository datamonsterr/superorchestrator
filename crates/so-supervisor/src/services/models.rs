//! The model catalog (`models refresh`, `POST /models/refresh`).

use so_accounts::HostEnv;
use so_core::{AccountFile, Config, Paths};
use so_router::Catalog;
use so_router::catalog::{RefreshEnv, RefreshReport, refresh};
use so_usage::TokioProbeRunner;

/// Refreshes `models.toml` from the harnesses and models.dev. The file is rewritten only
/// when something changed (a rewrite drops comments). Returns the catalog and the report.
pub async fn refresh_and_save(
    paths: &Paths,
    config: &Config,
    host: &HostEnv,
    http: &reqwest::Client,
) -> Result<(Catalog, RefreshReport), String> {
    let accounts = AccountFile::list(paths).map_err(|e| e.to_string())?;
    let env = RefreshEnv {
        paths,
        host,
        accounts: &accounts,
        runner: &TokioProbeRunner,
        http,
        models_dev_url: None,
        now: chrono::Utc::now(),
    };
    let before = Catalog::load(paths).map_err(|e| e.to_string())?;
    let (catalog, report) = refresh(&env, before.clone(), &config.choices).await;
    if catalog != before || !paths.models_toml().is_file() {
        catalog.save(paths).map_err(|e| e.to_string())?;
    }
    Ok((catalog, report))
}

//! The model catalog (`models refresh`, `POST /models/refresh`).

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use so_accounts::HostEnv;
use so_core::{AccountFile, Config, Paths};
use so_router::Catalog;
use so_router::catalog::{RefreshEnv, RefreshReport, refresh};
use so_usage::TokioProbeRunner;

/// Why a refresh failed.
#[derive(Debug, thiserror::Error)]
pub enum ModelsError {
    /// `config.toml` does not load (the caller's input to fix).
    #[error("{0}")]
    Config(String),
    /// Accounts, the catalog or the filesystem.
    #[error("{0}")]
    Io(String),
}

fn stamp_path(paths: &Paths) -> PathBuf {
    paths.config_dir.join("cache/models.refreshed_at")
}

/// When the catalog was last refreshed (written on every refresh, even when nothing changed).
pub fn last_refresh(paths: &Paths) -> Option<DateTime<Utc>> {
    let text = std::fs::read_to_string(stamp_path(paths)).ok()?;
    DateTime::parse_from_rfc3339(text.trim())
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

fn io(e: impl std::fmt::Display) -> ModelsError {
    ModelsError::Io(e.to_string())
}

/// Refreshes `models.toml` from the harnesses and models.dev. The catalog file is
/// rewritten only when something changed (a rewrite drops comments). All disk reads and
/// writes run off the async threads. `models_dev_url` overrides the public models.dev URL.
pub async fn refresh_and_save(
    paths: &Paths,
    host: &HostEnv,
    http: &reqwest::Client,
    models_dev_url: Option<&str>,
) -> Result<(Catalog, RefreshReport, DateTime<Utc>), ModelsError> {
    let p = paths.clone();
    let (config, accounts, before) =
        tokio::task::spawn_blocking(move || -> Result<_, ModelsError> {
            let config = Config::load(&p).map_err(|e| ModelsError::Config(e.to_string()))?;
            let accounts = AccountFile::list(&p).map_err(io)?;
            let before = Catalog::load(&p).map_err(io)?;
            Ok((config, accounts, before))
        })
        .await
        .map_err(io)??;
    let now = Utc::now();
    let env = RefreshEnv {
        pass_env: &config.accounts.pass_env,
        paths,
        host,
        accounts: &accounts,
        runner: &TokioProbeRunner,
        http,
        models_dev_url,
        now,
    };
    let (catalog, report) = refresh(&env, before.clone(), &config.choices).await;
    let (p, saved) = (paths.clone(), catalog.clone());
    tokio::task::spawn_blocking(move || -> Result<(), ModelsError> {
        if saved != before || !p.models_toml().is_file() {
            saved.save(&p).map_err(io)?;
        }
        let stamp = stamp_path(&p);
        if let Some(dir) = stamp.parent() {
            std::fs::create_dir_all(dir).map_err(io)?;
        }
        std::fs::write(&stamp, now.to_rfc3339()).map_err(io)
    })
    .await
    .map_err(io)??;
    Ok((catalog, report, now))
}

#[cfg(test)]
mod tests {
    use super::*;
    use so_core::Platform;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// A one-shot-per-connection HTTP server answering every request with `{}`.
    async fn stub_models_dev() -> String {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = l.accept().await else {
                    return;
                };
                tokio::spawn(async move {
                    let mut buf = [0u8; 2048];
                    let _ = s.read(&mut buf).await;
                    let _ = s
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
                        .await;
                });
            }
        });
        format!("http://{addr}/api.json")
    }

    #[tokio::test]
    async fn refresh_offline_writes_once_and_leaves_an_unchanged_catalog_alone() {
        let d = tempfile::tempdir().unwrap();
        let paths = Paths::from_dir(d.path().join("cfg"));
        let host = HostEnv {
            home: d.path().to_path_buf(),
            path_var: None,
            platform: Platform::Linux,
        };
        let url = stub_models_dev().await;
        let http = reqwest::Client::new();
        assert!(last_refresh(&paths).is_none());

        let (_, report, t1) = refresh_and_save(&paths, &host, &http, Some(&url))
            .await
            .unwrap();
        assert!(!report.sources.is_empty());
        assert!(
            paths.models_toml().is_file(),
            "first refresh creates models.toml"
        );
        assert_eq!(last_refresh(&paths).unwrap().timestamp(), t1.timestamp());

        let before = std::fs::read(paths.models_toml()).unwrap();
        let mtime = std::fs::metadata(paths.models_toml())
            .unwrap()
            .modified()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let (_, _, t2) = refresh_and_save(&paths, &host, &http, Some(&url))
            .await
            .unwrap();
        assert_eq!(std::fs::read(paths.models_toml()).unwrap(), before);
        assert_eq!(
            std::fs::metadata(paths.models_toml())
                .unwrap()
                .modified()
                .unwrap(),
            mtime
        );
        assert!(t2 >= t1 && last_refresh(&paths).unwrap().timestamp() == t2.timestamp());
    }

    #[tokio::test]
    async fn a_broken_config_is_a_config_error() {
        let d = tempfile::tempdir().unwrap();
        let paths = Paths::from_dir(d.path().join("cfg"));
        std::fs::create_dir_all(&paths.config_dir).unwrap();
        std::fs::write(paths.config_toml(), "not = [valid\n").unwrap();
        let host = HostEnv {
            home: d.path().to_path_buf(),
            path_var: None,
            platform: Platform::Linux,
        };
        let r = refresh_and_save(
            &paths,
            &host,
            &reqwest::Client::new(),
            Some("http://127.0.0.1:1/"),
        )
        .await;
        assert!(matches!(r, Err(ModelsError::Config(_))));
    }
}

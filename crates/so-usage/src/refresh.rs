//! Active account refresh shared by the supervisor, CLI and terminal UI.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use so_accounts::{SignIn, sign_in_state};
use so_core::{AccountFile, Harness, Paths};
use tokio::sync::Mutex;

use crate::UsageService;

struct Attempt {
    account: AccountFile,
    at: Instant,
    error: Option<String>,
}

/// Serializes refresh batches and remembers failed attempts for the probe TTL too. A UI
/// polling every second must not start a new process for an unavailable account every second.
pub struct UsageRefresher {
    service: UsageService,
    attempts: Mutex<HashMap<String, Attempt>>,
}

impl UsageRefresher {
    pub fn new(service: UsageService) -> Self {
        Self {
            service,
            attempts: Mutex::new(HashMap::new()),
        }
    }

    /// Refreshes enabled, signed-in accounts with an active probe. Returns safe diagnostics;
    /// failed probes leave existing readings intact. Never launches inference or login.
    pub async fn refresh(&self, paths: &Paths) -> Vec<String> {
        let mut attempts = self.attempts.lock().await;
        let (paths, host) = (paths.clone(), self.service.host.clone());
        let loaded = tokio::task::spawn_blocking(move || {
            let mut accounts = Vec::new();
            let mut errors = Vec::new();
            let entries = match std::fs::read_dir(paths.accounts_dir()) {
                Ok(entries) => entries,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return (accounts, errors);
                }
                Err(e) => {
                    return (
                        accounts,
                        vec![format!("could not read usage accounts: {e}")],
                    );
                }
            };
            for entry in entries {
                let path = match entry {
                    Ok(entry) => entry.path(),
                    Err(e) => {
                        errors.push(format!("could not read usage account: {e}"));
                        continue;
                    }
                };
                if path.extension().is_none_or(|ext| ext != "toml") {
                    continue;
                }
                match AccountFile::load(&path) {
                    Ok(account)
                        if account.enabled
                            && matches!(account.harness, Harness::Codex | Harness::Agy) =>
                    {
                        match sign_in_state(&account, &host) {
                            Ok(SignIn::SignedIn) => accounts.push(account),
                            Ok(_) => {}
                            Err(e) => errors.push(format!("usage for {}: {e}", account.name)),
                        }
                    }
                    Ok(_) => {}
                    Err(e) => {
                        // TOML error continuation lines can quote account-file contents.
                        errors.push(format!(
                            "could not load usage account: {}",
                            e.to_string().lines().next().unwrap_or_default()
                        ));
                    }
                }
            }
            accounts.sort_by(|a, b| a.name.cmp(&b.name));
            (accounts, errors)
        })
        .await;
        let (accounts, mut errors) = match loaded {
            Ok(loaded) => loaded,
            Err(_) => return vec!["could not load usage accounts".into()],
        };
        attempts.retain(|name, _| accounts.iter().any(|a| &a.name == name));
        let ttl = self
            .service
            .config
            .usage
            .probe_ttl
            .as_duration()
            .max(Duration::from_secs(1));
        let due: Vec<_> = accounts
            .into_iter()
            .filter(|a| {
                !attempts.get(&a.name).is_some_and(|last| {
                    last.error.is_some() && last.account == *a && last.at.elapsed() < ttl
                })
            })
            .collect();
        for (name, result) in self.service.refresh_all(&due, false).await {
            if let Some(account) = due.iter().find(|a| a.name == name) {
                attempts.insert(
                    name,
                    Attempt {
                        account: account.clone(),
                        at: Instant::now(),
                        error: result.err().map(|e| e.to_string()),
                    },
                );
            }
        }
        errors.extend(attempts.iter().filter_map(|(name, last)| {
            last.error
                .as_ref()
                .map(|e| format!("usage for {name}: {e}"))
        }));
        errors.sort();
        errors
    }
}

/// Production refresher with the same environment boundary as supervised runs.
pub fn active_refresher(
    store: so_store::Store,
    config: so_core::Config,
    host: so_accounts::HostEnv,
) -> Arc<UsageRefresher> {
    Arc::new(UsageRefresher::new(UsageService::new(
        store,
        config,
        host,
        Arc::new(crate::TokioProbeRunner),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BoxFuture, ProbeOutput, ProbeRequest, ProbeRunner, Result};
    use chrono::Utc;
    use so_accounts::HostEnv;
    use so_core::{Config, HeadroomSource, Platform};
    use so_store::Store;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Runner {
        calls: AtomicUsize,
        fail: bool,
    }

    impl ProbeRunner for Runner {
        fn run<'a>(&'a self, req: ProbeRequest) -> BoxFuture<'a, Result<ProbeOutput>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                tokio::task::yield_now().await;
                Ok(ProbeOutput::ok(if self.fail {
                    "sign-in URL and secret must not reach diagnostics"
                } else if req.args.iter().any(|a| a == "app-server") {
                    r#"{"result":{"rateLimits":{"primary":{"usedPercent":28,"windowDurationMins":300},"secondary":{"usedPercent":41,"windowDurationMins":10080}}}}"#
                } else {
                    "Gemini Models\tFive Hour Limit Remaining\t82%\t\nClaude and GPT models\tWeekly Limit Remaining\t0%\t\n"
                }))
            })
        }
    }

    fn account(
        paths: &Paths,
        host: &HostEnv,
        name: &str,
        harness: Harness,
        enabled: bool,
        signed_in: bool,
    ) -> AccountFile {
        let home = host.home.join(name);
        std::fs::create_dir_all(home.join(".gemini")).unwrap();
        if signed_in {
            let auth = if harness == Harness::Agy {
                ".gemini/oauth_creds.json"
            } else {
                "auth.json"
            };
            std::fs::write(home.join(auth), "unread credential").unwrap();
        }
        let mut a = AccountFile::new(name, harness, home.to_str().unwrap(), "/bin/true").unwrap();
        a.enabled = enabled;
        a.save(paths).unwrap();
        a
    }

    fn setup(fail: bool) -> (tempfile::TempDir, Paths, UsageRefresher, Arc<Runner>) {
        let d = tempfile::tempdir().unwrap();
        let paths = Paths::from_dir(d.path().join("cfg"));
        let runner = Arc::new(Runner {
            calls: AtomicUsize::new(0),
            fail,
        });
        let service = UsageService::new(
            Store::open_in_memory().unwrap(),
            Config::default(),
            HostEnv {
                home: d.path().join("home"),
                path_var: None,
                platform: Platform::Linux,
            },
            runner.clone(),
        );
        (d, paths, UsageRefresher::new(service), runner)
    }

    #[tokio::test]
    async fn refreshes_only_active_signed_in_probe_accounts_and_shares_ttl() {
        let (_d, paths, refresh, runner) = setup(false);
        let host = &refresh.service.host;
        account(&paths, host, "codex-1", Harness::Codex, true, true);
        account(&paths, host, "agy-1", Harness::Agy, true, true);
        account(&paths, host, "codex-off", Harness::Codex, false, true);
        account(&paths, host, "codex-out", Harness::Codex, true, false);
        account(&paths, host, "claude-1", Harness::Claude, true, true);
        account(&paths, host, "opencode-1", Harness::Opencode, true, true);
        let (one, two) = tokio::join!(refresh.refresh(&paths), refresh.refresh(&paths));
        assert!(one.is_empty() && two.is_empty());
        assert_eq!(runner.calls.load(Ordering::SeqCst), 2);
        let s = &refresh.service.store;
        let codex = s.get_headroom("codex-1", "default").unwrap().unwrap();
        assert_eq!(
            (codex.five_hour_remaining_pct, codex.weekly_remaining_pct),
            (Some(72.0), Some(59.0))
        );
        assert_eq!(
            s.get_headroom("agy-1", "gemini")
                .unwrap()
                .unwrap()
                .five_hour_remaining_pct,
            Some(82.0)
        );
        assert_eq!(
            s.get_headroom("agy-1", "claude")
                .unwrap()
                .unwrap()
                .weekly_remaining_pct,
            Some(0.0)
        );
        assert_eq!(s.list_headroom(100).unwrap().len(), 3);
        // A new refresher (another CLI process) respects the shared store's TTL too.
        assert!(
            UsageRefresher::new(refresh.service.clone())
                .refresh(&paths)
                .await
                .is_empty()
        );
        assert_eq!(runner.calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn failures_preserve_readings_back_off_and_redact_output() {
        let (_d, paths, refresh, runner) = setup(true);
        account(
            &paths,
            &refresh.service.host,
            "agy-1",
            Harness::Agy,
            true,
            true,
        );
        let mut old = crate::merge_into_record(
            "agy-1",
            "gemini",
            &[crate::WindowReading {
                pool: "gemini".into(),
                window: crate::Window::FiveHour,
                remaining_pct: Some(23.0),
                resets_at: None,
            }],
            15.0,
            HeadroomSource::Probe,
            chrono::DateTime::from_timestamp(Utc::now().timestamp() - 3600, 0).unwrap(),
        );
        refresh.service.store.upsert_headroom(&old).unwrap();
        let errors = refresh.refresh(&paths).await;
        assert_eq!(
            errors,
            vec!["usage for agy-1: unexpected probe output: agy output has no usage rows"]
        );
        assert_eq!(refresh.refresh(&paths).await, errors);
        assert_eq!(runner.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            refresh
                .service
                .store
                .get_headroom("agy-1", "gemini")
                .unwrap()
                .unwrap(),
            old
        );
        // An expired failure is retried.
        refresh.attempts.lock().await.get_mut("agy-1").unwrap().at -= Duration::from_secs(301);
        refresh.refresh(&paths).await;
        assert_eq!(runner.calls.load(Ordering::SeqCst), 2);
        // Ledger placeholders never suppress an active measurement.
        old.source = HeadroomSource::Ledger;
        old.observed_at = Utc::now();
        refresh.service.store.upsert_headroom(&old).unwrap();
        assert!(
            refresh
                .service
                .refresh(
                    &AccountFile::load(&paths.account_file("agy-1")).unwrap(),
                    false
                )
                .await
                .is_err()
        );
        assert_eq!(runner.calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn malformed_account_does_not_prevent_other_accounts_refreshing() {
        let (_d, paths, refresh, runner) = setup(false);
        account(
            &paths,
            &refresh.service.host,
            "codex-1",
            Harness::Codex,
            true,
            true,
        );
        std::fs::write(
            paths.accounts_dir().join("broken.toml"),
            "name = 'SECRET'\nharness = [\n",
        )
        .unwrap();
        let errors = refresh.refresh(&paths).await;
        assert_eq!(errors.len(), 1);
        assert!(!errors[0].contains("SECRET"));
        assert_eq!(runner.calls.load(Ordering::SeqCst), 1);
    }
}

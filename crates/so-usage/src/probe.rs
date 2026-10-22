//! Active probes (SPEC 6.1): codex `account/rateLimits/read` over app-server stdio and
//! `agy --print /usage`. Process execution sits behind [`ProbeRunner`] so tests inject output.
//! Nothing read from auth files is stored, and agy stdout (which may carry sign-in URLs) is
//! never logged.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use so_accounts::{HostEnv, LaunchMode, build_launch, resolve_binary};
use so_core::{AccountFile, Config, Harness, Headroom, HeadroomSource, HeadroomStatus};
use so_store::Store;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

use crate::error::{Result, UsageError};
use crate::headroom::merge_into_record;
use crate::ledger::ledger_headroom;
use crate::parse::{WindowReading, parse_agy_usage, parse_codex_rate_limits};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

const CODEX_TIMEOUT: Duration = Duration::from_secs(15);
const AGY_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_PARALLEL: usize = 4;

/// When a run of a probe command is finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseMatcher {
    /// Read stdout to EOF and return it all (agy).
    Eof,
    /// JSON-RPC: after writing each stdin line that has an `id`, wait for the response with
    /// that id; return the last response line, then stop the process (codex).
    JsonRpcResponses,
}

#[derive(Debug, Clone)]
pub struct ProbeRequest {
    pub program: PathBuf,
    pub args: Vec<String>,
    /// Added to the inherited environment.
    pub env: Vec<(String, String)>,
    pub stdin_lines: Vec<String>,
    pub wait_for: ResponseMatcher,
    pub timeout: Duration,
}

pub trait ProbeRunner: Send + Sync {
    fn run<'a>(&'a self, req: ProbeRequest) -> BoxFuture<'a, Result<String>>;
}

/// Real runner on `tokio::process`; the child is killed on drop.
pub struct TokioProbeRunner;

impl ProbeRunner for TokioProbeRunner {
    fn run<'a>(&'a self, req: ProbeRequest) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            let secs = req.timeout.as_secs();
            tokio::time::timeout(req.timeout, run_process(&req))
                .await
                .map_err(|_| UsageError::Timeout(secs))?
        })
    }
}

fn rpc_id(line: &str) -> Option<Value> {
    let v: Value = serde_json::from_str(line).ok()?;
    v.get("id").filter(|i| !i.is_null()).cloned()
}

async fn run_process(req: &ProbeRequest) -> Result<String> {
    let mut child = Command::new(&req.program)
        .args(&req.args)
        .envs(req.env.iter().map(|(k, v)| (k, v)))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| UsageError::Probe(format!("spawn {}: {}", req.program.display(), e.kind())))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| UsageError::Probe("no stdin".into()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| UsageError::Probe("no stdout".into()))?;
    match req.wait_for {
        ResponseMatcher::Eof => {
            for l in &req.stdin_lines {
                stdin.write_all(format!("{l}\n").as_bytes()).await?;
            }
            drop(stdin);
            let mut out = String::new();
            let mut r = stdout;
            r.read_to_string(&mut out).await?;
            let _ = child.wait().await;
            Ok(out)
        }
        ResponseMatcher::JsonRpcResponses => {
            let mut lines = BufReader::new(stdout).lines();
            let mut last = None;
            for l in &req.stdin_lines {
                stdin.write_all(format!("{l}\n").as_bytes()).await?;
                stdin.flush().await?;
                let Some(id) = rpc_id(l) else { continue };
                loop {
                    let Some(resp) = lines.next_line().await? else {
                        return Err(UsageError::Probe("process exited before responding".into()));
                    };
                    let Ok(v) = serde_json::from_str::<Value>(&resp) else {
                        continue;
                    };
                    if v.get("id") == Some(&id) && v.get("method").is_none() {
                        last = Some(resp);
                        break;
                    }
                }
            }
            let _ = child.kill().await;
            last.ok_or_else(|| UsageError::Probe("no request sent".into()))
        }
    }
}

/// Headroom refresh for accounts. Cheap to clone.
#[derive(Clone)]
pub struct UsageService {
    pub store: Store,
    pub config: Config,
    pub host: HostEnv,
    pub runner: Arc<dyn ProbeRunner>,
}

impl UsageService {
    pub fn new(store: Store, config: Config, host: HostEnv, runner: Arc<dyn ProbeRunner>) -> Self {
        Self {
            store,
            config,
            host,
            runner,
        }
    }

    pub async fn refresh(&self, account: &AccountFile, force: bool) -> Result<Vec<Headroom>> {
        self.refresh_at(account, force, Utc::now()).await
    }

    /// Like [`refresh`](Self::refresh) with an injected clock. Skips the probe when every pool
    /// of the account was observed within `usage.probe_ttl`, unless `force`.
    pub async fn refresh_at(
        &self,
        account: &AccountFile,
        force: bool,
        now: DateTime<Utc>,
    ) -> Result<Vec<Headroom>> {
        let ttl = chrono::Duration::from_std(self.config.usage.probe_ttl.as_duration())
            .unwrap_or_else(|_| chrono::Duration::minutes(5));
        let existing: Vec<Option<Headroom>> = account
            .pools
            .iter()
            .map(|p| self.store.get_headroom(&account.name, p))
            .collect::<std::result::Result<_, _>>()?;
        let fresh = !existing.is_empty()
            && existing
                .iter()
                .all(|h| h.as_ref().is_some_and(|h| now - h.observed_at < ttl));
        let probes = matches!(account.harness, Harness::Codex | Harness::Agy);
        if !force && fresh && probes {
            return Ok(existing.into_iter().flatten().collect());
        }
        match account.harness {
            Harness::Codex => {
                let readings = self.probe_codex(account).await?;
                self.store_readings(account, &readings, HeadroomSource::Probe, now)
            }
            Harness::Agy => match self.probe_agy(account).await {
                Ok(readings) => self.store_readings(account, &readings, HeadroomSource::Probe, now),
                Err(UsageError::NotEligible) => {
                    self.store_readings(account, &[], HeadroomSource::Probe, now)?;
                    Err(UsageError::NotEligible)
                }
                Err(e) => Err(e),
            },
            Harness::Opencode | Harness::Claude => {
                let mut out = Vec::new();
                for (pool, prior) in account.pools.iter().zip(existing) {
                    // A passive or probed record is better than ledger guesses.
                    if let Some(h) = prior.filter(|h| h.source != HeadroomSource::Ledger) {
                        out.push(h);
                        continue;
                    }
                    let h = ledger_headroom(&self.store, &account.name, pool, now)?;
                    self.store.upsert_headroom(&h)?;
                    out.push(h);
                }
                Ok(out)
            }
        }
    }

    /// Records passive readings (codex `account/rateLimits/updated`, claude `rate_limit_event`).
    pub fn ingest_event(
        &self,
        account: &AccountFile,
        readings: &[WindowReading],
        now: DateTime<Utc>,
    ) -> Result<Vec<Headroom>> {
        self.store_readings(account, readings, HeadroomSource::Event, now)
    }

    /// Refreshes all accounts, at most 4 at a time. Results are in input order.
    pub async fn refresh_all(
        &self,
        accounts: &[AccountFile],
        force: bool,
    ) -> Vec<(String, Result<Vec<Headroom>>)> {
        let sem = Arc::new(tokio::sync::Semaphore::new(MAX_PARALLEL));
        let mut set = tokio::task::JoinSet::new();
        for (i, a) in accounts.iter().cloned().enumerate() {
            let (svc, sem) = (self.clone(), sem.clone());
            set.spawn(async move {
                let _permit = sem.acquire_owned().await;
                let r = svc.refresh(&a, force).await;
                (i, a.name, r)
            });
        }
        let mut out: Vec<Option<(String, Result<Vec<Headroom>>)>> =
            accounts.iter().map(|_| None).collect();
        while let Some(joined) = set.join_next().await {
            match joined {
                Ok((i, name, r)) => out[i] = Some((name, r)),
                Err(e) => tracing::warn!("usage refresh task failed: {e}"),
            }
        }
        out.into_iter()
            .zip(accounts)
            .map(|(r, a)| {
                r.unwrap_or_else(|| (a.name.clone(), Err(UsageError::Probe("task failed".into()))))
            })
            .collect()
    }

    fn store_readings(
        &self,
        account: &AccountFile,
        readings: &[WindowReading],
        source: HeadroomSource,
        now: DateTime<Utc>,
    ) -> Result<Vec<Headroom>> {
        let low = f64::from(self.config.usage.low_pct);
        let mut out = Vec::new();
        for pool in &account.pools {
            let mut h = merge_into_record(&account.name, pool, readings, low, source, now);
            if self
                .store
                .get_flag(&account.name, pool)?
                .is_some_and(|f| f.until > now)
            {
                h.status = HeadroomStatus::Exhausted;
            }
            self.store.upsert_headroom(&h)?;
            out.push(h);
        }
        Ok(out)
    }

    async fn probe_codex(&self, account: &AccountFile) -> Result<Vec<WindowReading>> {
        let launch = build_launch(account, &self.host, None, LaunchMode::Supervised)?;
        let mut args = launch.argv_prefix.clone();
        args.extend(launch.extra_args.clone());
        args.extend(["app-server".into(), "--listen".into(), "stdio://".into()]);
        let req = ProbeRequest {
            program: launch.program,
            args,
            env: launch.vars,
            stdin_lines: codex_probe_lines(),
            wait_for: ResponseMatcher::JsonRpcResponses,
            timeout: CODEX_TIMEOUT,
        };
        let resp = self.runner.run(req).await?;
        let v: Value = serde_json::from_str(resp.trim())
            .map_err(|_| UsageError::Parse("codex response is not JSON".into()))?;
        if v.get("error").is_some() {
            return Err(UsageError::Probe("codex returned a JSON-RPC error".into()));
        }
        Ok(parse_codex_rate_limits(&v))
    }

    /// agy needs no bwrap for `/usage` (ADR-0005): `--gemini_dir` selects the account.
    async fn probe_agy(&self, account: &AccountFile) -> Result<Vec<WindowReading>> {
        let program = resolve_binary(&account.binary, self.host.path_var.as_deref())?;
        let home = self.host.expand(&account.home)?;
        let req = ProbeRequest {
            program,
            args: vec![
                format!("--gemini_dir={}", home.join(".gemini").display()),
                "--print".into(),
                "/usage".into(),
            ],
            env: Vec::new(),
            stdin_lines: Vec::new(),
            wait_for: ResponseMatcher::Eof,
            timeout: AGY_TIMEOUT,
        };
        let out = self.runner.run(req).await?;
        parse_agy_usage(&out)
    }
}

/// The codex probe conversation: `initialize`, `initialized`, `account/rateLimits/read`.
pub fn codex_probe_lines() -> Vec<String> {
    vec![
        json!({"id": 1, "method": "initialize", "params": {"clientInfo": {
            "name": "superorchestrator", "title": null, "version": env!("CARGO_PKG_VERSION")}}})
        .to_string(),
        json!({"method": "initialized"}).to_string(),
        json!({"id": 2, "method": "account/rateLimits/read"}).to_string(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::Window;
    use chrono::Duration as CDur;
    use std::sync::Mutex;

    const FX: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../so-harness/tests/fixtures");

    struct Fake {
        reply: Mutex<Result<String>>,
        calls: Mutex<Vec<ProbeRequest>>,
    }
    impl Fake {
        fn new(reply: Result<String>) -> Arc<Self> {
            Arc::new(Self {
                reply: Mutex::new(reply),
                calls: Mutex::new(vec![]),
            })
        }
        fn n(&self) -> usize {
            self.calls.lock().unwrap().len()
        }
    }
    impl ProbeRunner for Fake {
        fn run<'a>(&'a self, req: ProbeRequest) -> BoxFuture<'a, Result<String>> {
            self.calls.lock().unwrap().push(req);
            let r = match &*self.reply.lock().unwrap() {
                Ok(s) => Ok(s.clone()),
                Err(UsageError::NotEligible) => Err(UsageError::NotEligible),
                Err(e) => Err(UsageError::Probe(e.to_string())),
            };
            Box::pin(async move { r })
        }
    }

    fn setup(
        fake: Arc<Fake>,
        harness: Harness,
        bin: &str,
    ) -> (tempfile::TempDir, UsageService, AccountFile) {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let bindir = d.path().join("bin");
        std::fs::create_dir_all(&bindir).unwrap();
        let p = bindir.join(bin);
        std::fs::write(&p, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        let host = HostEnv {
            home: d.path().to_path_buf(),
            path_var: Some(bindir.to_string_lossy().into_owned()),
            platform: so_core::Platform::Linux,
        };
        let svc = UsageService::new(
            Store::open_in_memory().unwrap(),
            Config::default(),
            host,
            fake,
        );
        let home = format!("~/.{bin}-acc");
        let acct = AccountFile::new(&format!("{bin}-acc"), harness, &home, bin).unwrap();
        (d, svc, acct)
    }

    fn codex_reply() -> Result<String> {
        let v: Value = serde_json::from_str(
            &std::fs::read_to_string(format!("{FX}/codex/rate-limits-read.response.json")).unwrap(),
        )
        .unwrap();
        Ok(v.to_string())
    }

    #[tokio::test]
    async fn codex_ttl_force_and_request_lines() {
        let fake = Fake::new(codex_reply());
        let (d, svc, acct) = setup(fake.clone(), Harness::Codex, "codex");
        let now = DateTime::from_timestamp(Utc::now().timestamp(), 0).unwrap();
        let h = svc.refresh_at(&acct, false, now).await.unwrap();
        assert_eq!(fake.n(), 1);
        assert_eq!(h[0].source, HeadroomSource::Probe);
        assert_eq!(h[0].five_hour_remaining_pct, Some(79.0));
        assert_eq!(h[0].status, HeadroomStatus::Ok);
        assert_eq!(
            svc.store
                .get_headroom("codex-acc", "default")
                .unwrap()
                .unwrap(),
            h[0]
        );
        // Within TTL: skipped.
        svc.refresh_at(&acct, false, now + CDur::minutes(4))
            .await
            .unwrap();
        assert_eq!(fake.n(), 1);
        // Force bypasses.
        svc.refresh_at(&acct, true, now + CDur::minutes(4))
            .await
            .unwrap();
        assert_eq!(fake.n(), 2);
        // After TTL.
        svc.refresh_at(&acct, false, now + CDur::minutes(10))
            .await
            .unwrap();
        assert_eq!(fake.n(), 3);

        let call = fake.calls.lock().unwrap()[0].clone();
        assert_eq!(call.args, ["app-server", "--listen", "stdio://"]);
        assert_eq!(
            call.env,
            vec![(
                "CODEX_HOME".to_string(),
                d.path().join(".codex-acc").display().to_string()
            )]
        );
        assert_eq!(call.wait_for, ResponseMatcher::JsonRpcResponses);
        let msgs: Vec<Value> = call
            .stdin_lines
            .iter()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(msgs[0]["method"], "initialize");
        assert_eq!(msgs[0]["id"], 1);
        assert!(msgs[0]["params"]["clientInfo"]["name"].is_string());
        assert_eq!(msgs[1]["method"], "initialized");
        assert!(msgs[1].get("id").is_none());
        assert_eq!(msgs[2]["method"], "account/rateLimits/read");
        assert_eq!(msgs[2]["id"], 2);
    }

    #[tokio::test]
    async fn flag_forces_exhausted_on_probe() {
        let fake = Fake::new(codex_reply());
        let (_d, svc, acct) = setup(fake, Harness::Codex, "codex");
        let now = Utc::now();
        crate::exhaustion::mark_exhausted(
            &svc.store,
            "codex-acc",
            "default",
            None,
            Duration::from_secs(3600),
            "q",
            now,
        )
        .unwrap();
        let h = svc.refresh_at(&acct, true, now).await.unwrap();
        assert_eq!(h[0].status, HeadroomStatus::Exhausted);
    }

    #[tokio::test]
    async fn agy_probe_per_pool_and_not_eligible() {
        let ok = std::fs::read_to_string(format!("{FX}/agy/usage-exhausted.txt")).unwrap();
        let fake = Fake::new(Ok(ok));
        let (d, svc, acct) = setup(fake.clone(), Harness::Agy, "agy");
        let h = svc.refresh_at(&acct, false, Utc::now()).await.unwrap();
        let call = fake.calls.lock().unwrap()[0].clone();
        assert_eq!(
            call.args,
            [
                format!("--gemini_dir={}/.agy-acc/.gemini", d.path().display()),
                "--print".into(),
                "/usage".into()
            ]
        );
        assert_eq!(call.wait_for, ResponseMatcher::Eof);
        assert_eq!(h.len(), 2);
        assert_eq!(
            (h[0].pool.as_str(), h[0].status),
            ("gemini", HeadroomStatus::Exhausted)
        );
        assert_eq!(
            (h[1].pool.as_str(), h[1].status),
            ("claude", HeadroomStatus::Low)
        );

        let fake = Fake::new(Err(UsageError::NotEligible));
        let (_d, svc, acct) = setup(fake, Harness::Agy, "agy");
        let r = svc.refresh_at(&acct, false, Utc::now()).await;
        assert!(matches!(r, Err(UsageError::NotEligible)));
        let h = svc
            .store
            .get_headroom("agy-acc", "gemini")
            .unwrap()
            .unwrap();
        assert_eq!(h.status, HeadroomStatus::Unknown);
    }

    #[tokio::test]
    async fn agy_output_with_eligibility_text_is_not_eligible() {
        let fake = Fake::new(Ok(
            "error: Eligibility check failed: nope\nhttps://secret\n".into(),
        ));
        let (_d, svc, acct) = setup(fake, Harness::Agy, "agy");
        assert!(matches!(
            svc.refresh_at(&acct, false, Utc::now()).await,
            Err(UsageError::NotEligible)
        ));
    }

    #[tokio::test]
    async fn claude_uses_ledger_and_keeps_event_records() {
        let fake = Fake::new(Ok(String::new()));
        let (_d, svc, acct) = setup(fake.clone(), Harness::Claude, "claude");
        let now = Utc::now();
        let h = svc.refresh_at(&acct, false, now).await.unwrap();
        assert_eq!(
            (h[0].source, h[0].status),
            (HeadroomSource::Ledger, HeadroomStatus::Unknown)
        );
        assert_eq!(fake.n(), 0);
        let ev = vec![WindowReading {
            pool: "default".into(),
            window: Window::Weekly,
            remaining_pct: Some(5.0),
            resets_at: None,
        }];
        let h = svc.ingest_event(&acct, &ev, now).unwrap();
        assert_eq!(
            (h[0].source, h[0].status),
            (HeadroomSource::Event, HeadroomStatus::Low)
        );
        let h = svc.refresh_at(&acct, false, now).await.unwrap();
        assert_eq!(h[0].source, HeadroomSource::Event);
    }

    #[tokio::test]
    async fn refresh_all_runs_every_account() {
        let fake = Fake::new(codex_reply());
        let (_d, svc, acct) = setup(fake.clone(), Harness::Codex, "codex");
        let mut accts = vec![];
        for i in 0..6 {
            let mut a = acct.clone();
            a.name = format!("codex-acc{i}");
            accts.push(a);
        }
        let res = svc.refresh_all(&accts, true).await;
        assert_eq!(res.len(), 6);
        assert!(res.iter().all(|(_, r)| r.is_ok()));
        assert_eq!(res[3].0, "codex-acc3");
        assert_eq!(fake.n(), 6);
    }

    #[tokio::test]
    async fn real_runner_jsonrpc_and_eof() {
        // A shell stand-in for app-server proves line-by-line request/response handling.
        let script = r#"read a; echo '{"id":1,"result":{}}'; read b; read c; echo '{"id":2,"result":{"ok":true}}'"#;
        let req = ProbeRequest {
            program: "sh".into(),
            args: vec!["-c".into(), script.into()],
            env: vec![],
            stdin_lines: codex_probe_lines(),
            wait_for: ResponseMatcher::JsonRpcResponses,
            timeout: Duration::from_secs(5),
        };
        let out = TokioProbeRunner.run(req).await.unwrap();
        assert!(out.contains("\"ok\":true"));
        let req = ProbeRequest {
            program: "sh".into(),
            args: vec!["-c".into(), "echo hi".into()],
            env: vec![],
            stdin_lines: vec![],
            wait_for: ResponseMatcher::Eof,
            timeout: Duration::from_secs(5),
        };
        assert_eq!(TokioProbeRunner.run(req).await.unwrap().trim(), "hi");
        let req = ProbeRequest {
            program: "sleep".into(),
            args: vec!["5".into()],
            env: vec![],
            stdin_lines: vec![],
            wait_for: ResponseMatcher::Eof,
            timeout: Duration::from_millis(200),
        };
        assert!(matches!(
            TokioProbeRunner.run(req).await,
            Err(UsageError::Timeout(_))
        ));
    }

    /// Manual: runs the real probes for every enabled codex/agy login on this machine.
    #[tokio::test]
    #[ignore = "uses the real codex/agy binaries and logins"]
    async fn real_probe_manual() {
        let host = HostEnv::detect().unwrap();
        let svc = UsageService::new(
            Store::open_in_memory().unwrap(),
            Config::default(),
            host.clone(),
            Arc::new(TokioProbeRunner),
        );
        let codex = AccountFile::new("codex", Harness::Codex, "~/.codex", "codex").unwrap();
        let agy = AccountFile::new("agy", Harness::Agy, "~", "agy").unwrap();
        for a in [codex, agy] {
            println!("{}: {:?}", a.name, svc.refresh(&a, true).await);
        }
    }
}

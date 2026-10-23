//! Active probes (SPEC 6.1): codex `account/rateLimits/read` over app-server stdio and
//! `agy --print /usage`. Process execution sits behind [`ProbeRunner`] so tests inject output.
//! Nothing read from auth files is stored, and agy stdout (which may carry sign-in URLs) is
//! never logged.
//!
//! `Store` methods block on SQLite, so every store access here runs through [`blocking`]
//! (`spawn_blocking`).

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use so_accounts::{HostEnv, build_launch, resolve_binary};
use so_core::{AccountFile, Config, Harness, Headroom, HeadroomSource};
use so_store::{FlagRow, Store};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

use crate::error::{Result, UsageError};
use crate::headroom::{effective_with, merge_into_record, merge_partial};
use crate::ledger::ledger_headroom;
use crate::parse::{WindowReading, parse_agy_usage, parse_codex_rate_limits};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

const CODEX_TIMEOUT: Duration = Duration::from_secs(15);
const AGY_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_PARALLEL: usize = 4;
const MAX_STDOUT: u64 = 1 << 20;
const NOT_ELIGIBLE: &str = "not_eligible";
const NOT_ELIGIBLE_HOLD: chrono::Duration = chrono::Duration::hours(1);

/// Runs `f` with the store on the blocking pool.
pub async fn blocking<T, F>(store: &Store, f: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce(&Store) -> Result<T> + Send + 'static,
{
    let store = store.clone();
    tokio::task::spawn_blocking(move || f(&store))
        .await
        .map_err(|e| UsageError::Probe(format!("store task failed: {e}")))?
}

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
    /// Additional inherited names from the account and global settings.
    pub pass_env: Vec<String>,
    pub program: PathBuf,
    pub args: Vec<String>,
    /// Explicit variables added on top of the isolated environment.
    pub env: Vec<(String, String)>,
    pub stdin_lines: Vec<String>,
    pub wait_for: ResponseMatcher,
    pub timeout: Duration,
}

/// Captured output (at most 1 MiB) and whether the process ended successfully (always true
/// for a JSON-RPC probe that got its response).
#[derive(Debug, Clone)]
pub struct ProbeOutput {
    pub stdout: String,
    pub success: bool,
}

impl ProbeOutput {
    pub fn ok(stdout: impl Into<String>) -> Self {
        Self {
            stdout: stdout.into(),
            success: true,
        }
    }
}

pub trait ProbeRunner: Send + Sync {
    fn run<'a>(&'a self, req: ProbeRequest) -> BoxFuture<'a, Result<ProbeOutput>>;
}

/// Real runner on `tokio::process`. The child leads its own process group, which is killed on
/// timeout; the child is also killed on drop.
pub struct TokioProbeRunner;

impl ProbeRunner for TokioProbeRunner {
    fn run<'a>(&'a self, req: ProbeRequest) -> BoxFuture<'a, Result<ProbeOutput>> {
        Box::pin(run_process(req))
    }
}

fn kill_group(pid: Option<u32>) {
    if let Some(pid) = pid
        .and_then(|p| i32::try_from(p).ok())
        .and_then(rustix::process::Pid::from_raw)
    {
        let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
    }
}

fn rpc_id(line: &str) -> Option<Value> {
    let v: Value = serde_json::from_str(line).ok()?;
    v.get("id").filter(|i| !i.is_null()).cloned()
}

async fn run_process(req: ProbeRequest) -> Result<ProbeOutput> {
    let mut command = Command::new(&req.program);
    so_core::child_env::isolate(command.as_std_mut(), &req.pass_env);
    let mut child = command
        .args(&req.args)
        .envs(req.env.iter().map(|(k, v)| (k, v)))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| UsageError::Probe(format!("spawn {}: {}", req.program.display(), e.kind())))?;
    let pid = child.id();
    let secs = req.timeout.as_secs();
    let io = drive(&mut child, &req);
    match tokio::time::timeout(req.timeout, io).await {
        Ok(r) => {
            if req.wait_for == ResponseMatcher::JsonRpcResponses || r.is_err() {
                kill_group(pid);
                let _ = child.wait().await;
            }
            r
        }
        Err(_) => {
            kill_group(pid);
            let _ = child.wait().await;
            Err(UsageError::Timeout(secs))
        }
    }
}

async fn drive(child: &mut tokio::process::Child, req: &ProbeRequest) -> Result<ProbeOutput> {
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| UsageError::Probe("no stdin".into()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| UsageError::Probe("no stdout".into()))?;
    let capped = stdout.take(MAX_STDOUT + 1);
    match req.wait_for {
        ResponseMatcher::Eof => {
            for l in &req.stdin_lines {
                stdin.write_all(format!("{l}\n").as_bytes()).await?;
            }
            drop(stdin);
            let mut buf = Vec::new();
            let mut r = capped;
            r.read_to_end(&mut buf).await?;
            if buf.len() as u64 > MAX_STDOUT {
                // Runaway output: stop the whole group before waiting, and do not parse it.
                kill_group(child.id());
                let _ = child.wait().await;
                return Err(UsageError::Probe("output too large".into()));
            }
            let success = child.wait().await?.success();
            Ok(ProbeOutput {
                stdout: String::from_utf8_lossy(&buf).into_owned(),
                success,
            })
        }
        ResponseMatcher::JsonRpcResponses => {
            let mut lines = BufReader::new(capped).lines();
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
            last.map(ProbeOutput::ok)
                .ok_or_else(|| UsageError::Probe("no request sent".into()))
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

    fn low_pct(&self) -> f64 {
        f64::from(self.config.usage.low_pct)
    }

    pub async fn refresh(&self, account: &AccountFile, force: bool) -> Result<Vec<Headroom>> {
        self.refresh_at(account, force, Utc::now()).await
    }

    /// Like [`refresh`](Self::refresh) with an injected clock. Skips the probe when every pool
    /// of the account was observed within `usage.probe_ttl`, unless `force`. A recent
    /// not-eligible result is returned as an error without probing again.
    pub async fn refresh_at(
        &self,
        account: &AccountFile,
        force: bool,
        now: DateTime<Utc>,
    ) -> Result<Vec<Headroom>> {
        let ttl = chrono::Duration::from_std(self.config.usage.probe_ttl.as_duration())
            .unwrap_or_else(|_| chrono::Duration::minutes(5));
        let low = self.low_pct();
        let probes = matches!(account.harness, Harness::Codex | Harness::Agy);
        let acct = account.clone();
        let (existing, flags) = blocking(&self.store, move |s| {
            let mut ex = Vec::new();
            let mut fl = Vec::new();
            for p in &acct.pools {
                ex.push(s.get_headroom(&acct.name, p)?);
                fl.push(s.get_flag(&acct.name, p)?.filter(|f| f.until > now));
            }
            Ok((ex, fl))
        })
        .await?;
        if probes && !force {
            let recent_ineligible = flags
                .iter()
                .flatten()
                .any(|f| f.reason == NOT_ELIGIBLE && now < f.until - NOT_ELIGIBLE_HOLD + ttl);
            if recent_ineligible {
                return Err(UsageError::NotEligible);
            }
            let fresh = !existing.is_empty()
                && existing.iter().all(|h| {
                    h.as_ref()
                        .is_some_and(|h| h.observed_at <= now && now - h.observed_at < ttl)
                });
            if fresh {
                return Ok(existing
                    .iter()
                    .flatten()
                    .zip(&flags)
                    .map(|(h, f)| effective_with(h, low, f.is_some(), now))
                    .collect());
            }
        }
        match account.harness {
            Harness::Codex => {
                let readings = self.probe_codex(account).await?;
                self.store_readings(account, readings, HeadroomSource::Probe, now, false)
                    .await
            }
            Harness::Agy => match self.probe_agy(account).await {
                Ok(readings) => {
                    self.store_readings(account, readings, HeadroomSource::Probe, now, false)
                        .await
                }
                Err(UsageError::NotEligible) => {
                    // Hold the pools back from routing; keep any old record as it was.
                    let acct = account.clone();
                    blocking(&self.store, move |s| {
                        for p in &acct.pools {
                            s.set_flag(&FlagRow {
                                account: acct.name.clone(),
                                pool: p.clone(),
                                until: now + NOT_ELIGIBLE_HOLD,
                                reason: NOT_ELIGIBLE.into(),
                            })?;
                        }
                        Ok(())
                    })
                    .await?;
                    Err(UsageError::NotEligible)
                }
                Err(e) => Err(e),
            },
            Harness::Opencode | Harness::Claude => {
                let acct = account.clone();
                blocking(&self.store, move |s| {
                    let mut out = Vec::new();
                    for ((pool, prior), flag) in acct.pools.iter().zip(existing).zip(flags) {
                        // A passive or probed record is better than ledger guesses.
                        if let Some(h) = prior.filter(|h| h.source != HeadroomSource::Ledger) {
                            out.push(effective_with(&h, low, flag.is_some(), now));
                            continue;
                        }
                        let h = ledger_headroom(s, &acct.name, pool, now)?;
                        s.upsert_headroom(&h)?;
                        out.push(effective_with(&h, low, flag.is_some(), now));
                    }
                    Ok(out)
                })
                .await
            }
        }
    }

    /// Records passive readings (codex `account/rateLimits/updated`, claude `rate_limit_event`).
    /// Windows in `readings` replace the stored ones; the other window is kept.
    pub async fn ingest_event(
        &self,
        account: &AccountFile,
        readings: Vec<WindowReading>,
        now: DateTime<Utc>,
    ) -> Result<Vec<Headroom>> {
        self.store_readings(account, readings, HeadroomSource::Event, now, true)
            .await
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

    /// `partial`: merge per window into the stored record and only touch pools that have
    /// readings. Otherwise replace the record of every pool of the account.
    async fn store_readings(
        &self,
        account: &AccountFile,
        readings: Vec<WindowReading>,
        source: HeadroomSource,
        now: DateTime<Utc>,
        partial: bool,
    ) -> Result<Vec<Headroom>> {
        let low = self.low_pct();
        let acct = account.clone();
        blocking(&self.store, move |s| {
            let mut out = Vec::new();
            for pool in &acct.pools {
                if partial && !readings.iter().any(|r| &r.pool == pool) {
                    continue;
                }
                let flag = s.get_flag(&acct.name, pool)?.filter(|f| f.until > now);
                let mut h = if partial {
                    let prior = s.get_headroom(&acct.name, pool)?;
                    merge_partial(
                        prior.as_ref(),
                        &acct.name,
                        pool,
                        &readings,
                        low,
                        source,
                        now,
                    )
                } else {
                    merge_into_record(&acct.name, pool, &readings, low, source, now)
                };
                if flag.as_ref().is_some_and(|f| f.reason == NOT_ELIGIBLE) {
                    // A successful probe proves the account works again.
                    s.clear_flag(&acct.name, pool)?;
                } else if flag.is_some() {
                    h.status = so_core::HeadroomStatus::Exhausted;
                }
                s.upsert_headroom(&h)?;
                out.push(h);
            }
            Ok(out)
        })
        .await
    }

    async fn probe_codex(&self, account: &AccountFile) -> Result<Vec<WindowReading>> {
        let launch = build_launch(account, &self.host)?
            .with_default_pass_env(&self.config.accounts.pass_env);
        let mut args = launch.argv_prefix.clone();
        args.extend(launch.extra_args.clone());
        args.extend(["app-server".into(), "--listen".into(), "stdio://".into()]);
        let req = ProbeRequest {
            pass_env: launch.pass_env,
            program: launch.program,
            args,
            env: launch.vars,
            stdin_lines: codex_probe_lines(),
            wait_for: ResponseMatcher::JsonRpcResponses,
            timeout: CODEX_TIMEOUT,
        };
        let out = self.runner.run(req).await?;
        let v: Value = serde_json::from_str(out.stdout.trim())
            .map_err(|_| UsageError::Parse("codex response is not JSON".into()))?;
        if v.get("error").is_some() {
            return Err(UsageError::Probe("codex returned a JSON-RPC error".into()));
        }
        let readings = parse_codex_rate_limits(&v);
        if readings.is_empty() {
            return Err(UsageError::Parse(
                "codex response has no rate-limit windows".into(),
            ));
        }
        Ok(readings)
    }

    /// agy needs no bwrap for `/usage` (ADR-0005): `--gemini_dir` selects the account.
    async fn probe_agy(&self, account: &AccountFile) -> Result<Vec<WindowReading>> {
        let program = resolve_binary(&account.binary, self.host.path_var.as_deref())?;
        let home = self.host.expand(&account.home)?;
        let req = ProbeRequest {
            pass_env: account
                .pass_env
                .iter()
                .chain(&self.config.accounts.pass_env)
                .cloned()
                .collect(),
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
        let readings = parse_agy_usage(&out.stdout)?;
        if readings.is_empty() {
            // Never echo the output: it can contain sign-in URLs.
            return Err(if !out.success {
                UsageError::Probe("agy exited with an error and printed no usage".into())
            } else if out.stdout.trim().is_empty() {
                UsageError::Probe("agy printed nothing".into())
            } else {
                UsageError::Parse("agy output has no usage rows".into())
            });
        }
        Ok(readings)
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
    use so_core::HeadroomStatus;
    use so_store::FlagRow;
    use std::sync::Mutex;

    const FX: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

    struct Fake {
        reply: Mutex<Result<ProbeOutput>>,
        calls: Mutex<Vec<ProbeRequest>>,
    }
    impl Fake {
        fn new(reply: Result<ProbeOutput>) -> Arc<Self> {
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
        fn run<'a>(&'a self, req: ProbeRequest) -> BoxFuture<'a, Result<ProbeOutput>> {
            self.calls.lock().unwrap().push(req);
            let r = match &*self.reply.lock().unwrap() {
                Ok(o) => Ok(o.clone()),
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

    fn codex_reply() -> Result<ProbeOutput> {
        let v: Value = serde_json::from_str(
            &std::fs::read_to_string(format!("{FX}/codex/rate-limits-read.response.json")).unwrap(),
        )
        .unwrap();
        Ok(ProbeOutput::ok(v.to_string()))
    }

    #[tokio::test]
    async fn codex_ttl_force_and_request_lines() {
        let fake = Fake::new(codex_reply());
        let (d, mut svc, mut acct) = setup(fake.clone(), Harness::Codex, "codex");
        acct.pass_env = vec!["SO_ACCOUNT_VAR".into()];
        svc.config.accounts.pass_env = vec!["SO_GLOBAL_VAR".into()];
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
        assert_eq!(call.pass_env, ["SO_ACCOUNT_VAR", "SO_GLOBAL_VAR"]);
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
        let fake = Fake::new(Ok(ProbeOutput::ok(ok)));
        let (d, mut svc, mut acct) = setup(fake.clone(), Harness::Agy, "agy");
        acct.pass_env = vec!["SO_ACCOUNT_VAR".into()];
        svc.config.accounts.pass_env = vec!["SO_GLOBAL_VAR".into()];
        let h = svc.refresh_at(&acct, false, Utc::now()).await.unwrap();
        let call = fake.calls.lock().unwrap()[0].clone();
        assert_eq!(call.pass_env, ["SO_ACCOUNT_VAR", "SO_GLOBAL_VAR"]);
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
        let (_d, svc, acct) = setup(fake.clone(), Harness::Agy, "agy");
        let now = DateTime::from_timestamp(Utc::now().timestamp(), 0).unwrap();
        let r = svc.refresh_at(&acct, false, now).await;
        assert!(matches!(r, Err(UsageError::NotEligible)));
        // No Unknown record cached as fresh; the pools are flagged so the router skips them.
        assert!(
            svc.store
                .get_headroom("agy-acc", "gemini")
                .unwrap()
                .is_none()
        );
        for pool in ["gemini", "claude"] {
            let f = svc.store.get_flag("agy-acc", pool).unwrap().unwrap();
            assert_eq!(
                (f.reason.as_str(), f.until),
                ("not_eligible", now + CDur::hours(1))
            );
            assert!(!crate::is_available(&svc.store, "agy-acc", pool, now).unwrap());
        }
        // Within TTL: error again without probing; after TTL: probes again; force always probes.
        let r = svc.refresh_at(&acct, false, now + CDur::minutes(2)).await;
        assert!(matches!(r, Err(UsageError::NotEligible)));
        assert_eq!(fake.n(), 1);
        let _ = svc.refresh_at(&acct, false, now + CDur::minutes(6)).await;
        assert_eq!(fake.n(), 2);
        let _ = svc.refresh_at(&acct, true, now + CDur::minutes(7)).await;
        assert_eq!(fake.n(), 3);
    }

    #[tokio::test]
    async fn successful_probe_clears_not_eligible_flag() {
        let ok = std::fs::read_to_string(format!("{FX}/agy/usage-ok.txt")).unwrap();
        let (_d, svc, acct) = setup(Fake::new(Ok(ProbeOutput::ok(ok))), Harness::Agy, "agy");
        let now = Utc::now();
        for p in ["gemini", "claude"] {
            svc.store
                .set_flag(&FlagRow {
                    account: "agy-acc".into(),
                    pool: p.into(),
                    until: now + CDur::minutes(30),
                    reason: "not_eligible".into(),
                })
                .unwrap();
        }
        let h = svc.refresh_at(&acct, true, now).await.unwrap();
        assert_eq!(h[0].status, HeadroomStatus::Ok);
        assert!(svc.store.get_flag("agy-acc", "gemini").unwrap().is_none());
    }

    #[tokio::test]
    async fn bad_probe_output_never_overwrites_good_record() {
        let ok = std::fs::read_to_string(format!("{FX}/agy/usage-ok.txt")).unwrap();
        let fake = Fake::new(Ok(ProbeOutput::ok(ok)));
        let (_d, svc, acct) = setup(fake.clone(), Harness::Agy, "agy");
        let now = DateTime::from_timestamp(Utc::now().timestamp(), 0).unwrap();
        let good = svc.refresh_at(&acct, true, now).await.unwrap();
        let later = now + CDur::minutes(10);
        let cases = [
            (
                ProbeOutput {
                    stdout: "boom\n".into(),
                    success: false,
                },
                "Probe",
            ),
            (
                ProbeOutput {
                    stdout: String::new(),
                    success: false,
                },
                "Probe",
            ),
            (ProbeOutput::ok("hello without rows\n"), "Parse"),
        ];
        for (out, kind) in cases {
            *fake.reply.lock().unwrap() = Ok(out);
            let e = svc.refresh_at(&acct, true, later).await.unwrap_err();
            assert!(
                matches!(
                    (&e, kind),
                    (UsageError::Probe(_), "Probe") | (UsageError::Parse(_), "Parse")
                ),
                "{e:?}"
            );
            assert!(!e.to_string().contains("boom") && !e.to_string().contains("hello"));
        }
        let kept = svc
            .store
            .get_headroom("agy-acc", "gemini")
            .unwrap()
            .unwrap();
        assert_eq!(kept, good[0]);
        // Codex with a response that has no windows is a parse error too.
        let fake = Fake::new(Ok(ProbeOutput::ok(r#"{"id":2,"result":{}}"#)));
        let (_d, svc, acct) = setup(fake, Harness::Codex, "codex");
        assert!(matches!(
            svc.refresh_at(&acct, true, now).await,
            Err(UsageError::Parse(_))
        ));
    }

    #[tokio::test]
    async fn future_observed_at_is_not_fresh() {
        let fake = Fake::new(codex_reply());
        let (_d, svc, acct) = setup(fake.clone(), Harness::Codex, "codex");
        let now = Utc::now();
        svc.refresh_at(&acct, true, now + CDur::hours(2))
            .await
            .unwrap();
        assert_eq!(fake.n(), 1);
        svc.refresh_at(&acct, false, now).await.unwrap();
        assert_eq!(fake.n(), 2);
    }

    #[tokio::test]
    async fn event_merges_per_window() {
        let (_d, svc, acct) = setup(Fake::new(codex_reply()), Harness::Codex, "codex");
        let now = Utc::now();
        svc.refresh_at(&acct, true, now).await.unwrap();
        let ev = vec![WindowReading {
            pool: "default".into(),
            window: Window::FiveHour,
            remaining_pct: Some(5.0),
            resets_at: None,
        }];
        let later = now + CDur::minutes(4);
        let h = svc.ingest_event(&acct, ev, later).await.unwrap();
        // The probe's timestamp survives, so the TTL still counts from the probe.
        assert_eq!(
            h[0].observed_at,
            svc.store
                .get_headroom("codex-acc", "default")
                .unwrap()
                .unwrap()
                .observed_at
        );
        assert!((later - h[0].observed_at) > CDur::minutes(3));
        assert_eq!(h[0].five_hour_remaining_pct, Some(5.0));
        assert_eq!(h[0].weekly_remaining_pct, Some(30.0));
        assert_eq!(
            (h[0].status, h[0].source),
            (HeadroomStatus::Low, HeadroomSource::Event)
        );
    }

    #[tokio::test]
    async fn agy_output_with_eligibility_text_is_not_eligible() {
        let fake = Fake::new(Ok(ProbeOutput::ok(
            "error: Eligibility check failed: nope\nhttps://secret\n",
        )));
        let (_d, svc, acct) = setup(fake, Harness::Agy, "agy");
        assert!(matches!(
            svc.refresh_at(&acct, false, Utc::now()).await,
            Err(UsageError::NotEligible)
        ));
    }

    #[tokio::test]
    async fn claude_uses_ledger_and_keeps_event_records() {
        let fake = Fake::new(Ok(ProbeOutput::ok(String::new())));
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
        let h = svc.ingest_event(&acct, ev, now).await.unwrap();
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

    #[test]
    fn probe_environment_isolation() {
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "probe::tests::probe_environment_child",
                "--nocapture",
            ])
            .env("SO_ENV_TEST_CHILD", "1")
            .env("SO_TEST_SECRET", "must-not-inherit")
            .env("LC_SO_TEST", "allowlisted")
            .env("SO_ACCOUNT_VAR", "account")
            .env("SO_DRIVER_VAR", "parent")
            .status()
            .unwrap();
        assert!(status.success());
    }

    #[tokio::test]
    async fn probe_environment_child() {
        if std::env::var_os("SO_ENV_TEST_CHILD").is_none() {
            return;
        }
        let req = ProbeRequest {
            pass_env: vec!["SO_ACCOUNT_VAR".into(), "SO_DRIVER_VAR".into(), "MISSING_OPT_IN".into()],
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "test \"$SO_ACCOUNT_VAR\" = account && test -z \"${SO_TEST_SECRET+x}\" && test \"$LC_SO_TEST\" = allowlisted && test \"$SO_DRIVER_VAR\" = explicit".into()],
            env: vec![("SO_DRIVER_VAR".into(), "explicit".into())],
            stdin_lines: vec![],
            wait_for: ResponseMatcher::Eof,
            timeout: Duration::from_secs(5),
        };
        assert!(TokioProbeRunner.run(req).await.unwrap().success);
    }

    #[tokio::test]
    async fn real_runner_jsonrpc_and_eof() {
        // A shell stand-in for app-server proves line-by-line request/response handling.
        let script = r#"read a; echo '{"id":1,"result":{}}'; read b; read c; echo '{"id":2,"result":{"ok":true}}'"#;
        let req = ProbeRequest {
            pass_env: vec![],
            program: "sh".into(),
            args: vec!["-c".into(), script.into()],
            env: vec![],
            stdin_lines: codex_probe_lines(),
            wait_for: ResponseMatcher::JsonRpcResponses,
            timeout: Duration::from_secs(5),
        };
        let out = TokioProbeRunner.run(req).await.unwrap();
        assert!(out.stdout.contains("\"ok\":true"));
        let req = ProbeRequest {
            pass_env: vec![],
            program: "sh".into(),
            args: vec!["-c".into(), "echo hi".into()],
            env: vec![],
            stdin_lines: vec![],
            wait_for: ResponseMatcher::Eof,
            timeout: Duration::from_secs(5),
        };
        assert_eq!(TokioProbeRunner.run(req).await.unwrap().stdout.trim(), "hi");
        let req = ProbeRequest {
            pass_env: vec![],
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
        // Nonzero exit is reported, and output is capped at 1 MiB.
        let req = ProbeRequest {
            pass_env: vec![],
            program: "sh".into(),
            args: vec!["-c".into(), "echo err; exit 3".into()],
            env: vec![],
            stdin_lines: vec![],
            wait_for: ResponseMatcher::Eof,
            timeout: Duration::from_secs(5),
        };
        let out = TokioProbeRunner.run(req).await.unwrap();
        assert!(!out.success);
        let req = ProbeRequest {
            pass_env: vec![],
            program: "sh".into(),
            args: vec!["-c".into(), "yes | head -c 5000000".into()],
            env: vec![],
            stdin_lines: vec![],
            wait_for: ResponseMatcher::Eof,
            timeout: Duration::from_secs(5),
        };
        match TokioProbeRunner.run(req).await {
            Err(UsageError::Probe(m)) => assert_eq!(m, "output too large"),
            other => panic!("{other:?}"),
        }
        // Exactly at the cap is still accepted.
        let req = ProbeRequest {
            pass_env: vec![],
            program: "sh".into(),
            args: vec!["-c".into(), "head -c 1048576 /dev/zero".into()],
            env: vec![],
            stdin_lines: vec![],
            wait_for: ResponseMatcher::Eof,
            timeout: Duration::from_secs(5),
        };
        assert_eq!(
            TokioProbeRunner.run(req).await.unwrap().stdout.len(),
            1 << 20
        );
    }

    #[tokio::test]
    async fn timeout_kills_the_process_group() {
        let d = tempfile::tempdir().unwrap();
        let pidfile = d.path().join("pid");
        // A grandchild that would outlive a plain child kill.
        let script = format!("sleep 30 & echo $! > {}; wait", pidfile.display());
        let req = ProbeRequest {
            pass_env: vec![],
            program: "sh".into(),
            args: vec!["-c".into(), script],
            env: vec![],
            stdin_lines: vec![],
            wait_for: ResponseMatcher::Eof,
            timeout: Duration::from_millis(500),
        };
        assert!(matches!(
            TokioProbeRunner.run(req).await,
            Err(UsageError::Timeout(_))
        ));
        let pid: i32 = std::fs::read_to_string(&pidfile)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        let p = rustix::process::Pid::from_raw(pid).unwrap();
        assert!(rustix::process::test_kill_process(p).is_err());
    }

    /// Manual: probes the real logins named by `SO_PROBE_CODEX_HOME` / `SO_PROBE_AGY_HOME`
    /// (credential homes; each is skipped when unset). Prints percentages and reset times.
    #[tokio::test]
    #[ignore = "uses the real codex/agy binaries and logins"]
    async fn real_probe_manual() {
        let svc = UsageService::new(
            Store::open_in_memory().unwrap(),
            Config::default(),
            HostEnv::detect().unwrap(),
            Arc::new(TokioProbeRunner),
        );
        let mut accounts = Vec::new();
        if let Ok(h) = std::env::var("SO_PROBE_CODEX_HOME") {
            accounts.push(AccountFile::new("codex-probe", Harness::Codex, &h, "codex").unwrap());
        }
        if let Ok(h) = std::env::var("SO_PROBE_AGY_HOME") {
            accounts.push(AccountFile::new("agy-probe", Harness::Agy, &h, "agy").unwrap());
        }
        if accounts.is_empty() {
            println!("skipped: set SO_PROBE_CODEX_HOME and/or SO_PROBE_AGY_HOME");
        }
        for a in accounts {
            match svc.refresh(&a, true).await {
                Ok(hs) => {
                    for h in hs {
                        println!(
                            "{}/{}: {:?} 5h={:?}% resets {:?}; weekly={:?}% resets {:?}",
                            a.name,
                            h.pool,
                            h.status,
                            h.five_hour_remaining_pct,
                            h.five_hour_resets_at,
                            h.weekly_remaining_pct,
                            h.weekly_resets_at
                        );
                    }
                }
                Err(e) => println!("{}: error {e}", a.name),
            }
        }
    }
}

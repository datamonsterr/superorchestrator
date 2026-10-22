//! One `agent()` step: cache lookup, host call, failover waits, stall restarts.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::Utc;
use serde_json::Value;
use sha2::{Digest, Sha256};
use so_store::{Store, WorkflowCacheRow, WorkflowStepRow};
use tokio::sync::{Semaphore, SemaphorePermit};

use crate::host::{HostError, StepEvent, StepOutcome, StepRequest, WorkflowHost};

/// Longest single wait for an account to come back.
const MAX_WAIT: Duration = Duration::from_secs(24 * 60 * 60);
/// Floor for a wait, so a host that keeps answering `Unavailable` cannot spin.
const MIN_WAIT: Duration = Duration::from_secs(1);
/// A stalled step is restarted at most this many times.
const MAX_RESTARTS: u32 = 5;
/// Agents per run (parity with Claude Code).
const MAX_AGENTS: usize = 1000;

pub(crate) struct StepError {
    pub msg: String,
    /// Infrastructure failure: the script must not treat it as a failed agent.
    pub fatal: bool,
}

fn host_err(e: HostError) -> StepError {
    match e {
        HostError::Failed(m) => StepError::agent(m),
        HostError::Fatal(m) => StepError::fatal(m),
    }
}

impl StepError {
    fn agent(msg: impl Into<String>) -> Self {
        Self {
            msg: msg.into(),
            fatal: false,
        }
    }
    fn fatal(msg: impl Into<String>) -> Self {
        Self {
            msg: msg.into(),
            fatal: true,
        }
    }
}

struct Ticket {
    hash: String,
    prompt: String,
    opts: Value,
    phase: Option<String>,
}

/// Shared state of one workflow run.
pub(crate) struct RunState {
    pub host: Arc<dyn WorkflowHost>,
    pub store: Store,
    pub name: String,
    pub run_id: String,
    /// Directory relative `cwd` options resolve against; part of every cache key.
    base_cwd: PathBuf,
    /// Skip cache reads (results are still written).
    fresh: bool,
    started: AtomicUsize,
    occurrences: Mutex<HashMap<String, usize>>,
    tickets: Mutex<HashMap<usize, Ticket>>,
    permits: Semaphore,
}

fn sha_hex(parts: &[&str]) -> String {
    let mut h = Sha256::new();
    for p in parts {
        // Length prefix: ("ab", "c") and ("a", "bc") must not collide.
        h.update((p.len() as u64).to_le_bytes());
        h.update(p.as_bytes());
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// JSON with object keys sorted, so equal options hash equal.
fn canonical(v: &Value) -> String {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<_> = m.keys().collect();
            keys.sort();
            let items: Vec<String> = keys
                .into_iter()
                .map(|k| format!("{}:{}", Value::String(k.clone()), canonical(&m[k])))
                .collect();
            format!("{{{}}}", items.join(","))
        }
        Value::Array(a) => {
            let items: Vec<String> = a.iter().map(canonical).collect();
            format!("[{}]", items.join(","))
        }
        other => other.to_string(),
    }
}

/// Canonical working directory of a step: the `cwd` option (relative to the run's cwd) or
/// the run's cwd. Falls back to the unresolved path when it does not exist.
fn resolve_cwd(base: &Path, opt: Option<&str>) -> String {
    let p = match opt {
        Some(o) => base.join(o),
        None => base.to_path_buf(),
    };
    p.canonicalize().unwrap_or(p).to_string_lossy().into_owned()
}

async fn db<T: Send + 'static>(
    store: &Store,
    f: impl FnOnce(&Store) -> so_store::Result<T> + Send + 'static,
) -> Result<T, StepError> {
    let store = store.clone();
    tokio::task::spawn_blocking(move || f(&store))
        .await
        .map_err(|e| StepError::fatal(format!("store task: {e}")))?
        .map_err(|e| StepError::fatal(format!("store: {e}")))
}

impl RunState {
    pub fn new(
        host: Arc<dyn WorkflowHost>,
        store: Store,
        name: String,
        run_id: String,
        max_concurrent: usize,
        base_cwd: PathBuf,
        fresh: bool,
    ) -> Self {
        Self {
            host,
            store,
            name,
            run_id,
            base_cwd,
            fresh,
            started: AtomicUsize::new(0),
            occurrences: Mutex::new(HashMap::new()),
            tickets: Mutex::new(HashMap::new()),
            permits: Semaphore::new(max_concurrent.max(1)),
        }
    }

    /// Registers an `agent()` call. Runs synchronously at call time so the step index and
    /// the occurrence number follow the script's call order.
    pub fn begin(&self, prompt: &str, opts_json: &str, phase: &str) -> Result<usize, String> {
        let opts: Value =
            serde_json::from_str(opts_json).map_err(|e| format!("bad options: {e}"))?;
        let step = self.started.fetch_add(1, Ordering::SeqCst);
        if step >= MAX_AGENTS {
            return Err(format!("a workflow may start at most {MAX_AGENTS} agents"));
        }
        let cwd = resolve_cwd(&self.base_cwd, opts.get("cwd").and_then(Value::as_str));
        let base = sha_hex(&[&self.name, prompt, &canonical(&opts), &cwd]);
        let n = {
            let mut occ = self.occurrences.lock().map_err(|e| e.to_string())?;
            let n = occ.entry(base.clone()).or_insert(0);
            *n += 1;
            *n - 1
        };
        let hash = sha_hex(&[&base, &n.to_string()]);
        let ticket = Ticket {
            hash,
            prompt: prompt.to_string(),
            opts,
            phase: (!phase.is_empty()).then(|| phase.to_string()),
        };
        self.tickets
            .lock()
            .map_err(|e| e.to_string())?
            .insert(step, ticket);
        Ok(step)
    }

    async fn emit(&self, t: &Ticket, step: usize, run_id: Option<&str>, state: &str) {
        let ev = StepEvent {
            workflow_run_id: self.run_id.clone(),
            step_index: step,
            phase: t.phase.clone(),
            prompt_hash: t.hash.clone(),
            run_id: run_id.map(str::to_string),
            state: state.to_string(),
        };
        self.host.step_status(&ev).await;
    }

    async fn set_state(
        &self,
        t: &Ticket,
        step: usize,
        run_id: Option<String>,
        state: &'static str,
        finished: bool,
    ) -> Result<(), StepError> {
        let wf = self.run_id.clone();
        let rid = run_id.clone();
        db(&self.store, move |s| {
            s.update_workflow_step(
                &wf,
                step as i64,
                state,
                rid.as_deref(),
                finished.then(Utc::now),
            )
        })
        .await?;
        self.emit(t, step, run_id.as_deref(), state).await;
        Ok(())
    }

    /// Runs a registered step to its result.
    pub async fn run(&self, step: usize) -> Result<Value, StepError> {
        let t = self
            .tickets
            .lock()
            .map_err(|e| StepError::fatal(e.to_string()))?
            .remove(&step)
            .ok_or_else(|| StepError::fatal(format!("unknown step {step}")))?;
        let key = t.hash.clone();
        let cached = if self.fresh {
            None
        } else {
            db(&self.store, move |s| s.get_workflow_cache(&key)).await?
        };
        if let Some(c) = cached {
            let now = Utc::now();
            let row = self.step_row(&t, step, "cached", Some(now));
            db(&self.store, move |s| s.insert_workflow_step(&row)).await?;
            self.emit(&t, step, None, "cached").await;
            return serde_json::from_str(&c.result_json)
                .map_err(|e| StepError::fatal(format!("corrupt cache entry: {e}")));
        }

        let mut permit = Some(self.acquire().await?);
        let row = self.step_row(&t, step, "running", None);
        db(&self.store, move |s| s.insert_workflow_step(&row)).await?;
        self.emit(&t, step, None, "running").await;

        let req = StepRequest {
            workflow_run_id: self.run_id.clone(),
            step_index: step,
            phase: t.phase.clone(),
            prompt: t.prompt.clone(),
            opts: t.opts.clone(),
            stall_ms: t.opts.get("stall_ms").and_then(Value::as_u64),
        };
        match self.drive(&req, &t, step, &mut permit).await {
            Ok((result, run_id)) => {
                let row = WorkflowCacheRow {
                    key_hash: t.hash.clone(),
                    workflow: self.name.clone(),
                    result_json: result.to_string(),
                    created_at: Utc::now(),
                };
                db(&self.store, move |s| s.put_workflow_cache(&row)).await?;
                self.set_state(&t, step, run_id, "succeeded", true).await?;
                Ok(result)
            }
            Err(e) => {
                // Best effort: the original error is the one worth reporting.
                let _ = self.set_state(&t, step, None, "failed", true).await;
                Err(e)
            }
        }
    }

    async fn acquire(&self) -> Result<SemaphorePermit<'_>, StepError> {
        self.permits
            .acquire()
            .await
            .map_err(|e| StepError::fatal(e.to_string()))
    }

    fn step_row(
        &self,
        t: &Ticket,
        step: usize,
        state: &str,
        finished_at: Option<chrono::DateTime<Utc>>,
    ) -> WorkflowStepRow {
        WorkflowStepRow {
            workflow_run_id: self.run_id.clone(),
            step_index: step as i64,
            phase: t.phase.clone(),
            prompt_hash: t.hash.clone(),
            run_id: None,
            state: state.into(),
            started_at: Utc::now(),
            finished_at,
        }
    }

    /// Host calls with failover waits and stall restarts. The concurrency permit is given
    /// up while waiting for an account.
    async fn drive<'a>(
        &'a self,
        req: &StepRequest,
        t: &Ticket,
        step: usize,
        permit: &mut Option<SemaphorePermit<'a>>,
    ) -> Result<(Value, Option<String>), StepError> {
        let mut restarts = 0u32;
        let mut outcome = self.host.spawn_and_wait(req).await;
        loop {
            match outcome.map_err(host_err)? {
                StepOutcome::Done { result, run_id } => return Ok((result, run_id)),
                StepOutcome::Unavailable { retry_at } => {
                    self.set_state(t, step, None, "waiting", false).await?;
                    *permit = None;
                    let wait = (retry_at - Utc::now())
                        .to_std()
                        .unwrap_or_default()
                        .clamp(MIN_WAIT, MAX_WAIT);
                    tokio::time::sleep(wait).await;
                    *permit = Some(self.acquire().await?);
                    self.set_state(t, step, None, "running", false).await?;
                    outcome = self.host.spawn_and_wait(req).await;
                }
                StepOutcome::Stalled { run_id } => {
                    if restarts >= MAX_RESTARTS {
                        return Err(StepError::agent(format!(
                            "agent stalled on all {} attempts",
                            MAX_RESTARTS + 1
                        )));
                    }
                    restarts += 1;
                    self.set_state(t, step, run_id.clone(), "stalled", false)
                        .await?;
                    self.set_state(t, step, None, "running", false).await?;
                    outcome = self.host.restart(req, run_id.as_deref(), restarts).await;
                }
            }
        }
    }

    /// Called once the script is over (returned, threw, cancelled or timed out), after the
    /// runtime dropped any in-flight host futures: marks steps that never finished as
    /// `cancelled` and removes rows left by a longer earlier run with the same id.
    pub async fn finalize(&self) -> Result<(), StepError> {
        let (wf, started) = (self.run_id.clone(), self.started.load(Ordering::SeqCst));
        let open = db(&self.store, move |s| {
            let open = s.cancel_open_workflow_steps(&wf, Utc::now())?;
            s.delete_workflow_steps_from(&wf, started as i64)?;
            Ok(open)
        })
        .await?;
        for row in open {
            self.host
                .step_status(&StepEvent {
                    workflow_run_id: self.run_id.clone(),
                    step_index: row.step_index as usize,
                    phase: row.phase,
                    prompt_hash: row.prompt_hash,
                    run_id: row.run_id,
                    state: "cancelled".into(),
                })
                .await;
        }
        Ok(())
    }
}

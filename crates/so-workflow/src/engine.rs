//! The QuickJS workflow engine.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use chrono::Utc;
use rquickjs::function::Async;
use rquickjs::{AsyncContext, AsyncRuntime, CatchResultExt, Ctx, Function, Value as JsValue};
use serde_json::{Value, json};
use so_core::new_id;
use so_store::{Store, WorkflowRunRow};
use tokio::sync::Notify;

use crate::error::{Result, WorkflowError};
use crate::host::WorkflowHost;
use crate::script::{PRELUDE, wrap};
use crate::step::{RunState, StepError};

/// Limits of the sandbox.
#[derive(Debug, Clone)]
pub struct EngineOptions {
    /// QuickJS heap limit in bytes (default 64 MiB).
    pub memory_limit: usize,
    /// JS stack limit in bytes (default 1 MiB).
    pub stack_limit: usize,
    /// Agents running at once (default 16, as in Claude Code).
    pub max_concurrent_agents: usize,
    /// Wall-clock limit for a whole run, waits for accounts included. `None` (default) means
    /// no limit. Enforced both by the JS interrupt handler and while awaiting the host.
    pub max_duration: Option<Duration>,
}

impl Default for EngineOptions {
    fn default() -> Self {
        Self {
            memory_limit: 64 * 1024 * 1024,
            stack_limit: 1024 * 1024,
            max_concurrent_agents: 16,
            max_duration: None,
        }
    }
}

/// Cooperative cancellation. Interrupts running JS (even `while(true){}`) and any wait on
/// the host.
#[derive(Clone, Default)]
pub struct CancelToken {
    flag: Arc<AtomicBool>,
    notify: Arc<Notify>,
}

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    async fn cancelled(&self) {
        loop {
            let waiter = self.notify.notified();
            if self.is_cancelled() {
                return;
            }
            waiter.await;
        }
    }
}

/// Per-run settings for [`WorkflowEngine::run_with`].
#[derive(Default)]
pub struct RunOptions {
    /// Id for the `workflow_runs` row; generated when `None`. An existing row is reused.
    pub run_id: Option<String>,
    pub cancel: CancelToken,
    /// Directory the run's agents work in and that scopes the result cache: the same prompt
    /// and options in another directory are a different step. Relative `cwd` options of
    /// `agent()` resolve against it. Defaults to the process's current directory.
    pub cwd: Option<PathBuf>,
    /// Ignore cached results and run every step again (new results are still cached).
    pub fresh: bool,
}

/// Runs workflow scripts. Cheap to clone.
#[derive(Clone)]
pub struct WorkflowEngine {
    store: Store,
    options: EngineOptions,
}

impl WorkflowEngine {
    pub fn new(store: Store) -> Self {
        Self::with_options(store, EngineOptions::default())
    }

    pub fn with_options(store: Store, options: EngineOptions) -> Self {
        Self { store, options }
    }

    /// Runs `script_source` as workflow `name` and returns its result as JSON.
    ///
    /// Script styles (see `script::wrap`): a body with top-level `await`/`return`, or
    /// `export default async function (args) {...}`.
    pub async fn run(
        &self,
        script_source: &str,
        name: &str,
        args: Value,
        host: Arc<dyn WorkflowHost>,
    ) -> Result<Value> {
        self.run_with(RunOptions::default(), script_source, name, args, host)
            .await
            .map(|(_, v)| v)
    }

    /// Like [`run`](Self::run), with a caller-chosen run id and a cancel token. Returns the
    /// run id with the result.
    pub async fn run_with(
        &self,
        opts: RunOptions,
        script_source: &str,
        name: &str,
        args: Value,
        host: Arc<dyn WorkflowHost>,
    ) -> Result<(String, Value)> {
        let run_id = opts.run_id.unwrap_or_else(new_id);
        let cwd = opts
            .cwd
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default();
        let row = WorkflowRunRow {
            id: run_id.clone(),
            name: name.to_string(),
            session_id: None,
            state: "running".into(),
            args_json: args.to_string(),
            started_at: Utc::now(),
            finished_at: None,
            result_json: None,
            error: None,
        };
        {
            let store = self.store.clone();
            let id = run_id.clone();
            blocking(move || {
                if store.get_workflow_run(&id)?.is_none() {
                    store.insert_workflow_run(&row)?;
                } else {
                    store.reset_workflow_run(&id, &row.args_json)?;
                }
                Ok(())
            })
            .await?;
        }

        let state = Arc::new(RunState::new(
            host,
            self.store.clone(),
            name.to_string(),
            run_id.clone(),
            self.options.max_concurrent_agents,
            cwd,
            opts.fresh,
        ));
        let result = self
            .execute(state.clone(), script_source, &args, &opts.cancel)
            .await;
        // The runtime (and with it every in-flight host future) is gone by now.
        let finalized = state.finalize().await;

        let final_state = match &result {
            Ok(_) => "succeeded",
            Err(WorkflowError::Cancelled) => "cancelled",
            Err(_) => "failed",
        };
        let (result_json, error) = match &result {
            Ok(v) => (Some(v.to_string()), None),
            Err(e) => (None, Some(e.to_string())),
        };
        let store = self.store.clone();
        let id = run_id.clone();
        blocking(move || {
            store.finish_workflow_run(
                &id,
                final_state,
                Utc::now(),
                result_json.as_deref(),
                error.as_deref(),
            )
        })
        .await?;
        if let Err(e) = finalized {
            return Err(WorkflowError::Engine(e.msg));
        }
        result.map(|v| (run_id, v))
    }

    async fn execute(
        &self,
        state: Arc<RunState>,
        source: &str,
        args: &Value,
        cancel: &CancelToken,
    ) -> Result<Value> {
        let engine_err = |e: rquickjs::Error| WorkflowError::Engine(e.to_string());
        let rt = AsyncRuntime::new().map_err(engine_err)?;
        rt.set_memory_limit(self.options.memory_limit).await;
        rt.set_max_stack_size(self.options.stack_limit).await;
        let flag = cancel.flag.clone();
        let timed_out = Arc::new(AtomicBool::new(false));
        let deadline = self.options.max_duration.map(|d| Instant::now() + d);
        let to = timed_out.clone();
        rt.set_interrupt_handler(Some(Box::new(move || {
            if flag.load(Ordering::SeqCst) {
                return true;
            }
            if deadline.is_some_and(|d| Instant::now() >= d) {
                to.store(true, Ordering::SeqCst);
                return true;
            }
            false
        })))
        .await;
        // `full` adds only the language intrinsics; QuickJS has no fs, network or process
        // APIs, and no module loader is installed.
        let ctx = AsyncContext::full(&rt).await.map_err(engine_err)?;

        let wrapped = wrap(source).map_err(WorkflowError::Script)?;
        let args_json = args.to_string();
        let work = ctx.async_with(async move |ctx| drive(ctx, state, args_json, wrapped).await);
        let max = self.options.max_duration;
        let out = tokio::select! {
            r = work => r,
            () = cancel.cancelled() => return Err(WorkflowError::Cancelled),
            () = async {
                match max {
                    Some(d) => tokio::time::sleep(d).await,
                    None => std::future::pending().await,
                }
            } => return Err(WorkflowError::Timeout),
        };
        match out {
            Ok(v) => Ok(v),
            Err(_) if cancel.is_cancelled() => Err(WorkflowError::Cancelled),
            Err(_) if timed_out.load(Ordering::SeqCst) => Err(WorkflowError::Timeout),
            Err(msg) => Err(WorkflowError::Script(msg)),
        }
    }
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> so_store::Result<T> + Send + 'static,
) -> Result<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| WorkflowError::Engine(e.to_string()))?
        .map_err(WorkflowError::from)
}

fn step_json(r: std::result::Result<Value, StepError>) -> String {
    match r {
        Ok(v) => json!({ "ok": v }),
        Err(e) => json!({ "err": e.msg, "fatal": e.fatal }),
    }
    .to_string()
}

/// Installs the native hooks, runs the prelude and the script, and returns its JSON result.
async fn drive(
    ctx: Ctx<'_>,
    state: Arc<RunState>,
    args_json: String,
    wrapped: String,
) -> std::result::Result<Value, String> {
    install(&ctx, state, args_json)
        .catch(&ctx)
        .map_err(|e| e.to_string())?;
    let promise: rquickjs::Promise = ctx.eval(wrapped).catch(&ctx).map_err(|e| e.to_string())?;
    let value: JsValue = promise
        .into_future::<JsValue>()
        .await
        .catch(&ctx)
        .map_err(|e| e.to_string())?;
    match ctx
        .json_stringify(value)
        .catch(&ctx)
        .map_err(|e| e.to_string())?
    {
        None => Ok(Value::Null),
        Some(s) => {
            let text = s.to_string().map_err(|e| e.to_string())?;
            serde_json::from_str(&text).map_err(|e| e.to_string())
        }
    }
}

fn install<'js>(ctx: &Ctx<'js>, state: Arc<RunState>, args_json: String) -> rquickjs::Result<()> {
    let g = ctx.globals();

    let st = state.clone();
    g.set(
        "__begin",
        Function::new(
            ctx.clone(),
            move |prompt: String, opts: String, phase: String| {
                match st.begin(&prompt, &opts, &phase) {
                    Ok(step) => json!({ "ok": step }),
                    Err(e) => json!({ "err": e }),
                }
                .to_string()
            },
        )?,
    )?;

    let st = state.clone();
    g.set(
        "__run",
        Function::new(
            ctx.clone(),
            Async(move |step: u32| {
                let st = st.clone();
                async move { step_json(st.run(step as usize).await) }
            }),
        )?,
    )?;

    let st = state;
    g.set(
        "__event",
        Function::new(
            ctx.clone(),
            Async(move |kind: String, payload: String| {
                let st = st.clone();
                async move {
                    match kind.as_str() {
                        "log" => st.host.log(&payload).await,
                        "phase_started" => st.host.phase_started(&payload).await,
                        "phase_finished" => st.host.phase_finished(&payload).await,
                        _ => {}
                    }
                }
            }),
        )?,
    )?;

    g.set("__args_json", args_json)?;
    ctx.eval::<(), _>(PRELUDE)?;
    Ok(())
}

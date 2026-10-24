//! A scriptable [`WorkflowHost`] for tests (feature `test-util`).
#![allow(clippy::unwrap_used)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::time::Instant;

use crate::{BoxFut, HostError, StepEvent, StepOutcome, StepRequest, WorkflowHost};

type Handler = dyn Fn(&StepRequest, usize) -> Result<StepOutcome, HostError> + Send + Sync;

#[derive(Debug, Clone)]
pub struct Call {
    pub prompt: String,
    pub opts: Value,
    pub start: Instant,
    pub end: Instant,
}

pub struct FakeHost {
    handler: Box<Handler>,
    delay: Duration,
    calls: Mutex<Vec<Call>>,
    restarts: Mutex<Vec<u32>>,
    events: Mutex<Vec<String>>,
    statuses: Mutex<Vec<(usize, String)>>,
    n: AtomicUsize,
    hang: Mutex<Vec<String>>,
    dropped: Arc<AtomicUsize>,
    hung: AtomicUsize,
    hung_entered: tokio::sync::Notify,
}

impl FakeHost {
    pub fn new(
        handler: impl Fn(&StepRequest, usize) -> Result<StepOutcome, HostError> + Send + Sync + 'static,
    ) -> Arc<Self> {
        Self::with_delay(Duration::ZERO, handler)
    }

    pub fn with_delay(
        delay: Duration,
        handler: impl Fn(&StepRequest, usize) -> Result<StepOutcome, HostError> + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            handler: Box::new(handler),
            delay,
            calls: Mutex::default(),
            restarts: Mutex::default(),
            events: Mutex::default(),
            statuses: Mutex::default(),
            n: AtomicUsize::new(0),
            hang: Mutex::default(),
            dropped: Arc::default(),
            hung: AtomicUsize::new(0),
            hung_entered: tokio::sync::Notify::new(),
        })
    }

    /// Answers every prompt with the string `"done:<prompt>"`.
    pub fn echo() -> Arc<Self> {
        Self::new(|r, _| Ok(done(json!(format!("done:{}", r.prompt)))))
    }

    /// Makes `spawn_and_wait` for this prompt never finish (until its future is dropped).
    /// Call it before the run starts. Calls for other prompts wait until every hung prompt
    /// has started, so tests do not depend on how the scheduler orders the steps.
    pub fn hang_on(&self, prompt: &str) {
        let mut hang = self.hang.lock().unwrap();
        if !hang.iter().any(|p| p == prompt) {
            hang.push(prompt.to_string());
        }
    }
    /// How many hung `spawn_and_wait` futures were dropped (cancelled) so far.
    pub fn dropped(&self) -> usize {
        self.dropped.load(Ordering::SeqCst)
    }
    pub fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }
    pub fn events(&self) -> Vec<String> {
        self.events.lock().unwrap().clone()
    }
    /// `(step_index, state)` of every `step_status` call.
    pub fn statuses(&self) -> Vec<(usize, String)> {
        self.statuses.lock().unwrap().clone()
    }
    /// Attempt numbers passed to `restart`.
    pub fn restarts(&self) -> Vec<u32> {
        self.restarts.lock().unwrap().clone()
    }
}

pub fn done(result: Value) -> StepOutcome {
    StepOutcome::Done {
        result,
        run_id: Some("run-1".into()),
    }
}

impl WorkflowHost for FakeHost {
    fn spawn_and_wait<'a>(
        &'a self,
        req: &'a StepRequest,
    ) -> BoxFut<'a, Result<StepOutcome, HostError>> {
        Box::pin(async move {
            let start = Instant::now();
            let n = self.n.fetch_add(1, Ordering::SeqCst);
            let (hangs, expected) = {
                let h = self.hang.lock().unwrap();
                (h.contains(&req.prompt), h.len())
            };
            if hangs {
                let _guard = DropCount(self.dropped.clone());
                self.hung.fetch_add(1, Ordering::SeqCst);
                self.hung_entered.notify_waiters();
                std::future::pending::<()>().await;
            }
            loop {
                let mut entered = std::pin::pin!(self.hung_entered.notified());
                entered.as_mut().enable();
                if self.hung.load(Ordering::SeqCst) >= expected {
                    break;
                }
                entered.await;
            }
            tokio::time::sleep(self.delay).await;
            self.calls.lock().unwrap().push(Call {
                prompt: req.prompt.clone(),
                opts: req.opts.clone(),
                start,
                end: Instant::now(),
            });
            (self.handler)(req, n)
        })
    }

    fn restart<'a>(
        &'a self,
        req: &'a StepRequest,
        _stalled: Option<&'a str>,
        attempt: u32,
    ) -> BoxFut<'a, Result<StepOutcome, HostError>> {
        Box::pin(async move {
            self.restarts.lock().unwrap().push(attempt);
            let n = self.n.fetch_add(1, Ordering::SeqCst);
            (self.handler)(req, n)
        })
    }

    fn log<'a>(&'a self, msg: &'a str) -> BoxFut<'a, ()> {
        Box::pin(async move { self.events.lock().unwrap().push(format!("log:{msg}")) })
    }
    fn phase_started<'a>(&'a self, name: &'a str) -> BoxFut<'a, ()> {
        Box::pin(async move { self.events.lock().unwrap().push(format!("start:{name}")) })
    }
    fn phase_finished<'a>(&'a self, name: &'a str) -> BoxFut<'a, ()> {
        Box::pin(async move { self.events.lock().unwrap().push(format!("end:{name}")) })
    }
    fn step_status<'a>(&'a self, e: &'a StepEvent) -> BoxFut<'a, ()> {
        Box::pin(async move {
            self.statuses
                .lock()
                .unwrap()
                .push((e.step_index, e.state.clone()))
        })
    }
}

struct DropCount(Arc<AtomicUsize>);

impl Drop for DropCount {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

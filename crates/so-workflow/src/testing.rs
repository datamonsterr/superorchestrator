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
        })
    }

    /// Answers every prompt with the string `"done:<prompt>"`.
    pub fn echo() -> Arc<Self> {
        Self::new(|r, _| Ok(done(json!(format!("done:{}", r.prompt)))))
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

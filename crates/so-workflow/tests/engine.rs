#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use chrono::Utc;
use serde_json::{Value, json};
use so_store::Store;
use so_workflow::testing::{FakeHost, done};
use so_workflow::{CancelToken, HostError, RunOptions, StepOutcome, WorkflowEngine, WorkflowError};
use tokio::time::Instant;

fn engine() -> (WorkflowEngine, Store) {
    let store = Store::open_in_memory().unwrap();
    (WorkflowEngine::new(store.clone()), store)
}

async fn run(host: &Arc<FakeHost>, src: &str) -> Result<Value, WorkflowError> {
    run_args(host, src, Value::Null).await
}

async fn run_args(host: &Arc<FakeHost>, src: &str, args: Value) -> Result<Value, WorkflowError> {
    engine().0.run(src, "wf", args, host.clone()).await
}

#[tokio::test]
async fn agent_returns_result() {
    let host = FakeHost::new(|_, _| Ok(done(json!({"files": ["a", "b"]}))));
    let out = run(
        &host,
        "const r = await agent('list', {model: 'm'}); return r.files.length;",
    )
    .await
    .unwrap();
    assert_eq!(out, json!(2));
    let calls = host.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].prompt, "list");
    assert_eq!(calls[0].opts, json!({"model": "m"}));
}

#[tokio::test]
async fn opts_accept_camel_case_and_aliases() {
    let host = FakeHost::echo();
    let src = "await agent('p', {outputSchema: {type: 'object', maxItems: 2}, maxTurns: 3, \
               stallMs: 5000, label: 'x'});";
    run(&host, src).await.unwrap();
    assert_eq!(
        host.calls()[0].opts,
        json!({
            "output_schema": {"type": "object", "maxItems": 2},
            "max_turns": 3, "stall_ms": 5000, "name": "x"
        })
    );
    let host = FakeHost::echo();
    run(&host, "await agent('p', {schema: {b: 2}});")
        .await
        .unwrap();
    assert_eq!(host.calls()[0].opts, json!({"output_schema": {"b": 2}}));
}

#[tokio::test]
async fn export_default_style() {
    let host = FakeHost::echo();
    let src = "export const meta = {name: 'x', description: 'd'}\n\
               export default async function (args) { return await agent('hi ' + args.who); }";
    let out = run_args(&host, src, json!({"who": "bob"})).await.unwrap();
    assert_eq!(out, json!("done:hi bob"));
}

#[tokio::test]
async fn args_global() {
    let host = FakeHost::echo();
    let out = run_args(
        &host,
        "return [args.a, args.b.c];",
        json!({"a": 1, "b": {"c": "x"}}),
    )
    .await
    .unwrap();
    assert_eq!(out, json!([1, "x"]));
    let out = run(&host, "return typeof args;").await.unwrap();
    assert_eq!(out, json!("undefined"));
}

#[tokio::test(start_paused = true)]
async fn parallel_runs_concurrently_and_defaults_worktree() {
    let host = FakeHost::with_delay(Duration::from_millis(100), |r, _| {
        Ok(done(json!(r.prompt.clone())))
    });
    let src = "return await parallel([\
        () => agent('a'), () => agent('b', {isolation: 'none'}), agent('c'), \
        async (ag) => await ag('d')]);";
    let t0 = Instant::now();
    let out = run(&host, src).await.unwrap();
    assert_eq!(out, json!(["a", "b", "c", "d"]));
    // Four 100 ms calls overlapped.
    assert_eq!(t0.elapsed(), Duration::from_millis(100));
    let calls = host.calls();
    let iso = |p: &str| {
        calls
            .iter()
            .find(|c| c.prompt == p)
            .unwrap()
            .opts
            .get("isolation")
            .cloned()
    };
    assert_eq!(iso("a"), Some(json!("worktree")));
    assert_eq!(iso("b"), Some(json!("none")));
    assert_eq!(
        iso("c"),
        None,
        "promises started before parallel() keep their options"
    );
    assert_eq!(iso("d"), Some(json!("worktree")));
    let first_end = calls.iter().map(|c| c.end).min().unwrap();
    assert!(calls.iter().all(|c| c.start < first_end), "calls overlap");
}

#[tokio::test]
async fn agent_outside_parallel_has_no_isolation_default() {
    let host = FakeHost::echo();
    run(&host, "await agent('x');").await.unwrap();
    assert_eq!(host.calls()[0].opts, json!({}));
}

#[tokio::test]
async fn parallel_nulls_failed_agents_but_propagates_script_errors() {
    let host = FakeHost::new(|r, _| {
        if r.prompt == "bad" {
            Err(HostError("boom".into()))
        } else {
            Ok(done(json!(r.prompt.clone())))
        }
    });
    let out = run(
        &host,
        "return await parallel([() => agent('ok'), () => agent('bad')]);",
    )
    .await
    .unwrap();
    assert_eq!(out, json!(["ok", null]));
    let err = run(
        &host,
        "return await parallel([() => { throw new Error('script bug'); }]);",
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("script bug"), "{err}");
    let err = run(&host, "return await agent('bad');").await.unwrap_err();
    assert!(err.to_string().contains("boom"), "{err}");
}

#[tokio::test]
async fn pipeline_chains_and_maps() {
    let host = FakeHost::echo();
    let chain = "return await pipeline([\
        () => agent('one'), (prev) => agent('two after ' + prev)]);";
    assert_eq!(
        run(&host, chain).await.unwrap(),
        json!("done:two after done:one")
    );
    let map = "return await pipeline(['x', 'y'], (f, i) => agent('audit ' + f + i));";
    assert_eq!(
        run(&host, map).await.unwrap(),
        json!(["done:audit x0", "done:audit y1"])
    );
}

#[tokio::test]
async fn phase_and_log_events() {
    let host = FakeHost::echo();
    let src = "log('before');\
        const r = await phase('plan', async () => { log('inside'); return await agent('p'); });\
        log('after'); return r;";
    let out = run(&host, src).await.unwrap();
    assert_eq!(out, json!("done:p"));
    assert_eq!(
        host.events(),
        [
            "log:before",
            "start:plan",
            "log:inside",
            "end:plan",
            "log:after"
        ]
    );
    let statuses = host.statuses();
    assert_eq!(
        statuses,
        [(0, "running".to_string()), (0, "succeeded".to_string())]
    );
}

#[tokio::test]
async fn records_run_and_steps_in_store() {
    let host = FakeHost::echo();
    let (eng, store) = engine();
    let src = "await phase('p1', () => agent('a')); await agent('b');";
    let (id, _) = eng
        .run_with(RunOptions::default(), src, "wf", json!({"k": 1}), host)
        .await
        .unwrap();
    let row = store.get_workflow_run(&id).unwrap().unwrap();
    assert_eq!(row.state, "succeeded");
    assert_eq!(row.args_json, r#"{"k":1}"#);
    assert!(row.finished_at.is_some());
    let steps = store.list_workflow_steps(&id).unwrap();
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[0].phase.as_deref(), Some("p1"));
    assert_eq!(steps[1].phase, None);
    assert!(
        steps
            .iter()
            .all(|s| s.state == "succeeded" && s.finished_at.is_some())
    );
    assert_eq!(steps[0].run_id.as_deref(), Some("run-1"));
}

#[tokio::test]
async fn resume_replays_cache_and_continues() {
    let (eng, store) = engine();
    let src = "const a = await agent('one'); const b = await agent('two'); \
               const c = await agent('three'); return [a, b, c];";
    let failing = FakeHost::new(|r, _| {
        if r.prompt == "three" {
            Err(HostError("quota".into()))
        } else {
            Ok(done(json!(format!("r:{}", r.prompt))))
        }
    });
    let (e1, h1) = (&eng, failing.clone());
    let err = e1.run(src, "wf", Value::Null, h1).await.unwrap_err();
    assert!(err.to_string().contains("quota"));
    assert_eq!(failing.calls().len(), 3);
    let runs = store.list_workflow_runs(5).unwrap();
    assert_eq!(runs[0].state, "failed");

    let ok = FakeHost::new(|r, _| Ok(done(json!(format!("r:{}", r.prompt)))));
    let out = eng.run(src, "wf", Value::Null, ok.clone()).await.unwrap();
    assert_eq!(out, json!(["r:one", "r:two", "r:three"]));
    let calls = ok.calls();
    assert_eq!(calls.len(), 1, "only the failed step runs again");
    assert_eq!(calls[0].prompt, "three");
    let newest = &store.list_workflow_runs(5).unwrap()[0];
    let states: Vec<_> = store
        .list_workflow_steps(&newest.id)
        .unwrap()
        .into_iter()
        .map(|s| s.state)
        .collect();
    assert_eq!(states, ["cached", "cached", "succeeded"]);

    // A third run is fully cached and never touches the host.
    let none = FakeHost::new(|_, _| panic!("host must not be called"));
    eng.run(src, "wf", Value::Null, none.clone()).await.unwrap();
    assert!(none.calls().is_empty());

    // The cache is per workflow name.
    let other = FakeHost::echo();
    eng.run(src, "other", Value::Null, other.clone())
        .await
        .unwrap();
    assert_eq!(other.calls().len(), 3);
}

#[tokio::test]
async fn identical_calls_get_distinct_cache_slots() {
    let (eng, _) = engine();
    let src = "return [await agent('same'), await agent('same')];";
    let counter = Arc::new(AtomicUsize::new(0));
    let c = counter.clone();
    let host = FakeHost::new(move |_, _| Ok(done(json!(c.fetch_add(1, Ordering::SeqCst)))));
    assert_eq!(
        eng.run(src, "wf", Value::Null, host.clone()).await.unwrap(),
        json!([0, 1])
    );
    let host2 = FakeHost::new(|_, _| panic!("cached"));
    assert_eq!(
        eng.run(src, "wf", Value::Null, host2).await.unwrap(),
        json!([0, 1])
    );
}

#[tokio::test]
async fn changed_opts_miss_the_cache() {
    let (eng, _) = engine();
    let host = FakeHost::echo();
    eng.run(
        "await agent('p', {model: 'a'});",
        "wf",
        Value::Null,
        host.clone(),
    )
    .await
    .unwrap();
    eng.run(
        "await agent('p', {model: 'b'});",
        "wf",
        Value::Null,
        host.clone(),
    )
    .await
    .unwrap();
    // Key order does not matter.
    eng.run(
        "await agent('p', {effort: 'x', model: 'a'});",
        "wf",
        Value::Null,
        host.clone(),
    )
    .await
    .unwrap();
    eng.run(
        "await agent('p', {model: 'a', effort: 'x'});",
        "wf",
        Value::Null,
        host.clone(),
    )
    .await
    .unwrap();
    assert_eq!(host.calls().len(), 3);
}

#[tokio::test(start_paused = true)]
async fn waits_until_retry_at_then_retries() {
    let host = FakeHost::new(|_, n| {
        if n == 0 {
            Ok(StepOutcome::Unavailable {
                retry_at: Utc::now() + chrono::Duration::minutes(10),
            })
        } else {
            Ok(done(json!("ok")))
        }
    });
    let t0 = Instant::now();
    let out = run(&host, "return await agent('x');").await.unwrap();
    assert_eq!(out, json!("ok"));
    let waited = t0.elapsed();
    assert!(
        waited >= Duration::from_secs(599) && waited <= Duration::from_secs(601),
        "{waited:?}"
    );
    assert_eq!(host.calls().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn retry_wait_is_capped_at_24h() {
    let host = FakeHost::new(|_, n| {
        if n == 0 {
            Ok(StepOutcome::Unavailable {
                retry_at: Utc::now() + chrono::Duration::days(7),
            })
        } else {
            Ok(done(json!("ok")))
        }
    });
    let t0 = Instant::now();
    run(&host, "await agent('x');").await.unwrap();
    assert_eq!(t0.elapsed(), Duration::from_secs(24 * 3600));
}

fn stalls(times: usize) -> Arc<FakeHost> {
    FakeHost::new(move |_, n| {
        if n < times {
            Ok(StepOutcome::Stalled {
                run_id: Some(format!("r{n}")),
            })
        } else {
            Ok(done(json!("finally")))
        }
    })
}

#[tokio::test]
async fn stalled_step_restarts_up_to_five_times() {
    let host = stalls(5);
    let out = run(&host, "return await agent('x', {stallMs: 1000});")
        .await
        .unwrap();
    assert_eq!(out, json!("finally"));
    assert_eq!(host.restarts(), [1, 2, 3, 4, 5]);
    assert_eq!(host.calls()[0].opts, json!({"stall_ms": 1000}));

    let host = stalls(6);
    let err = run(&host, "return await agent('x', {stallMs: 1000});")
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("stalled on all 6 attempts"),
        "{err}"
    );
    assert_eq!(host.restarts().len(), 5);
}

#[tokio::test]
async fn js_errors_become_err() {
    let host = FakeHost::echo();
    let e = run(&host, "throw new Error('kaboom');").await.unwrap_err();
    assert!(matches!(e, WorkflowError::Script(_)));
    assert!(e.to_string().contains("kaboom"), "{e}");
    let e = run(&host, "this is not js").await.unwrap_err();
    assert!(matches!(e, WorkflowError::Script(_)), "{e}");
    let e = run(&host, "await agent(); null.x").await.unwrap_err();
    assert!(e.to_string().contains("null"), "{e}");
    let (eng, store) = engine();
    let _ = eng.run("throw 1", "wf", Value::Null, host).await;
    assert_eq!(store.list_workflow_runs(1).unwrap()[0].state, "failed");
}

#[tokio::test]
async fn sandbox_exposes_no_host_apis_and_no_nondeterminism() {
    let host = FakeHost::echo();
    let src = "return ['require','process','fetch','std','os','XMLHttpRequest','__run','__begin']\
        .map(n => typeof globalThis[n]);";
    let out = run(&host, src).await.unwrap();
    assert_eq!(out, json!(vec!["undefined"; 8]));
    for expr in ["Date.now()", "Math.random()"] {
        let e = run(&host, &format!("return {expr};")).await.unwrap_err();
        assert!(e.to_string().contains("disabled"), "{e}");
    }
    let e = run(&host, "await import('fs');").await.unwrap_err();
    assert!(matches!(e, WorkflowError::Script(_)), "{e}");
}

#[tokio::test]
async fn memory_limit_stops_runaway_allocation() {
    let host = FakeHost::echo();
    let e = run(
        &host,
        "const a = []; while (true) a.push(new Array(100000).fill(1));",
    )
    .await
    .unwrap_err();
    assert!(matches!(e, WorkflowError::Script(_)), "{e}");
    assert!(e.to_string().to_lowercase().contains("memory"), "{e}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_interrupts_infinite_loop() {
    let host = FakeHost::echo();
    let cancel = CancelToken::new();
    let c = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        c.cancel();
    });
    let (eng, store) = engine();
    let opts = RunOptions {
        run_id: Some("wfr".into()),
        cancel,
    };
    let e = eng
        .run_with(opts, "while (true) {}", "wf", Value::Null, host)
        .await
        .unwrap_err();
    assert!(matches!(e, WorkflowError::Cancelled), "{e}");
    assert_eq!(
        store.get_workflow_run("wfr").unwrap().unwrap().state,
        "failed"
    );
}

#[tokio::test(start_paused = true)]
async fn cancel_interrupts_host_wait() {
    let host = FakeHost::with_delay(Duration::from_secs(3600), |_, _| Ok(done(json!(1))));
    let cancel = CancelToken::new();
    let c = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(1)).await;
        c.cancel();
    });
    let opts = RunOptions {
        run_id: None,
        cancel,
    };
    let e = engine()
        .0
        .run_with(opts, "await agent('slow');", "wf", Value::Null, host)
        .await
        .unwrap_err();
    assert!(matches!(e, WorkflowError::Cancelled), "{e}");
}

#[tokio::test]
async fn rerun_with_same_run_id_resets_state_and_upserts_steps() {
    let (eng, store) = engine();
    let src = "await agent('a'); await agent('b');";
    let mk = || RunOptions {
        run_id: Some("same".into()),
        cancel: CancelToken::new(),
    };
    let bad = FakeHost::new(|r, _| {
        if r.prompt == "b" {
            Err(HostError("no".into()))
        } else {
            Ok(done(json!(1)))
        }
    });
    eng.run_with(mk(), src, "wf", Value::Null, bad)
        .await
        .unwrap_err();
    assert_eq!(
        store.get_workflow_run("same").unwrap().unwrap().state,
        "failed"
    );
    eng.run_with(mk(), src, "wf", Value::Null, FakeHost::echo())
        .await
        .unwrap();
    assert_eq!(
        store.get_workflow_run("same").unwrap().unwrap().state,
        "succeeded"
    );
    let steps = store.list_workflow_steps("same").unwrap();
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[0].state, "cached");
    assert_eq!(steps[1].state, "succeeded");
}

#[tokio::test]
async fn date_without_args_throws_but_explicit_dates_work() {
    let host = FakeHost::echo();
    for expr in ["new Date()", "Date()"] {
        let e = run(&host, &format!("return String({expr});"))
            .await
            .unwrap_err();
        assert!(e.to_string().contains("disabled"), "{e}");
    }
    let out = run_args(
        &host,
        "return [new Date(0).toISOString(), Date.UTC(1970,0,2), new Date(args.t).getTime(), \
         new Date(2020, 0, 1) instanceof Date, typeof Date(0)];",
        json!({"t": 5}),
    )
    .await
    .unwrap();
    assert_eq!(
        out,
        json!(["1970-01-01T00:00:00.000Z", 86_400_000, 5, true, "string"])
    );
}

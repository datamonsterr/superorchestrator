#![allow(clippy::unwrap_used)]
//! Workflows (SPEC 11) through `run_workflow` and `workflow_status` over the supervisor
//! socket, with the scripted fake harness behind every step.

mod common;

use std::time::Duration;

use chrono::Utc;
use common::*;
use serde_json::{Value, json};
use so_core::proto::{
    DEPTH_LIMIT, METHOD_RUN_WORKFLOW, METHOD_WORKFLOW_STATUS, NOT_FOUND, RunWorkflowResult,
    WorkflowStatusResult,
};
use so_core::{Choice, Harness, RunState};
use so_store::WorkflowRunRow;
use so_supervisor::SupervisorClient;

fn env() -> Env {
    let mut env = Env::new();
    env.config.choices = vec![Choice {
        id: "claude-mid".into(),
        harness: Harness::Claude,
        model: "claude-sonnet-5-5".into(),
        effort: None,
        roles: vec!["worker".into()],
        description: "d".into(),
        enabled: true,
    }];
    env.config.router.jev.key_env = "SO_WORKFLOW_TEST_NO_SUCH_KEY".into();
    env
}

const TWO_STEPS: &str = r#"
let out;
await phase('build', async () => {
  const a = await agent("do the task", {harness: "claude", isolation: "none"});
  const b = await agent("do the task", {harness: "claude", account: "claude-a", isolation: "none", model: "claude-opus-9"});
  out = [a, b];
});
log("both done");
return out;
"#;

async fn run(c: &SupervisorClient, params: Value) -> RunWorkflowResult {
    c.call(METHOD_RUN_WORKFLOW, params).await.unwrap()
}

async fn status(c: &SupervisorClient, id: &str) -> WorkflowStatusResult {
    c.call(METHOD_WORKFLOW_STATUS, json!({"id": id}))
        .await
        .unwrap()
}

fn step_states(s: &WorkflowStatusResult) -> Vec<&str> {
    s.steps.iter().map(|x| x.state.as_str()).collect()
}

#[tokio::test]
async fn a_two_step_workflow_runs_end_to_end_and_reports_progress() {
    let env = env();
    env.account("claude-a", 2, &ok_script(&[]));
    let h = env.start().await;
    let c = env.client(&h).await;
    let mut notes = c.notifications().unwrap();

    let r = run(&c, json!({"script": TWO_STEPS})).await;
    assert_eq!(r.state.as_deref(), Some("succeeded"), "{r:?}");
    assert_eq!(r.result, Some(json!(["done", "done"])));

    let s = status(&c, &r.id).await;
    assert_eq!(s.state, "succeeded");
    assert_eq!(s.result, r.result);
    assert!(s.name.starts_with("inline-"), "{}", s.name);
    assert_eq!(step_states(&s), ["succeeded", "succeeded"]);
    assert!(s.steps.iter().all(|x| x.name == "build"));
    assert_eq!(s.steps[1].index, 1);
    // Each step was a run of the caller's session.
    for step in &s.steps {
        let v = view(&c, step.run_id.as_deref().unwrap()).await;
        assert_eq!(v.state, RunState::Succeeded);
        assert_eq!(v.depth, 1);
        assert_eq!(v.session_id, s.session_id.clone().unwrap());
    }
    // A prefix of the id is enough.
    assert_eq!(status(&c, &r.id[..12]).await.id, r.id);

    // The caller was told as it went: step states, the phase and the log line.
    let mut progress = Vec::new();
    while let Ok(n) = notes.try_recv() {
        if n.method == "event" && n.params["type"] == "progress" {
            progress.push((
                n.params["data"]["step"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                n.params["data"]["status"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            ));
        }
    }
    let has = |step: &str, status: &str| progress.iter().any(|p| p.0 == step && p.1 == status);
    assert!(has("build", "started"), "{progress:?}");
    assert!(has("build", "running"), "{progress:?}");
    assert!(has("build", "succeeded"), "{progress:?}");
    assert!(has("build", "finished"), "{progress:?}");
    assert!(has("log", "both done"), "{progress:?}");
    h.shutdown().await;
}

#[tokio::test]
async fn without_wait_the_id_comes_back_at_once_and_status_follows_progress() {
    let env = env();
    env.account("claude-a", 2, &ok_script(&[json!({"sleep_ms": 700})]));
    let h = env.start().await;
    let c = env.client(&h).await;

    let script = r#"return await agent("do the task", {harness: "claude", isolation: "none"});"#;
    let r = run(&c, json!({"script": script, "wait": false})).await;
    assert_eq!(r.state.as_deref(), Some("running"));
    assert!(r.result.is_none());
    let early = status(&c, &r.id).await;
    assert_eq!(early.state, "running");
    // Soon the step is listed with its run while it works.
    let mut saw_running_step = false;
    let mut last = early;
    for _ in 0..200 {
        last = status(&c, &r.id).await;
        saw_running_step |= last
            .steps
            .iter()
            .any(|s| s.state == "running" && s.run_id.is_some());
        if last.state != "running" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(saw_running_step, "{last:?}");
    assert_eq!(last.state, "succeeded");
    assert_eq!(last.result, Some(json!("done")));
    h.shutdown().await;
}

#[tokio::test]
async fn a_relaunched_workflow_replays_cached_steps_after_a_restart() {
    let env = env();
    env.account("claude-a", 2, &ok_script(&[]));
    // The second step's account fails the first time round.
    env.account(
        "claude-b",
        2,
        &script(&[
            json!({"expect_stdin_contains": "second"}),
            json!({"stdout": init_msg()}),
            json!({"stdout": result_msg(json!({"is_error": true, "result": "the build exploded"}))}),
        ]),
    );
    let script_src = r#"
const a = await agent("do the task", {harness: "claude", account: "claude-a", isolation: "none"});
const b = await agent("second step", {harness: "claude", account: "claude-b", isolation: "none"});
return [a, b];
"#;
    let h = env.start().await;
    let c = env.client(&h).await;
    let first = run(&c, json!({"script": script_src})).await;
    assert_eq!(first.state.as_deref(), Some("failed"), "{first:?}");
    assert!(
        first.error.as_deref().unwrap().contains("failed"),
        "{first:?}"
    );
    let s = status(&c, &first.id).await;
    assert_eq!(step_states(&s), ["succeeded", "failed"]);
    h.shutdown().await;

    // A new supervisor over the same state; the account now works.
    env.account(
        "claude-b",
        2,
        &script(&[
            json!({"expect_stdin_contains": "second"}),
            json!({"stdout": init_msg()}),
            json!({"stdout": result_msg(json!({"result": "fixed"}))}),
        ]),
    );
    let h = env.start().await;
    let c = env.client(&h).await;
    let second = run(&c, json!({"script": script_src})).await;
    assert_eq!(second.state.as_deref(), Some("succeeded"), "{second:?}");
    assert_eq!(second.result, Some(json!(["done", "fixed"])));
    let s = status(&c, &second.id).await;
    assert_eq!(step_states(&s), ["cached", "succeeded"]);
    // The first step ran once in total; only the failed one ran again.
    assert_eq!(env.launches("claude-a").len(), 1);
    assert_eq!(env.launches("claude-b").len(), 2);
    h.shutdown().await;
}

#[tokio::test]
async fn a_workflow_left_running_by_a_dead_supervisor_is_failed_on_start() {
    let env = env();
    let store = env.store();
    store
        .insert_workflow_run(&WorkflowRunRow {
            id: "wf-orphan".into(),
            name: "w".into(),
            session_id: None,
            state: "running".into(),
            args_json: "null".into(),
            started_at: Utc::now(),
            finished_at: None,
            result_json: None,
            error: None,
        })
        .unwrap();
    let h = env.start().await;
    let c = env.inspect_client(&h).await;
    let s = status(&c, "wf-orphan").await;
    assert_eq!(s.state, "failed");
    assert!(s.error.unwrap().contains("supervisor stopped"));
    h.shutdown().await;
}

#[tokio::test]
async fn a_stalled_step_is_restarted_a_bounded_number_of_times() {
    let env = env();
    // The harness takes the prompt and then says nothing.
    env.account(
        "claude-a",
        // Room for the stalled runs that are still being cancelled.
        8,
        &script(&[json!({"expect_stdin_contains": PROMPT})]),
    );
    let h = env.start().await;
    let c = env.client(&h).await;
    let script_src = r#"return await agent("do the task", {harness: "claude", isolation: "none", stallMs: 250});"#;
    let r = run(&c, json!({"script": script_src})).await;
    assert_eq!(r.state.as_deref(), Some("failed"), "{r:?}");
    assert!(r.error.unwrap().contains("stalled on all 6 attempts"));
    // The first start and five restarts, every stalled run cancelled.
    assert_eq!(env.launches("claude-a").len(), 6);
    let s = status(&c, &r.id).await;
    assert_eq!(step_states(&s), ["failed"]);
    let store = env.store();
    let session = s.session_id.unwrap();
    let mut states = Vec::new();
    for _ in 0..400 {
        states = store
            .list_runs_by_session(&session, 100)
            .unwrap()
            .iter()
            .map(|r| r.state)
            .collect();
        if states.iter().all(|s| *s == RunState::Cancelled) && states.len() == 6 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(states, vec![RunState::Cancelled; 6]);
    h.shutdown().await;
}

#[tokio::test]
async fn a_step_waits_for_an_account_to_come_back() {
    let env = env();
    env.account("claude-a", 2, &ok_script(&[]));
    let h = env.start().await;
    let c = env.client(&h).await;
    // The only account is in cooldown for two seconds.
    let until = Utc::now() + chrono::Duration::seconds(2);
    so_usage::mark_exhausted(
        &env.store(),
        "claude-a",
        "default",
        Some(until),
        Duration::from_secs(3600),
        "test",
        Utc::now(),
    )
    .unwrap();
    let script_src =
        r#"return await agent("do the task", {harness: "claude", isolation: "none"});"#;
    let r = run(&c, json!({"script": script_src, "wait": false})).await;
    let waiting = async {
        for _ in 0..100 {
            let s = status(&c, &r.id).await;
            if s.steps.iter().any(|x| x.state == "waiting") {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        false
    };
    assert!(waiting.await, "the step never reported waiting");
    let mut s = status(&c, &r.id).await;
    for _ in 0..200 {
        if s.state != "running" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        s = status(&c, &r.id).await;
    }
    assert_eq!(s.state, "succeeded", "{s:?}");
    assert!(Utc::now() >= until, "ran before the reset");
    h.shutdown().await;
}

#[tokio::test]
async fn saved_workflows_run_by_name_and_unknown_ones_are_not_found() {
    let env = env();
    env.account("claude-a", 2, &ok_script(&[]));
    std::fs::create_dir_all(env.paths.workflows_dir()).unwrap();
    std::fs::write(
        env.paths.workflows_dir().join("hello.js"),
        "return {greeting: args.who, got: await agent(\"do the task\", {harness: 'claude', isolation: 'none'})};",
    )
    .unwrap();
    let h = env.start().await;
    let c = env.client(&h).await;
    let r = run(&c, json!({"name": "hello", "args": {"who": "you"}})).await;
    assert_eq!(r.state.as_deref(), Some("succeeded"), "{r:?}");
    assert_eq!(r.result, Some(json!({"greeting": "you", "got": "done"})));
    assert_eq!(status(&c, &r.id).await.name, "hello");

    let err = c
        .call::<_, Value>(METHOD_RUN_WORKFLOW, json!({"name": "nope"}))
        .await
        .unwrap_err();
    assert_eq!(rpc_code(err), NOT_FOUND);
    let err = c
        .call::<_, Value>(METHOD_RUN_WORKFLOW, json!({}))
        .await
        .unwrap_err();
    assert_eq!(rpc_code(err), so_core::proto::INVALID_PARAMS);
    h.shutdown().await;
}

#[tokio::test]
async fn a_depth_two_run_cannot_start_a_workflow() {
    let env = env();
    env.account("claude-a", 2, &ok_script(&[]));
    let h = env.start().await;
    let deep = insert_session_and_run(&env.store(), 2, RunState::Running);
    let c = SupervisorClient::connect(h.socket_path()).await.unwrap();
    hello_run(&c, &env.store(), &env.paths, &deep.id).await;
    let err = c
        .call::<_, Value>(METHOD_RUN_WORKFLOW, json!({"script": "return 1;"}))
        .await
        .unwrap_err();
    assert_eq!(rpc_code(err), DEPTH_LIMIT);
    h.shutdown().await;
}

fn idle_script() -> String {
    script(&[json!({"expect_stdin_contains": PROMPT})])
}

async fn wait_workflow(c: &SupervisorClient, id: &str, want: &str) -> WorkflowStatusResult {
    let mut s = status(c, id).await;
    for _ in 0..400 {
        if s.state == want {
            return s;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
        s = status(c, id).await;
    }
    panic!("workflow {id} never became {want}: {s:?}");
}

#[tokio::test]
async fn a_session_with_a_workflow_between_steps_stays_open() {
    let env = env();
    env.account("claude-a", 2, &ok_script(&[]));
    let h = env.start().await;
    // The only account is out for two seconds, so the second step waits with no run active.
    so_usage::mark_exhausted(
        &env.store(),
        "claude-a",
        "default",
        Some(Utc::now() + chrono::Duration::seconds(2)),
        Duration::from_secs(3600),
        "t",
        Utc::now(),
    )
    .unwrap();
    let script_src = r#"
await agent("do the task", {harness: "claude", account: "claude-a", isolation: "none"});
return await agent("do the task", {harness: "claude", isolation: "none"});
"#;
    let (id, session) = {
        let c = env.client(&h).await;
        let r = run(&c, json!({"script": script_src, "wait": false})).await;
        (r.id, r.session_id.unwrap())
        // The client goes away here.
    };
    tokio::time::sleep(Duration::from_millis(900)).await;
    let store = env.store();
    let row = store.get_session(&session).unwrap().unwrap();
    assert!(
        row.ended_at.is_none(),
        "the session ended mid-workflow: {row:?}"
    );
    assert!(h.state().busy(), "idle exit must wait for the workflow");

    // It finishes after the reset, and only then does the session end.
    let c = env.inspect_client(&h).await;
    let s = wait_workflow(&c, &id, "succeeded").await;
    assert_eq!(s.result, Some(json!("done")));
    for _ in 0..200 {
        if store
            .get_session(&session)
            .unwrap()
            .unwrap()
            .ended_at
            .is_some()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let fin = store.get_session(&session).unwrap().unwrap();
    assert!(
        fin.ended_at.is_some(),
        "{fin:?} wf={:?} running={}",
        store.get_workflow_run(&id).unwrap().unwrap().state,
        store.session_has_running_workflow(&session).unwrap()
    );
    h.shutdown().await;
}

#[tokio::test]
async fn cancel_workflow_stops_it_and_its_step_runs() {
    let env = env();
    env.account("claude-a", 2, &idle_script());
    let h = env.start().await;
    let c = env.client(&h).await;
    let script_src = r#"return await agent("do the task", {harness: "claude", account: "claude-a", isolation: "none"});"#;
    let r = run(&c, json!({"script": script_src, "wait": false})).await;
    let mut run_id = None;
    for _ in 0..400 {
        run_id = status(&c, &r.id)
            .await
            .steps
            .first()
            .and_then(|s| s.run_id.clone());
        if run_id.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let run_id = run_id.expect("a step run");
    let ack: Value = c
        .call(so_core::proto::METHOD_CANCEL_WORKFLOW, json!({"id": r.id}))
        .await
        .unwrap();
    assert_eq!(ack["ok"], true);
    let s = wait_workflow(&c, &r.id, "cancelled").await;
    assert_eq!(step_states(&s), ["cancelled"]);
    wait_state(&c, &run_id, RunState::Cancelled).await;
    // A finished workflow cannot be cancelled again.
    let err = c
        .call::<_, Value>(so_core::proto::METHOD_CANCEL_WORKFLOW, json!({"id": r.id}))
        .await
        .unwrap_err();
    assert_eq!(rpc_code(err), so_core::proto::INVALID_PARAMS);
    h.shutdown().await;
}

#[tokio::test]
async fn a_waiting_caller_that_goes_away_cancels_the_workflow() {
    let env = env();
    env.account("claude-a", 2, &idle_script());
    let h = env.start().await;
    let script_src = r#"return await agent("do the task", {harness: "claude", account: "claude-a", isolation: "none"});"#;
    let watcher = env.inspect_client(&h).await;
    let caller = env.client(&h).await;
    let call = tokio::spawn(async move {
        let _ = caller
            .call::<_, Value>(METHOD_RUN_WORKFLOW, json!({"script": script_src}))
            .await;
    });
    let mut id = None;
    for _ in 0..400 {
        let runs = env.store().list_workflow_runs(5).unwrap();
        if let Some(r) = runs.first() {
            id = Some(r.id.clone());
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let id = id.expect("a workflow run");
    // Dropping the call (and with it the connection) stops the workflow.
    call.abort();
    let _ = call.await;
    wait_workflow(&watcher, &id, "cancelled").await;
    h.shutdown().await;
}

#[tokio::test]
async fn a_script_that_never_yields_is_cut_off_at_max_duration() {
    let mut env = env();
    env.config.workflows.max_duration = so_core::duration::HumanDuration::from_millis(700);
    env.account("claude-a", 2, &ok_script(&[]));
    let h = env.start().await;
    let c = env.client(&h).await;
    let r = run(&c, json!({"script": "while (true) {}"})).await;
    assert_eq!(r.state.as_deref(), Some("failed"), "{r:?}");
    assert!(r.error.unwrap().contains("maximum duration"));
    // The row is not left running.
    assert_eq!(status(&c, &r.id).await.state, "failed");
    h.shutdown().await;
}

#[tokio::test]
async fn fresh_runs_every_step_again() {
    let env = env();
    env.account("claude-a", 2, &ok_script(&[]));
    let h = env.start().await;
    let c = env.client(&h).await;
    let src = r#"return await agent("do the task", {harness: "claude", account: "claude-a", isolation: "none"});"#;
    run(&c, json!({"script": src})).await;
    run(&c, json!({"script": src})).await;
    assert_eq!(
        env.launches("claude-a").len(),
        1,
        "the second run replayed the cache"
    );
    let r = run(&c, json!({"script": src, "fresh": true})).await;
    assert_eq!(r.state.as_deref(), Some("succeeded"));
    assert_eq!(env.launches("claude-a").len(), 2);
    h.shutdown().await;
}

#[tokio::test]
async fn steps_over_the_parents_child_limit_wait_for_one_to_finish() {
    let mut env = env();
    env.config.limits.max_children = 1;
    env.account("claude-a", 4, &ok_script(&[json!({"sleep_ms": 300})]));
    let h = env.start().await;
    let parent = insert_session_and_run(&env.store(), 1, RunState::Running);
    let c = SupervisorClient::connect(h.socket_path()).await.unwrap();
    hello_run(&c, &env.store(), &env.paths, &parent.id).await;
    let src = r#"
return await parallel([
  (a) => a("do the task", {harness: "claude", account: "claude-a", isolation: "none", model: "m1"}),
  (a) => a("do the task", {harness: "claude", account: "claude-a", isolation: "none", model: "m2"}),
]);
"#;
    let r = run(&c, json!({"script": src})).await;
    assert_eq!(r.state.as_deref(), Some("succeeded"), "{r:?}");
    assert_eq!(r.result, Some(json!(["done", "done"])));
    h.shutdown().await;
}

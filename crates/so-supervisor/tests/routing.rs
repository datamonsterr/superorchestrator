#![allow(clippy::unwrap_used)]
//! Auto routing and failover (SPEC 8.3, 8.5) against the scripted fake harness.

mod common;

use common::*;
use serde_json::{Value, json};
use so_core::proto::{METHOD_CANCEL_AGENT, RouteResult, RouteSource, RunRef, RunView};
use so_core::{Choice, ErrorKind, Harness, Isolation, RunState};

fn choice(id: &str, model: &str) -> Choice {
    Choice {
        id: id.into(),
        harness: Harness::Claude,
        model: model.into(),
        effort: Some("medium".into()),
        roles: vec!["worker".into()],
        description: "test choice".into(),
        enabled: true,
    }
}

/// Claude accounts only: no decider key, one choice.
fn routed_env() -> Env {
    let mut env = Env::new();
    env.config.choices = vec![choice("claude-mid", "claude-sonnet-5-5")];
    env.config.router.jev.key_env = "SO_ROUTING_TEST_NO_SUCH_KEY".into();
    env
}

fn quota_script() -> String {
    script(&[
        json!({"expect_stdin_contains": PROMPT}),
        json!({"stdout": init_msg()}),
        json!({"stdout": result_msg(json!({"is_error": true, "result": "Claude AI usage limit reached"}))}),
    ])
}

/// A harness that takes the prompt and then keeps the run alive until it is cancelled.
fn idle_script() -> String {
    script(&[json!({"expect_stdin_contains": PROMPT})])
}

fn auto_params(env: &Env) -> so_core::proto::SpawnParams {
    let mut p = env.spawn_params("unused");
    p.harness = None;
    p.account = None;
    p.isolation = Some(Isolation::None);
    p.wait = true;
    p
}

fn events<'a>(v: &'a RunView, name: &str) -> Vec<&'a so_core::Event> {
    v.recent_events
        .iter()
        .filter(|e| e.event_type.as_str() == name)
        .collect()
}

#[tokio::test]
async fn auto_routing_picks_an_account_and_records_the_decision() {
    let env = routed_env();
    env.account("claude-a", 2, &ok_script(&[]));
    env.account("claude-b", 2, &ok_script(&[]));
    let h = env.start().await;
    let c = env.client(&h).await;

    let r = spawn(&c, auto_params(&env)).await;
    assert_eq!(r.state, RunState::Succeeded, "{r:?}");
    let v = view(&c, &r.run_id).await;
    assert_eq!(v.harness, Some(Harness::Claude));
    assert_eq!(v.account.as_deref(), Some("claude-a"));
    assert_eq!(v.model.as_deref(), Some("claude-sonnet-5-5"));

    // The decision is stored in the shape the web API parses (a `RouteResult`).
    let ev = events(&v, "route_decision");
    assert_eq!(ev.len(), 1, "{:?}", types(&v));
    let result: RouteResult = serde_json::from_value(ev[0].data.clone()).unwrap();
    assert_eq!(result.choice_id, "claude-mid");
    assert_eq!(result.account, "claude-a");
    assert_eq!(result.source, RouteSource::Heuristic);
    assert_eq!(result.ranked_accounts, vec!["claude-a", "claude-b"]);
    assert!(
        !ev[0].data.to_string().contains(PROMPT),
        "no task text in the decision"
    );
    // The launch used the choice's model and effort.
    let argv = &env.launches("claude-a")[0];
    let at = |flag: &str| {
        argv.iter()
            .position(|a| a == flag)
            .map(|i| argv[i + 1].clone())
    };
    assert_eq!(at("--model").as_deref(), Some("claude-sonnet-5-5"));
    assert_eq!(at("--effort").as_deref(), Some("medium"));
    h.shutdown().await;
}

#[tokio::test]
async fn pinned_fields_are_respected() {
    let env = routed_env();
    env.account("claude-a", 2, &ok_script(&[]));
    env.account("claude-b", 2, &ok_script(&[]));
    let h = env.start().await;
    let c = env.client(&h).await;

    // An account named without a harness: the router keeps to it.
    let mut p = auto_params(&env);
    p.account = Some("claude-b".into());
    let r = spawn(&c, p).await;
    assert_eq!(r.state, RunState::Succeeded, "{r:?}");
    assert_eq!(
        view(&c, &r.run_id).await.account.as_deref(),
        Some("claude-b")
    );

    // A harness, model and effort named: they reach the harness unchanged, even though no
    // choice has that model.
    let mut p = auto_params(&env);
    p.harness = Some("claude".into());
    p.model = Some("claude-opus-9".into());
    p.effort = Some("high".into());
    let r = spawn(&c, p).await;
    assert_eq!(r.state, RunState::Succeeded, "{r:?}");
    let v = view(&c, &r.run_id).await;
    assert_eq!(v.model.as_deref(), Some("claude-opus-9"));
    let route: RouteResult =
        serde_json::from_value(events(&v, "route_decision")[0].data.clone()).unwrap();
    assert_eq!(route.source, RouteSource::Pinned);
    let argv = env.launches(v.account.as_deref().unwrap()).pop().unwrap();
    assert!(argv.windows(2).any(|w| w == ["--model", "claude-opus-9"]));
    assert!(argv.windows(2).any(|w| w == ["--effort", "high"]));

    // `harness: auto` is the same as leaving it out.
    let mut p = auto_params(&env);
    p.harness = Some("auto".into());
    assert_eq!(spawn(&c, p).await.state, RunState::Succeeded);

    // Harness and account both named: nothing is routed, nothing is recorded.
    let mut p = env.spawn_params("claude-b");
    p.isolation = Some(Isolation::None);
    p.wait = true;
    let r = spawn(&c, p).await;
    let v = view(&c, &r.run_id).await;
    assert_eq!(v.account.as_deref(), Some("claude-b"));
    assert!(events(&v, "route_decision").is_empty());
    h.shutdown().await;
}

#[tokio::test]
async fn a_harness_pin_works_without_a_matching_choice() {
    let mut env = routed_env();
    env.config.choices.clear();
    env.account("claude-a", 2, &ok_script(&[]));
    let h = env.start().await;
    let c = env.client(&h).await;
    let mut p = auto_params(&env);
    p.harness = Some("claude".into());
    let r = spawn(&c, p).await;
    assert_eq!(r.state, RunState::Succeeded, "{r:?}");
    let v = view(&c, &r.run_id).await;
    assert_eq!(v.account.as_deref(), Some("claude-a"));
    // With no harness named and no choice, routing says why nothing can run.
    let err = c
        .call::<_, Value>(so_core::proto::METHOD_SPAWN_AGENT, auto_params(&env))
        .await
        .unwrap_err();
    assert_eq!(rpc_code(err), so_core::proto::UNAVAILABLE);
    h.shutdown().await;
}

#[tokio::test]
async fn quota_error_fails_over_to_the_next_account() {
    let env = routed_env();
    env.account("claude-a", 2, &quota_script());
    env.account("claude-b", 2, &ok_script(&[]));
    let h = env.start().await;
    let c = env.client(&h).await;

    let r = spawn(&c, auto_params(&env)).await;
    assert_eq!(r.state, RunState::Succeeded, "{r:?}");
    let v = view(&c, &r.run_id).await;
    assert_eq!(v.account.as_deref(), Some("claude-b"));
    assert_eq!(v.result, Some(json!("done")));

    let f = events(&v, "failover");
    assert_eq!(f.len(), 1, "{:?}", types(&v));
    assert_eq!(f[0].data["from"], "claude-a");
    assert_eq!(f[0].data["to"], "claude-b");
    assert_eq!(f[0].data["error_kind"], "quota");
    // The new attempt follows the failover, and the second decision is recorded.
    let t = types(&v);
    let pos = |n: &str| t.iter().position(|x| *x == n).unwrap();
    assert!(pos("failover") < t.iter().rposition(|x| *x == "attempt_started").unwrap());
    assert_eq!(events(&v, "route_decision").len(), 2);

    // Two attempts on two accounts, and the first account is out of rotation.
    let store = env.store();
    let attempts = store.list_attempts(&r.run_id, 10).unwrap();
    let on: Vec<(&str, Option<&str>)> = attempts
        .iter()
        .map(|a| (a.account.as_str(), a.outcome.as_deref()))
        .collect();
    assert_eq!(
        on,
        vec![
            ("claude-a", Some("failover")),
            ("claude-b", Some("succeeded"))
        ]
    );
    assert!(store.get_flag("claude-a", "default").unwrap().is_some());
    assert!(store.get_flag("claude-b", "default").unwrap().is_none());
    // No slot is left behind on either account.
    assert_eq!(store.count_active_runs_by_account("claude-a").unwrap(), 0);
    assert_eq!(store.count_active_runs_by_account("claude-b").unwrap(), 0);
    // The run kept its id; the second account was launched once, the first once.
    assert_eq!(env.launches("claude-a").len(), 1);
    assert_eq!(env.launches("claude-b").len(), 1);
    // The new account was told what happened, after the original task.
    let said = env.stdin_lines("claude-b");
    assert!(
        said[0].contains(PROMPT) && said[0].contains("ran out of quota"),
        "{said:?}"
    );
    h.shutdown().await;
}

#[tokio::test]
async fn failover_stops_when_no_account_is_left() {
    let env = routed_env();
    env.account("claude-a", 2, &quota_script());
    env.account("claude-b", 2, &quota_script());
    let h = env.start().await;
    let c = env.client(&h).await;

    let r = spawn(&c, auto_params(&env)).await;
    assert_eq!(r.state, RunState::Failed, "{r:?}");
    assert_eq!(r.error_kind, Some(ErrorKind::Quota));
    let v = view(&c, &r.run_id).await;
    let reason = events(&v, "run_finished")[0].data["reason"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        reason.contains("no account") && reason.contains("claude-a, claude-b"),
        "{reason}"
    );
    assert_eq!(events(&v, "failover").len(), 1);
    let store = env.store();
    assert_eq!(store.list_attempts(&r.run_id, 10).unwrap().len(), 2);
    assert_eq!(store.count_active_runs_by_account("claude-b").unwrap(), 0);
    h.shutdown().await;
}

#[tokio::test]
async fn failover_with_every_target_full_waits_in_the_queue_and_starts_on_the_freed_account() {
    let env = routed_env();
    env.account("claude-a", 2, &quota_script());
    env.account("claude-b", 1, &idle_script());
    let h = env.start().await;
    let c = env.client(&h).await;

    // `claude-b` is full with a run of someone else.
    let mut occupant = env.spawn_params("claude-b");
    occupant.isolation = Some(Isolation::None);
    let occupant = spawn(&c, occupant).await;
    wait_state(&c, &occupant.run_id, RunState::Running).await;

    let mut p = auto_params(&env);
    p.wait = false;
    let r = spawn(&c, p).await;
    // It hits the limit on `claude-a` and, with nowhere to go, waits with its failover pending.
    wait_state(&c, &r.run_id, RunState::Queued).await;
    let v = view(&c, &r.run_id).await;
    assert_eq!(v.account.as_deref(), Some("claude-b"));
    assert!(
        events(&v, "failover").is_empty(),
        "announced only once it starts"
    );
    assert!(
        env.store()
            .get_failover_pending(&r.run_id)
            .unwrap()
            .is_some()
    );
    assert_eq!(
        env.store()
            .count_active_runs_by_account("claude-a")
            .unwrap(),
        0
    );

    // The occupant ends; the queued run starts on the freed account, fresh, with the note.
    // (An account's native session is not portable to another account.)
    let _: RunView = c
        .call(
            METHOD_CANCEL_AGENT,
            RunRef {
                run: occupant.run_id,
            },
        )
        .await
        .unwrap();
    wait_state(&c, &r.run_id, RunState::Running).await;
    let v = view(&c, &r.run_id).await;
    let f = events(&v, "failover");
    assert_eq!(f.len(), 1, "{:?}", types(&v));
    assert_eq!(
        (f[0].data["from"].as_str(), f[0].data["to"].as_str()),
        (Some("claude-a"), Some("claude-b"))
    );
    assert!(
        env.store()
            .get_failover_pending(&r.run_id)
            .unwrap()
            .is_none()
    );
    let said = env.stdin_lines("claude-b");
    assert!(
        said.iter().any(|l| l.contains("ran out of quota")),
        "{said:?}"
    );
    let attempts = env.store().list_attempts(&r.run_id, 10).unwrap();
    assert_eq!(attempts[0].outcome.as_deref(), Some("failover"));
    assert_eq!(attempts.len(), 2);
    let _: RunView = c
        .call(METHOD_CANCEL_AGENT, RunRef { run: r.run_id })
        .await
        .unwrap();
    h.shutdown().await;
}

#[tokio::test]
async fn an_over_cap_auto_spawn_moves_to_the_account_that_frees_up() {
    let env = routed_env();
    env.account("claude-a", 1, &idle_script());
    env.account("claude-b", 1, &idle_script());
    let h = env.start().await;
    let c = env.client(&h).await;
    let mut hold = |acct: &str| {
        let mut p = env.spawn_params(acct);
        p.isolation = Some(Isolation::None);
        p
    };
    let r1 = spawn(&c, hold("claude-a")).await;
    let r2 = spawn(&c, hold("claude-b")).await;
    wait_state(&c, &r1.run_id, RunState::Running).await;
    wait_state(&c, &r2.run_id, RunState::Running).await;

    // Both accounts are full: the spawn queues (on the best one, `claude-a`).
    let mut p = auto_params(&env);
    p.wait = false;
    let q = spawn(&c, p).await;
    assert_eq!(q.state, RunState::Queued);
    assert_eq!(
        view(&c, &q.run_id).await.account.as_deref(),
        Some("claude-a")
    );

    // `claude-b` frees up first: the queued run takes it instead of waiting for `claude-a`.
    let _: RunView = c
        .call(METHOD_CANCEL_AGENT, RunRef { run: r2.run_id })
        .await
        .unwrap();
    wait_state(&c, &q.run_id, RunState::Running).await;
    let v = view(&c, &q.run_id).await;
    assert_eq!(v.account.as_deref(), Some("claude-b"));
    assert_eq!(
        events(&v, "route_decision").len(),
        2,
        "the move is recorded"
    );
    assert_eq!(
        env.store()
            .count_active_runs_by_account("claude-a")
            .unwrap(),
        1
    );
    for r in [q.run_id, r1.run_id] {
        let _: RunView = c
            .call(METHOD_CANCEL_AGENT, RunRef { run: r })
            .await
            .unwrap();
    }
    h.shutdown().await;
}

#[tokio::test]
async fn a_resumed_run_fails_over_with_the_resume_prompt() {
    let env = routed_env();
    env.account("claude-a", 2, &ok_script(&[]));
    env.account(
        "claude-b",
        2,
        &script(&[
            // Only the resume message gets this account going.
            json!({"expect_stdin_contains": "again please"}),
            json!({"stdout": init_msg()}),
            json!({"stdout": result_msg(json!({"result": "second"}))}),
        ]),
    );
    let h = env.start().await;
    let c = env.client(&h).await;
    let r = spawn(&c, auto_params(&env)).await;
    assert_eq!(r.state, RunState::Succeeded, "{r:?}");

    // The next turn on `claude-a` hits its limit.
    env.account("claude-a", 2, &quota_script_for("again please"));
    let _: Value = c
        .call(
            so_core::proto::METHOD_SEND_MESSAGE,
            json!({"run": r.run_id, "text": "again please"}),
        )
        .await
        .unwrap();
    for _ in 0..400 {
        let v = view(&c, &r.run_id).await;
        if v.state == RunState::Succeeded && v.account.as_deref() == Some("claude-b") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    let v = view(&c, &r.run_id).await;
    assert_eq!(v.account.as_deref(), Some("claude-b"), "{:?}", types(&v));
    assert_eq!(v.result, Some(json!("second")));
    let said = env.stdin_lines("claude-b");
    assert!(
        said[0].contains("again please") && !said[0].contains(PROMPT),
        "{said:?}"
    );
    h.shutdown().await;
}

fn quota_script_for(prompt: &str) -> String {
    script(&[
        json!({"expect_stdin_contains": prompt}),
        json!({"stdout": init_msg()}),
        json!({"stdout": result_msg(json!({"is_error": true, "result": "Claude AI usage limit reached"}))}),
    ])
}

#[tokio::test]
async fn cancelling_while_a_run_fails_over_leaves_nothing_running() {
    let env = routed_env();
    env.account("claude-a", 2, &quota_script());
    env.account("claude-b", 2, &idle_script());
    let h = env.start().await;
    let c = env.client(&h).await;
    h.state()
        .runs
        .set_launch_delay(std::time::Duration::from_millis(600));

    let mut p = auto_params(&env);
    p.wait = false;
    let r = spawn(&c, p).await;
    // As soon as the slot has moved to `claude-b` (its launch is still delayed), cancel.
    for _ in 0..800 {
        if view(&c, &r.run_id).await.account.as_deref() == Some("claude-b") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let _: RunView = c
        .call(
            METHOD_CANCEL_AGENT,
            RunRef {
                run: r.run_id.clone(),
            },
        )
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    let v = view(&c, &r.run_id).await;
    assert_eq!(v.state, RunState::Cancelled, "{:?}", types(&v));
    assert_eq!(h.state().runs.live_count(), 0);
    assert_eq!(
        env.store()
            .count_active_runs_by_account("claude-b")
            .unwrap(),
        0
    );
    h.shutdown().await;
}

#[tokio::test]
async fn a_named_account_does_not_fail_over() {
    let env = routed_env();
    env.account("claude-a", 2, &quota_script());
    env.account("claude-b", 2, &ok_script(&[]));
    let h = env.start().await;
    let c = env.client(&h).await;

    let mut p = auto_params(&env);
    p.account = Some("claude-a".into());
    let r = spawn(&c, p).await;
    assert_eq!(r.state, RunState::Failed, "{r:?}");
    assert_eq!(r.error_kind, Some(ErrorKind::Quota));
    let v = view(&c, &r.run_id).await;
    assert!(events(&v, "failover").is_empty());
    assert_eq!(env.store().list_attempts(&r.run_id, 10).unwrap().len(), 1);
    assert!(env.launches("claude-b").is_empty());
    h.shutdown().await;
}

#[tokio::test]
async fn failover_is_bounded() {
    let env = routed_env();
    // Five accounts that all hit their limit: the run moves three times, then fails.
    for n in ["a", "b", "c", "d", "e"] {
        env.account(&format!("claude-{n}"), 2, &quota_script());
    }
    let h = env.start().await;
    let c = env.client(&h).await;
    let r = spawn(&c, auto_params(&env)).await;
    assert_eq!(r.state, RunState::Failed, "{r:?}");
    let v = view(&c, &r.run_id).await;
    assert_eq!(events(&v, "failover").len(), 3);
    assert_eq!(env.store().list_attempts(&r.run_id, 10).unwrap().len(), 4);
    h.shutdown().await;
}

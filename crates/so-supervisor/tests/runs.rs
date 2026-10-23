#![allow(clippy::unwrap_used)]
//! Run manager against the scripted fake harness, in-process supervisor and real git.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use serde_json::json;
use so_core::proto::{
    CAP_EXCEEDED, DEPTH_LIMIT, JoinResult, METHOD_CANCEL_AGENT, METHOD_JOIN_RUN,
    METHOD_SPAWN_AGENT, RunRef, RunView, SpawnResult, UNAVAILABLE,
};
use so_core::{EventType, RunState, new_id};
use so_store::RunRow;
use so_supervisor::SupervisorClient;
use tempfile::TempDir;

use common::*;

#[tokio::test]
async fn spawn_wait_get_succeeds_and_removes_untouched_worktree() {
    let env = Env::new();
    env.account("claude-acc1", 2, &ok_script(&[]));
    let h = env.start().await;
    let c = env.client(&h).await;
    let mut notes = c.notifications().unwrap();

    let r = spawn(&c, env.spawn_params("claude-acc1")).await;
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    let w = wait_for(&c, &r.run_id).await;
    assert_eq!(w.finished, vec![r.run_id.clone()]);
    assert!(!w.timed_out);

    let v = view(&c, &r.run_id).await;
    assert_eq!(v.state, RunState::Succeeded);
    assert_eq!(v.result, Some(json!("done")));
    assert!(v.usage.output_tokens > 0);
    let t = types(&v);
    let pos = |name: &str| {
        t.iter()
            .position(|x| *x == name)
            .unwrap_or_else(|| panic!("{name} in {t:?}"))
    };
    assert_eq!(t[0], "run_queued");
    assert!(pos("run_queued") < pos("worktree_created"));
    assert!(pos("worktree_created") < pos("attempt_started"));
    assert!(pos("attempt_started") < pos("run_started"));
    assert!(pos("run_started") < pos("tool_call_started"));
    assert!(pos("tool_call_started") < pos("tool_call_finished"));
    assert!(pos("tool_call_finished") < pos("usage"));
    assert!(pos("usage") < pos("run_finished"));
    assert!(pos("run_finished") < pos("worktree_removed"));
    assert_eq!(*t.last().unwrap(), "worktree_removed");
    // Offsets point into the transcript.
    assert!(
        v.recent_events
            .iter()
            .any(|e| e.data.get("transcript_offset").is_some())
    );

    // Worktree and branch are gone; the run dir is private and holds the transcript.
    let wt = v.worktree.clone().unwrap();
    assert!(!Path::new(&wt).exists());
    assert_eq!(git(&env.repo, &["branch", "--list", "so/*"]), "");
    let run_dir = env.paths.run_dir(&r.run_id);
    assert_eq!(
        std::fs::metadata(&run_dir).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert!(run_dir.join("transcript.jsonl").exists());
    assert!(run_dir.join("system.md").exists());
    let mcp = std::fs::read_to_string(run_dir.join("mcp.json")).unwrap();
    assert!(mcp.contains("SUPERORCHESTRATOR_RUN_ID") && mcp.contains(&r.run_id));
    assert!(
        mcp.contains("SUPERORCHESTRATOR_SOCKET") && mcp.contains("SUPERORCHESTRATOR_CONFIG_DIR")
    );

    // Events were pushed to the waiting connection.
    let mut pushed = Vec::new();
    let mut seqs = Vec::new();
    while let Ok(n) = notes.try_recv() {
        assert_eq!(n.method, "event");
        pushed.push(n.params["type"].as_str().unwrap().to_string());
        seqs.push(n.params["seq"].as_u64().unwrap());
    }
    assert!(seqs.windows(2).all(|w| w[0] < w[1]), "{seqs:?}");
    assert!(pushed.contains(&"run_finished".to_string()), "{pushed:?}");

    // Ledger row written.
    let sums = env
        .store()
        .ledger_sums("claude-acc1", "default", chrono::DateTime::UNIX_EPOCH)
        .unwrap();
    assert_eq!(sums.runs, 1);
    h.shutdown().await;
}

#[tokio::test]
async fn spawn_with_wait_returns_the_result() {
    let env = Env::new();
    env.account("claude-acc1", 2, &ok_script(&[]));
    let h = env.start().await;
    let c = env.client(&h).await;
    let mut p = env.spawn_params("claude-acc1");
    p.wait = true;
    p.isolation = Some(so_core::Isolation::None);
    let r = spawn(&c, p).await;
    assert_eq!(r.state, RunState::Succeeded);
    assert_eq!(r.result, Some(json!("done")));
    h.shutdown().await;
}

#[tokio::test]
async fn worktree_is_kept_when_the_harness_commits() {
    let env = Env::new();
    let commit =
        json!({"shell": "git -c user.name=t -c user.email=t@e commit -q --allow-empty -m work"});
    env.account("claude-acc1", 2, &ok_script(&[commit]));
    let h = env.start().await;
    let c = env.client(&h).await;
    let r = spawn(&c, env.spawn_params("claude-acc1")).await;
    wait_for(&c, &r.run_id).await;
    let v = view(&c, &r.run_id).await;
    assert_eq!(v.state, RunState::Succeeded);
    let wt = v.worktree.clone().unwrap();
    assert!(Path::new(&wt).is_dir());
    assert!(!types(&v).contains(&"worktree_removed"));
    let branch = v.branch.unwrap();
    assert!(branch.starts_with("so/"), "{branch}");
    assert!(git(&env.repo, &["branch", "--list", &branch]).contains(&branch));
    h.shutdown().await;
}

#[tokio::test]
async fn non_git_cwd_warns_and_runs_without_isolation() {
    let env = Env::new();
    env.account("claude-acc1", 2, &ok_script(&[]));
    let plain = env.dir.path().join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    let h = env.start().await;
    let c = env.client(&h).await;
    let mut p = env.spawn_params("claude-acc1");
    p.cwd = Some(plain.to_str().unwrap().into());
    let r = spawn(&c, p).await;
    assert!(
        r.warnings
            .iter()
            .any(|w| w.contains("not a git repository")),
        "{:?}",
        r.warnings
    );
    wait_for(&c, &r.run_id).await;
    let v = view(&c, &r.run_id).await;
    assert_eq!(v.state, RunState::Succeeded);
    assert!(v.worktree.is_none());
    h.shutdown().await;
}

#[tokio::test]
async fn dirty_tree_warns() {
    let env = Env::new();
    env.account("claude-acc1", 2, &ok_script(&[]));
    std::fs::write(env.repo.join("dirty.txt"), "x").unwrap();
    let h = env.start().await;
    let c = env.client(&h).await;
    let r = spawn(&c, env.spawn_params("claude-acc1")).await;
    assert!(
        r.warnings.iter().any(|w| w.contains("uncommitted")),
        "{:?}",
        r.warnings
    );
    wait_for(&c, &r.run_id).await;
    h.shutdown().await;
}

#[tokio::test]
async fn account_cap_queues_fifo() {
    let env = Env::new();
    env.account("claude-acc1", 1, &ok_script(&[json!({"sleep_ms": 600})]));
    let h = env.start().await;
    let c = env.client(&h).await;
    let mut p = env.spawn_params("claude-acc1");
    p.isolation = Some(so_core::Isolation::None);
    let a = spawn(&c, p.clone()).await;
    let b = spawn(&c, p.clone()).await;
    let d = spawn(&c, p).await;
    assert_ne!(a.state, RunState::Queued);
    assert_eq!(b.state, RunState::Queued);
    assert_eq!(d.state, RunState::Queued);
    for id in [&a.run_id, &b.run_id, &d.run_id] {
        wait_state(&c, id, RunState::Succeeded).await;
    }
    let store = env.store();
    let started = |id: &str| {
        store
            .list_events_by_run(id, 1000)
            .unwrap()
            .into_iter()
            .find(|e| e.event_type == EventType::RunStarted)
            .unwrap()
            .ts
    };
    let finished = |id: &str| store.get_run(id).unwrap().unwrap().finished_at.unwrap();
    assert!(started(&b.run_id) >= finished(&a.run_id));
    assert!(started(&d.run_id) >= finished(&b.run_id));
    h.shutdown().await;
}

fn interactive_session(account: &str, pid: i64) -> so_store::SessionRow {
    so_store::SessionRow {
        id: new_id(),
        kind: so_core::SessionKind::Interactive,
        client: "wrapper".into(),
        cwd: None,
        account: Some(account.into()),
        pid: Some(pid),
        started_at: chrono::Utc::now(),
        ended_at: None,
        status: "active".into(),
    }
}

#[tokio::test]
async fn a_live_interactive_session_holds_the_cap_and_a_dead_one_does_not() {
    let env = Env::new();
    env.account("claude-acc1", 1, &ok_script(&[]));
    let store = env.store();
    // A real process stands in for the wrapper.
    let mut wrapper = Command::new("sleep").arg("60").spawn().unwrap();
    let live = interactive_session("claude-acc1", i64::from(wrapper.id()));
    store.insert_session(&live).unwrap();
    let h = env.start_reaping(Duration::from_millis(50)).await;
    let c = env.client(&h).await;
    let mut p = env.spawn_params("claude-acc1");
    p.isolation = Some(so_core::Isolation::None);

    let queued = spawn(&c, p.clone()).await;
    assert_eq!(
        queued.state,
        RunState::Queued,
        "live wrapper holds the slot"
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(view(&c, &queued.run_id).await.state, RunState::Queued);
    assert!(
        store
            .get_session(&live.id)
            .unwrap()
            .unwrap()
            .ended_at
            .is_none()
    );

    // The wrapper dies: the periodic reaper frees the slot and the queue drains, with no
    // `status` call.
    wrapper.kill().unwrap();
    wrapper.wait().unwrap();
    wait_state(&c, &queued.run_id, RunState::Succeeded).await;
    assert!(
        store
            .get_session(&live.id)
            .unwrap()
            .unwrap()
            .ended_at
            .is_some()
    );
    h.shutdown().await;
}

#[tokio::test]
async fn spawn_reaps_a_dead_wrapper_before_checking_the_cap() {
    let env = Env::new();
    env.account("claude-acc1", 1, &ok_script(&[]));
    let store = env.store();
    let dead = interactive_session("claude-acc1", i64::from(i32::MAX));
    store.insert_session(&dead).unwrap();
    // The reaper never fires inside this test: only the cap check can free the slot.
    let h = env.start_reaping(Duration::from_secs(3600)).await;
    // `start` already reaped once; put a fresh dead session back after it.
    let dead2 = interactive_session("claude-acc1", i64::from(i32::MAX));
    store.insert_session(&dead2).unwrap();
    let c = env.client(&h).await;
    let mut p = env.spawn_params("claude-acc1");
    p.isolation = Some(so_core::Isolation::None);
    let r = spawn(&c, p).await;
    assert_ne!(r.state, RunState::Queued);
    wait_state(&c, &r.run_id, RunState::Succeeded).await;
    assert!(
        store
            .get_session(&dead2.id)
            .unwrap()
            .unwrap()
            .ended_at
            .is_some()
    );
    h.shutdown().await;
}

#[tokio::test]
async fn depth_two_caller_cannot_spawn_and_children_are_capped() {
    let mut env = Env::new();
    env.config.limits.max_children = 1;
    env.account("claude-acc1", 4, &ok_script(&[json!({"sleep_ms": 1500})]));
    let h = env.start().await;
    let deep = insert_session_and_run(&env.store(), 2, RunState::Running);
    let shallow = insert_session_and_run_in(&env.store(), 1, RunState::Running, &env.repo);

    let c = SupervisorClient::connect(h.socket_path()).await.unwrap();
    c.hello("child", "/", Some(deep.id.clone()), None)
        .await
        .unwrap();
    let e = c
        .call::<_, SpawnResult>(METHOD_SPAWN_AGENT, env.spawn_params("claude-acc1"))
        .await
        .unwrap_err();
    assert_eq!(rpc_code(e), DEPTH_LIMIT);

    let c = SupervisorClient::connect(h.socket_path()).await.unwrap();
    c.hello("child", "/", Some(shallow.id.clone()), None)
        .await
        .unwrap();
    let mut p = env.spawn_params("claude-acc1");
    p.isolation = Some(so_core::Isolation::None);
    let first = spawn(&c, p.clone()).await;
    assert_eq!(view(&c, &first.run_id).await.depth, 2);
    let e = c
        .call::<_, SpawnResult>(METHOD_SPAWN_AGENT, p)
        .await
        .unwrap_err();
    assert_eq!(rpc_code(e), CAP_EXCEEDED);
    h.shutdown().await;
}

#[tokio::test]
async fn auto_routing_and_missing_account_are_unavailable() {
    let env = Env::new();
    let h = env.start().await;
    let c = env.client(&h).await;
    let mut p = env.spawn_params("nope");
    p.harness = Some("auto".into());
    let e = c
        .call::<_, SpawnResult>(METHOD_SPAWN_AGENT, p)
        .await
        .unwrap_err();
    assert_eq!(rpc_code(e), UNAVAILABLE);
    let mut p = env.spawn_params("x");
    p.account = None;
    let e = c
        .call::<_, SpawnResult>(METHOD_SPAWN_AGENT, p)
        .await
        .unwrap_err();
    assert_eq!(rpc_code(e), UNAVAILABLE);
    h.shutdown().await;
}

#[tokio::test]
async fn cancel_cancels_descendants() {
    let env = Env::new();
    let wait_for_interrupt = script(&[
        json!({"expect_stdin_contains": PROMPT}),
        json!({"stdout": init_msg()}),
        json!({"expect_stdin_contains": "interrupt"}),
        json!({"stdout": {"type": "control_response",
            "response": {"subtype": "success", "request_id": "so-int-1"}}}),
    ]);
    env.account("claude-acc1", 2, &wait_for_interrupt);
    env.account("claude-acc2", 2, &wait_for_interrupt);
    let h = env.start().await;
    let c = env.client(&h).await;
    let mut p = env.spawn_params("claude-acc1");
    p.isolation = Some(so_core::Isolation::None);
    let parent = spawn(&c, p.clone()).await;
    wait_state(&c, &parent.run_id, RunState::Running).await;

    let cc = SupervisorClient::connect(h.socket_path()).await.unwrap();
    cc.hello("child", "/", Some(parent.run_id.clone()), None)
        .await
        .unwrap();
    p.account = Some("claude-acc2".into());
    let child = spawn(&cc, p).await;
    wait_state(&c, &child.run_id, RunState::Running).await;

    let v: RunView = c
        .call(
            METHOD_CANCEL_AGENT,
            RunRef {
                run: parent.run_id.clone(),
            },
        )
        .await
        .unwrap();
    assert_eq!(v.state, RunState::Cancelled);
    assert_eq!(view(&c, &child.run_id).await.state, RunState::Cancelled);
    h.shutdown().await;
}

#[tokio::test]
async fn cancel_queued_run() {
    let env = Env::new();
    env.account("claude-acc1", 1, &ok_script(&[json!({"sleep_ms": 1500})]));
    let h = env.start().await;
    let c = env.client(&h).await;
    let mut p = env.spawn_params("claude-acc1");
    p.isolation = Some(so_core::Isolation::None);
    let a = spawn(&c, p.clone()).await;
    let b = spawn(&c, p).await;
    assert_eq!(b.state, RunState::Queued);
    let v: RunView = c
        .call(
            METHOD_CANCEL_AGENT,
            RunRef {
                run: b.run_id.clone(),
            },
        )
        .await
        .unwrap();
    assert_eq!(v.state, RunState::Cancelled);
    let _: RunView = c
        .call(
            METHOD_CANCEL_AGENT,
            RunRef {
                run: a.run_id.clone(),
            },
        )
        .await
        .unwrap();
    h.shutdown().await;
}

#[tokio::test]
async fn quota_error_fails_the_run_and_flags_the_account() {
    let env = Env::new();
    let result = result_msg(json!({"is_error": true, "result": "Claude AI usage limit reached"}));
    env.account(
        "claude-acc1",
        2,
        &script(&[
            json!({"expect_stdin_contains": PROMPT}),
            json!({"stdout": init_msg()}),
            json!({"stdout": result}),
        ]),
    );
    let h = env.start().await;
    let c = env.client(&h).await;
    let mut p = env.spawn_params("claude-acc1");
    p.wait = true;
    p.isolation = Some(so_core::Isolation::None);
    let r = spawn(&c, p).await;
    assert_eq!(r.state, RunState::Failed);
    assert_eq!(r.error_kind, Some(so_core::ErrorKind::Quota));
    let flag = env.store().get_flag("claude-acc1", "default").unwrap();
    assert!(flag.is_some());
    h.shutdown().await;
}

#[tokio::test]
async fn invalid_structured_output_is_reprompted_then_accepted() {
    let env = Env::new();
    let bad = result_msg(json!({"structured_output": {"answer": 5}}));
    let good = result_msg(json!({"structured_output": {"answer": "ok"}}));
    env.account(
        "claude-acc1",
        2,
        &script(&[
            json!({"expect_stdin_contains": PROMPT}),
            json!({"stdout": init_msg()}),
            json!({"stdout": bad}),
            json!({"expect_stdin_contains": "schema"}),
            json!({"stdout": init_msg()}),
            json!({"stdout": good}),
        ]),
    );
    let h = env.start().await;
    let c = env.client(&h).await;
    let mut p = env.spawn_params("claude-acc1");
    p.wait = true;
    p.isolation = Some(so_core::Isolation::None);
    p.output_schema = Some(json!({
        "type": "object", "required": ["answer"],
        "properties": {"answer": {"type": "string"}}
    }));
    let r = spawn(&c, p).await;
    assert_eq!(r.state, RunState::Succeeded);
    assert_eq!(r.result, Some(json!({"answer": "ok"})));
    h.shutdown().await;
}

#[tokio::test]
async fn join_command_has_no_secret_values() {
    let env = Env::new();
    env.account("claude-acc1", 2, &ok_script(&[]));
    let h = env.start().await;
    let c = env.client(&h).await;
    let mut p = env.spawn_params("claude-acc1");
    p.wait = true;
    let r = spawn(&c, p).await;
    let v = view(&c, &r.run_id).await;
    let native = env
        .store()
        .get_run(&r.run_id)
        .unwrap()
        .unwrap()
        .native_session_id
        .unwrap();
    assert_eq!(
        v.join_command.as_deref(),
        Some(format!("claude-acc1 --resume {native}").as_str())
    );
    assert!(v.join_env.is_empty());
    let text = serde_json::to_string(&v).unwrap();
    assert!(
        !text.contains(env.home.to_str().unwrap()),
        "account home leaked"
    );

    let j: JoinResult = c
        .call(
            METHOD_JOIN_RUN,
            RunRef {
                run: r.run_id.clone(),
            },
        )
        .await
        .unwrap();
    assert_eq!(j.argv, ["claude-acc1", "--resume", native.as_str()]);
    let v = view(&c, &r.run_id).await;
    assert!(types(&v).contains(&"join_command_issued"));
    h.shutdown().await;
}

fn orphan(env: &Env, native: Option<&str>) -> RunRow {
    let store = env.store();
    let mut row = insert_session_and_run(&store, 1, RunState::Queued);
    // Re-insert as a started run owned by a dead supervisor.
    let id = new_id();
    row.id = id;
    row.state = RunState::Running;
    row.account = Some("claude-acc1".into());
    row.native_session_id = native.map(String::from);
    row.cwd = Some(env.repo.to_string_lossy().into_owned());
    row.prompt = Some(PROMPT.into());
    store.insert_run(&row).unwrap();
    row
}

#[tokio::test]
async fn reconcile_resumes_a_run_with_a_native_session() {
    let env = Env::new();
    env.account(
        "claude-acc1",
        2,
        &script(&[
            json!({"expect_stdin_contains": "Continue."}),
            json!({"stdout": init_msg()}),
            json!({"stdout": result_msg(json!({}))}),
        ]),
    );
    let row = orphan(&env, Some("native-1"));
    let h = env.start().await;
    let c = env.client(&h).await;
    wait_state(&c, &row.id, RunState::Succeeded).await;
    assert_eq!(view(&c, &row.id).await.result, Some(json!("done")));
    h.shutdown().await;
}

#[tokio::test]
async fn reconcile_fails_a_run_without_a_native_session() {
    let env = Env::new();
    env.account("claude-acc1", 2, &ok_script(&[]));
    let row = orphan(&env, None);
    let h = env.start().await;
    let c = env.client(&h).await;
    assert_eq!(view(&c, &row.id).await.state, RunState::Failed);
    h.shutdown().await;
}

#[tokio::test]
async fn queued_run_of_a_disabled_account_fails_and_the_queue_keeps_working() {
    let env = Env::new();
    env.account("claude-acc1", 1, &ok_script(&[json!({"sleep_ms": 500})]));
    let h = env.start().await;
    let c = env.client(&h).await;
    let mut p = env.spawn_params("claude-acc1");
    p.isolation = Some(so_core::Isolation::None);
    let a = spawn(&c, p.clone()).await;
    let b = spawn(&c, p.clone()).await;
    assert_eq!(b.state, RunState::Queued);
    env.set_enabled("claude-acc1", false);
    wait_state(&c, &a.run_id, RunState::Succeeded).await;
    wait_state(&c, &b.run_id, RunState::Failed).await;
    // The queue still works once the account is back.
    env.set_enabled("claude-acc1", true);
    let d = spawn(&c, p).await;
    wait_state(&c, &d.run_id, RunState::Succeeded).await;
    h.shutdown().await;
}

async fn pid_alive(pidfile: &Path) -> Option<bool> {
    let pid: i32 = std::fs::read_to_string(pidfile).ok()?.trim().parse().ok()?;
    // kill(pid, 0) sends nothing; it only reports whether the pid exists (portable).
    let pid = rustix::process::Pid::from_raw(pid)?;
    Some(match rustix::process::test_kill_process(pid) {
        Ok(()) => true,
        Err(e) => e != rustix::io::Errno::SRCH,
    })
}

#[tokio::test]
async fn pid_alive_sees_own_process_and_not_a_reaped_child() {
    let dir = TempDir::new().unwrap();
    let pidfile = dir.path().join("pid");
    std::fs::write(&pidfile, std::process::id().to_string()).unwrap();
    assert_eq!(pid_alive(&pidfile).await, Some(true));
    let mut child = Command::new("true").spawn().unwrap();
    let child_pid = child.id();
    child.wait().unwrap();
    std::fs::write(&pidfile, child_pid.to_string()).unwrap();
    assert_eq!(pid_alive(&pidfile).await, Some(false));
}

#[tokio::test]
async fn cancel_between_launch_and_go_live_leaves_no_process() {
    let env = Env::new();
    let pidfile = env.dir.path().join("pid");
    // Records its pid, then waits for input that never comes (stdin EOF ends it).
    env.account(
        "claude-acc1",
        2,
        &script(&[
            json!({"expect_stdin_contains": PROMPT}),
            json!({"shell": format!("echo $PPID > {}", pidfile.display())}),
            json!({"expect_stdin_contains": "never"}),
        ]),
    );
    let h = env.start().await;
    let runs = h.state().runs.clone();
    let c = env.client(&h).await;
    // Hold the run between "harness launched" and "harness registered".
    runs.set_go_live_open(false);
    let mut p = env.spawn_params("claude-acc1");
    p.isolation = Some(so_core::Isolation::None);
    let r = spawn(&c, p).await;
    for _ in 0..200 {
        if pidfile.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(
        pid_alive(&pidfile).await,
        Some(true),
        "harness never started"
    );

    let v: RunView = c
        .call(
            METHOD_CANCEL_AGENT,
            RunRef {
                run: r.run_id.clone(),
            },
        )
        .await
        .unwrap();
    assert_eq!(v.state, RunState::Cancelled);
    // Nothing knows the process yet, so it is still running.
    assert_eq!(pid_alive(&pidfile).await, Some(true));

    runs.set_go_live_open(true);
    for _ in 0..200 {
        if pid_alive(&pidfile).await == Some(false) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(
        pid_alive(&pidfile).await,
        Some(false),
        "harness survived the cancel"
    );
    assert_eq!(runs.live_count(), 0);
    assert_eq!(view(&c, &r.run_id).await.state, RunState::Cancelled);
    h.shutdown().await;
}

#[tokio::test]
async fn slow_launch_fails_the_run() {
    let env = Env::new();
    env.account("claude-acc1", 2, &ok_script(&[]));
    let h = env.start().await;
    h.state()
        .runs
        .set_launch_timeout(Duration::from_millis(200));
    h.state().runs.set_launch_delay(Duration::from_secs(3));
    let c = env.client(&h).await;
    let mut p = env.spawn_params("claude-acc1");
    p.wait = true;
    p.isolation = Some(so_core::Isolation::None);
    let r = spawn(&c, p).await;
    assert_eq!(r.state, RunState::Failed);
    let v = view(&c, &r.run_id).await;
    let finished = v
        .recent_events
        .iter()
        .find(|e| e.event_type == EventType::RunFinished)
        .unwrap();
    assert!(
        finished.data["reason"]
            .as_str()
            .unwrap()
            .contains("did not start"),
        "{finished:?}"
    );
    h.shutdown().await;
}

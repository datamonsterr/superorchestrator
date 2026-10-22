#![allow(clippy::unwrap_used)]
//! Run manager against the scripted fake harness, in-process supervisor and real git.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Once;
use std::time::Duration;

use serde_json::{Value, json};
use so_accounts::HostEnv;
use so_core::proto::{
    CAP_EXCEEDED, DEPTH_LIMIT, JoinResult, METHOD_CANCEL_AGENT, METHOD_GET_RUN, METHOD_JOIN_RUN,
    METHOD_SPAWN_AGENT, METHOD_WAIT_AGENT, RunRef, RunView, SpawnParams, SpawnResult, UNAVAILABLE,
    WaitParams, WaitResult,
};
use so_core::{Config, EventType, Harness, Paths, Platform, RunState, SessionKind, new_id};
use so_store::{RunRow, SessionRow, Store};
use so_supervisor::{
    ClientError, Supervisor, SupervisorClient, SupervisorHandle, SupervisorOptions,
    default_handlers,
};
use tempfile::TempDir;

const TRANSCRIPT: &str = include_str!("../../so-harness/tests/fixtures/claude/transcript.jsonl");
const PROMPT: &str = "do the task";

fn fake_harness() -> PathBuf {
    static BUILD: Once = Once::new();
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"));
    BUILD.call_once(|| {
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let st = Command::new(cargo)
            .args([
                "build",
                "-p",
                "so-harness",
                "--features",
                "test-support",
                "--bin",
                "fake-harness",
            ])
            .status()
            .unwrap();
        assert!(st.success());
    });
    let p = target.join("debug/fake-harness");
    assert!(p.exists(), "{} missing", p.display());
    p
}

/// Lines of the first recorded claude turn (up to and including its `result`).
fn first_turn() -> Vec<Value> {
    let mut out = Vec::new();
    for l in TRANSCRIPT.lines() {
        let v: Value = serde_json::from_str(l).unwrap();
        if v["dir"] != "in" {
            continue;
        }
        let msg = v["msg"].clone();
        let last = msg["type"] == "result";
        out.push(msg);
        if last {
            break;
        }
    }
    out
}

fn script(steps: &[Value]) -> String {
    steps.iter().map(|s| format!("{s}\n")).collect()
}

fn stdout_steps(msgs: &[Value]) -> Vec<Value> {
    msgs.iter().map(|m| json!({"stdout": m})).collect()
}

/// A successful turn after the prompt was read.
fn ok_script(extra_before: &[Value]) -> String {
    let mut steps = vec![json!({"expect_stdin_contains": PROMPT})];
    steps.extend_from_slice(extra_before);
    steps.extend(stdout_steps(&first_turn()));
    script(&steps)
}

fn result_msg(extra: Value) -> Value {
    let mut base = json!({
        "type": "result", "subtype": "success", "is_error": false, "result": "done",
        "session_id": "s", "total_cost_usd": 0.01,
        "usage": {"input_tokens": 3, "output_tokens": 4},
    });
    base.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    base
}

fn init_msg() -> Value {
    json!({"type": "system", "subtype": "init", "session_id": "s"})
}

struct Env {
    dir: TempDir,
    paths: Paths,
    sock: PathBuf,
    repo: PathBuf,
    home: PathBuf,
    config: Config,
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

impl Env {
    fn new() -> Env {
        let dir = tempfile::Builder::new()
            .prefix("so-")
            .tempdir_in("/tmp")
            .unwrap();
        let cfg = dir.path().join("cfg");
        let repo = dir.path().join("repo");
        let home = dir.path().join("home");
        for d in [&cfg, &repo, &home] {
            std::fs::create_dir_all(d).unwrap();
        }
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["config", "user.email", "t@example.com"]);
        git(&repo, &["config", "user.name", "t"]);
        std::fs::write(repo.join("a.txt"), "a").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-q", "-m", "init"]);
        let sock = dir.path().join("run/s.sock");
        Env {
            paths: Paths::from_dir(cfg),
            sock,
            repo,
            home,
            config: Config::default(),
            dir,
        }
    }

    /// A claude account whose binary replays `script`.
    fn account(&self, name: &str, cap: u32, script_text: &str) {
        let script_path = self.dir.path().join(format!("{name}.jsonl"));
        std::fs::write(&script_path, script_text).unwrap();
        let wrapper = self.dir.path().join(format!("{name}.sh"));
        std::fs::write(
            &wrapper,
            format!(
                "#!/bin/sh\nexport FAKE_HARNESS_SCRIPT={}\nexec {} \"$@\"\n",
                script_path.display(),
                fake_harness().display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::create_dir_all(self.paths.accounts_dir()).unwrap();
        std::fs::write(
            self.paths.account_file(name),
            format!(
                "name = \"{name}\"\nharness = \"claude\"\nhome = \"{}\"\nbinary = \"{}\"\ncap = {cap}\n",
                self.home.join(format!(".{name}")).display(),
                wrapper.display()
            ),
        )
        .unwrap();
    }

    fn store(&self) -> Store {
        Store::open(self.paths.state_db()).unwrap()
    }

    async fn start(&self) -> SupervisorHandle {
        let host = HostEnv {
            home: self.home.clone(),
            path_var: Some("/usr/bin:/bin".into()),
            platform: Platform::current(),
        };
        let opts =
            SupervisorOptions::new(self.paths.clone(), self.config.clone(), self.sock.clone())
                .with_lock_wait(Duration::ZERO)
                .with_idle_exit(Duration::from_secs(600))
                .with_host(host)
                .with_setup(default_handlers);
        Supervisor::start(opts).await.unwrap()
    }

    async fn client(&self, h: &SupervisorHandle) -> SupervisorClient {
        let c = SupervisorClient::connect(h.socket_path()).await.unwrap();
        c.hello("test", self.repo.to_str().unwrap(), None, None)
            .await
            .unwrap();
        c
    }

    fn spawn_params(&self, account: &str) -> SpawnParams {
        SpawnParams {
            prompt: Some(PROMPT.into()),
            harness: Some("claude".into()),
            account: Some(account.into()),
            cwd: Some(self.repo.to_str().unwrap().into()),
            ..SpawnParams::default()
        }
    }
}

async fn spawn(c: &SupervisorClient, p: SpawnParams) -> SpawnResult {
    c.call(METHOD_SPAWN_AGENT, p).await.unwrap()
}

async fn wait_for(c: &SupervisorClient, run: &str) -> WaitResult {
    c.call(
        METHOD_WAIT_AGENT,
        WaitParams {
            runs: vec![run.into()],
            timeout_ms: Some(20_000),
        },
    )
    .await
    .unwrap()
}

async fn view(c: &SupervisorClient, run: &str) -> RunView {
    c.call(METHOD_GET_RUN, RunRef { run: run.into() })
        .await
        .unwrap()
}

async fn wait_state(c: &SupervisorClient, run: &str, want: RunState) {
    for _ in 0..400 {
        if view(c, run).await.state == want {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("run {run} never reached {want}");
}

fn types(v: &RunView) -> Vec<&'static str> {
    v.recent_events
        .iter()
        .map(|e| e.event_type.as_str())
        .collect()
}

fn rpc_code(e: ClientError) -> i64 {
    match e {
        ClientError::Rpc(r) => r.code,
        other => panic!("expected rpc error, got {other:?}"),
    }
}

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
    while let Ok(n) = notes.try_recv() {
        assert_eq!(n.method, "event");
        pushed.push(n.params["type"].as_str().unwrap().to_string());
    }
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

fn insert_session_and_run(store: &Store, depth: i64, state: RunState) -> RunRow {
    let session = SessionRow {
        id: new_id(),
        kind: SessionKind::Orchestrator,
        client: "t".into(),
        cwd: None,
        account: None,
        pid: None,
        started_at: chrono::Utc::now(),
        ended_at: None,
        status: "active".into(),
    };
    store.insert_session(&session).unwrap();
    let row = RunRow {
        id: new_id(),
        session_id: session.id,
        parent_run_id: None,
        depth,
        name: None,
        agent: None,
        choice_id: None,
        harness: Harness::Claude,
        account: None,
        model: None,
        effort: None,
        cwd: None,
        worktree_path: None,
        branch: None,
        native_session_id: None,
        state,
        review: false,
        output_schema: None,
        result_json: None,
        error_kind: None,
        prompt: None,
        created_at: chrono::Utc::now(),
        started_at: None,
        finished_at: None,
    };
    store.insert_run(&row).unwrap();
    row
}

#[tokio::test]
async fn depth_two_caller_cannot_spawn_and_children_are_capped() {
    let mut env = Env::new();
    env.config.limits.max_children = 1;
    env.account("claude-acc1", 4, &ok_script(&[json!({"sleep_ms": 1500})]));
    let h = env.start().await;
    let deep = insert_session_and_run(&env.store(), 2, RunState::Running);
    let shallow = insert_session_and_run(&env.store(), 1, RunState::Running);

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

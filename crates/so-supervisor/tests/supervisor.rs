#![allow(clippy::unwrap_used)]

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use so_core::proto::{HelloResult, INVALID_REQUEST, METHOD_NOT_FOUND, PARSE_ERROR, StatusResult};
use so_core::{Config, EventType, Harness, Paths, RunState, SessionKind, new_id};
use so_store::{RunRow, SessionRow, Store};
use so_supervisor::{
    ClientError, Supervisor, SupervisorClient, SupervisorError, SupervisorHandle, SupervisorOptions,
};
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

struct Env {
    dir: TempDir,
    paths: Paths,
    sock: PathBuf,
}

impl Env {
    fn new() -> Env {
        // Unix socket paths are limited to ~108 bytes, so stay under a short /tmp dir.
        let dir = tempfile::Builder::new()
            .prefix("so-")
            .tempdir_in("/tmp")
            .unwrap();
        let cfg = dir.path().join("cfg");
        std::fs::create_dir_all(&cfg).unwrap();
        let sock = dir.path().join("run/s.sock");
        Env {
            paths: Paths::from_dir(cfg),
            sock,
            dir,
        }
    }

    fn opts(&self, idle: Duration) -> SupervisorOptions {
        SupervisorOptions::new(self.paths.clone(), Config::default(), self.sock.clone())
            .with_idle_exit(idle)
            .with_lock_wait(Duration::ZERO)
    }

    async fn start(&self) -> SupervisorHandle {
        Supervisor::start(self.opts(Duration::from_secs(600)))
            .await
            .unwrap()
    }

    fn store(&self) -> Store {
        Store::open(self.paths.state_db()).unwrap()
    }
}

fn session_row() -> SessionRow {
    SessionRow {
        id: new_id(),
        kind: SessionKind::Orchestrator,
        client: "t".into(),
        cwd: None,
        account: None,
        pid: None,
        started_at: chrono::Utc::now(),
        ended_at: None,
        status: "active".into(),
    }
}

fn run_row(session_id: &str, state: RunState, depth: i64) -> RunRow {
    RunRow {
        id: new_id(),
        session_id: session_id.into(),
        parent_run_id: None,
        depth,
        name: None,
        agent: None,
        choice_id: None,
        harness: Harness::Codex,
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
        isolation: so_core::Isolation::None,
        max_turns: None,
        worktree_base: None,
        worktree_repo: None,
        timeout_ms: None,
        created_at: chrono::Utc::now(),
        started_at: None,
        finished_at: None,
    }
}

async fn connect(h: &SupervisorHandle) -> SupervisorClient {
    SupervisorClient::connect(h.socket_path()).await.unwrap()
}

async fn hello(c: &SupervisorClient) -> HelloResult {
    c.hello("test", "/w", None, Some(7)).await.unwrap()
}

fn rpc_code(e: ClientError) -> i64 {
    match e {
        ClientError::Rpc(r) => r.code,
        other => panic!("expected rpc error, got {other:?}"),
    }
}

async fn wait_until(mut f: impl FnMut() -> bool) {
    for _ in 0..100 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    panic!("condition not reached");
}

#[tokio::test]
async fn second_supervisor_fails_with_already_running() {
    let env = Env::new();
    let h = env.start().await;
    match Supervisor::start(env.opts(Duration::from_secs(60))).await {
        Err(SupervisorError::AlreadyRunning { pid }) => {
            assert_eq!(pid, Some(std::process::id()))
        }
        other => panic!("expected AlreadyRunning, got {:?}", other.err()),
    }
    h.shutdown().await;
    // Lock released after shutdown.
    env.start().await.shutdown().await;
}

#[tokio::test]
async fn socket_is_0600_and_dir_0700() {
    let env = Env::new();
    let h = env.start().await;
    let sock_mode = std::fs::metadata(&env.sock).unwrap().permissions().mode() & 0o777;
    let dir_mode = std::fs::metadata(env.sock.parent().unwrap())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(sock_mode, 0o600);
    assert_eq!(dir_mode, 0o700);
    h.shutdown().await;
    assert!(!env.sock.exists());
}

#[tokio::test]
async fn stale_socket_is_replaced() {
    let env = Env::new();
    let h = env.start().await;
    h.shutdown().await;
    // Leave a stale socket file behind, as after a crash.
    let l = std::os::unix::net::UnixListener::bind(&env.sock).unwrap();
    drop(l);
    assert!(env.sock.exists());
    env.start().await.shutdown().await;
}

#[tokio::test]
async fn hello_required_first() {
    let env = Env::new();
    let h = env.start().await;
    let c = connect(&h).await;
    let e = c.call::<_, Value>("status", json!({})).await.unwrap_err();
    assert_eq!(rpc_code(e), INVALID_REQUEST);
    hello(&c).await;
    c.call::<_, StatusResult>("status", json!({}))
        .await
        .unwrap();
    h.shutdown().await;
}

#[tokio::test]
async fn hello_creates_session_row() {
    let env = Env::new();
    let h = env.start().await;
    let c = connect(&h).await;
    let r = hello(&c).await;
    assert_eq!(r.depth, 0);
    assert_eq!(r.run_id, None);
    let row = env.store().get_session(&r.session_id).unwrap().unwrap();
    assert_eq!(row.kind, SessionKind::Orchestrator);
    assert_eq!(row.client, "test");
    assert_eq!(row.cwd.as_deref(), Some("/w"));
    assert_eq!(row.pid, Some(7));
    assert_eq!(row.status, "active");
    h.shutdown().await;
}

#[tokio::test]
async fn hello_with_run_id_attaches_to_run_session() {
    let env = Env::new();
    let store = env.store();
    let s = session_row();
    store.insert_session(&s).unwrap();
    let run = run_row(&s.id, RunState::Running, 2);
    store.insert_run(&run).unwrap();
    let h = env.start().await;
    let c = connect(&h).await;
    let r = c
        .hello("so-mcp", "/w", Some(run.id.clone()), None)
        .await
        .unwrap();
    assert_eq!(r.session_id, s.id);
    assert_eq!(r.run_id.as_deref(), Some(run.id.as_str()));
    assert_eq!(r.depth, 2);
    // Closing a run connection leaves the session alone.
    let st = h.state().clone();
    let before = st.connected_clients();
    drop(c);
    wait_until(|| st.connected_clients() < before).await;
    assert_eq!(store.get_session(&s.id).unwrap().unwrap().status, "active");
    // Unknown run id is an error.
    let c2 = connect(&h).await;
    let e = c2
        .hello("x", "/w", Some("nope".into()), None)
        .await
        .unwrap_err();
    assert_eq!(rpc_code(e), so_core::proto::NOT_FOUND);
    h.shutdown().await;
}

#[tokio::test]
async fn unknown_method_and_malformed_line() {
    let env = Env::new();
    let h = env.start().await;
    let c = connect(&h).await;
    hello(&c).await;
    let e = c.call::<_, Value>("nope", json!({})).await.unwrap_err();
    assert_eq!(rpc_code(e), METHOD_NOT_FOUND);

    let stream = UnixStream::connect(&env.sock).await.unwrap();
    let (rd, mut wr) = stream.into_split();
    wr.write_all(b"{not json\n").await.unwrap();
    let mut lines = BufReader::new(rd).lines();
    let line = lines.next_line().await.unwrap().unwrap();
    let v: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(v["error"]["code"], PARSE_ERROR);
    assert!(v["id"].is_null());
    // The connection survives a bad line.
    wr.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":5,\"method\":\"status\"}\n")
        .await
        .unwrap();
    let v: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
    assert_eq!(v["id"], 5);
    assert_eq!(v["error"]["code"], INVALID_REQUEST);
    h.shutdown().await;
}

#[tokio::test]
async fn status_returns_counts() {
    let env = Env::new();
    let store = env.store();
    let s = session_row();
    store.insert_session(&s).unwrap();
    store
        .insert_run(&run_row(&s.id, RunState::Succeeded, 0))
        .unwrap();
    let h = Supervisor::start(env.opts(Duration::from_secs(600)).with_setup(|st| {
        // A hook that adopts runs, so the pre-inserted active run survives reconcile.
        struct Keep;
        impl so_supervisor::ReconcileHook for Keep {
            fn reconcile(&self, _: &Store, _: &RunRow) -> bool {
                true
            }
        }
        st.set_reconcile_hook(Arc::new(Keep));
    }))
    .await
    .unwrap();
    store
        .insert_run(&run_row(&s.id, RunState::Running, 0))
        .unwrap();
    let c = connect(&h).await;
    hello(&c).await;
    let st: StatusResult = c.call("status", json!({})).await.unwrap();
    assert_eq!(st.pid, std::process::id());
    assert_eq!(st.version, env!("CARGO_PKG_VERSION"));
    assert_eq!(st.sessions, 2); // the pre-inserted one and the hello session
    assert_eq!(st.active_runs, 1);
    h.shutdown().await;
}

#[tokio::test]
async fn client_round_trip_and_server_push() {
    let env = Env::new();
    let h = Supervisor::start(env.opts(Duration::from_secs(600)).with_setup(|st| {
        st.router
            .register("echo", |_ctx, params| async move { Ok(params) });
        st.router.register("push", |ctx, _p| async move {
            ctx.notifier.notify("progress", json!({"pct": 50})).await;
            Ok(json!("done"))
        });
    }))
    .await
    .unwrap();
    let c = connect(&h).await;
    let mut notes = c.notifications().unwrap();
    assert!(c.notifications().is_none());
    hello(&c).await;
    let v: Value = c.call("echo", json!({"a": 1})).await.unwrap();
    assert_eq!(v, json!({"a": 1}));
    let r: String = c.call("push", json!({})).await.unwrap();
    assert_eq!(r, "done");
    let n = tokio::time::timeout(Duration::from_secs(2), notes.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(n.method, "progress");
    assert_eq!(n.params["pct"], 50);
    h.shutdown().await;
}

#[tokio::test]
async fn closing_connection_ends_orchestrator_session() {
    let env = Env::new();
    let h = env.start().await;
    let c = connect(&h).await;
    let s = hello(&c).await.session_id;
    let store = env.store();
    assert_eq!(store.get_session(&s).unwrap().unwrap().status, "active");
    drop(c);
    wait_until(|| store.get_session(&s).unwrap().unwrap().status == "ended").await;
    assert!(store.get_session(&s).unwrap().unwrap().ended_at.is_some());

    // A session that still has an active run stays open.
    let c = connect(&h).await;
    let s2 = hello(&c).await.session_id;
    store
        .insert_run(&run_row(&s2, RunState::Running, 0))
        .unwrap();
    drop(c);
    let st = h.state().clone();
    wait_until(|| st.connected_clients() == 0).await;
    // (reconcile only runs at start, so the run is still active)
    assert_eq!(store.get_session(&s2).unwrap().unwrap().status, "active");
    h.shutdown().await;
}

#[tokio::test]
async fn idle_exit_shuts_down_and_removes_socket() {
    let env = Env::new();
    let h = Supervisor::start(env.opts(Duration::from_millis(300)))
        .await
        .unwrap();
    assert!(env.sock.exists());
    tokio::time::timeout(Duration::from_secs(3), h.wait())
        .await
        .unwrap();
    assert!(!env.sock.exists());
    // Lock released.
    env.start().await.shutdown().await;
}

#[tokio::test]
async fn connected_client_and_busy_probe_keep_it_alive() {
    let env = Env::new();
    let busy = Arc::new(AtomicBool::new(true));
    let b = busy.clone();
    let h = Supervisor::start(env.opts(Duration::from_millis(300)).with_setup(move |st| {
        st.set_probe(Arc::new(move || b.load(Ordering::SeqCst)));
    }))
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(900)).await;
    assert!(env.sock.exists(), "busy probe must keep the server up");
    SupervisorClient::connect(&env.sock).await.unwrap();
    busy.store(false, Ordering::SeqCst);
    tokio::time::timeout(Duration::from_secs(3), h.wait())
        .await
        .unwrap();
    assert!(!env.sock.exists());
}

#[tokio::test]
async fn connected_client_keeps_server_alive() {
    let env = Env::new();
    let h = Supervisor::start(env.opts(Duration::from_millis(300)))
        .await
        .unwrap();
    let c = connect(&h).await;
    hello(&c).await;
    tokio::time::sleep(Duration::from_millis(900)).await;
    assert!(env.sock.exists());
    drop(c);
    tokio::time::timeout(Duration::from_secs(3), h.wait())
        .await
        .unwrap();
}

#[tokio::test]
async fn reconcile_fails_orphaned_runs_with_events() {
    let env = Env::new();
    let store = env.store();
    let s = session_row();
    store.insert_session(&s).unwrap();
    let running = run_row(&s.id, RunState::Running, 0);
    let queued = run_row(&s.id, RunState::Queued, 0);
    let done = run_row(&s.id, RunState::Succeeded, 0);
    for r in [&running, &queued, &done] {
        store.insert_run(r).unwrap();
    }
    let h = env.start().await;
    for id in [&running.id, &queued.id] {
        let r = store.get_run(id).unwrap().unwrap();
        assert_eq!(r.state, RunState::Failed);
        assert_eq!(r.error_kind, Some(so_core::ErrorKind::Other));
        assert!(r.finished_at.is_some());
        let types: Vec<EventType> = store
            .list_events_by_run(id, 100)
            .unwrap()
            .into_iter()
            .map(|e| e.event_type)
            .collect();
        assert_eq!(types, vec![EventType::StateChanged, EventType::RunFinished]);
    }
    assert_eq!(
        store.get_run(&done.id).unwrap().unwrap().state,
        RunState::Succeeded
    );
    h.shutdown().await;
}

#[tokio::test]
async fn connect_or_spawn_starts_server() {
    let env = Env::new();
    let opts = env.opts(Duration::from_secs(60));
    let c = SupervisorClient::connect_or_spawn(&env.sock, move || {
        tokio::spawn(async move {
            Supervisor::start(opts).await.unwrap().wait().await;
        });
        Ok(())
    })
    .await
    .unwrap();
    hello(&c).await;
    let _ = &env.dir;
}

#[tokio::test]
async fn socket_dir_owned_by_other_user_is_refused() {
    // /proc is owned by root.
    let env = Env::new();
    let opts = SupervisorOptions::new(
        env.paths.clone(),
        Config::default(),
        PathBuf::from("/proc/so-test.sock"),
    );
    if rustix::process::geteuid().is_root() {
        return;
    }
    match Supervisor::start(opts).await {
        Err(SupervisorError::InsecureSocketDir(_)) => {}
        other => panic!("expected InsecureSocketDir, got {:?}", other.err()),
    }
}

#[tokio::test]
async fn oversized_line_gets_error_and_is_skipped() {
    let env = Env::new();
    let h = env.start().await;
    let stream = UnixStream::connect(&env.sock).await.unwrap();
    let (rd, mut wr) = stream.into_split();
    let big = vec![b'a'; so_supervisor::MAX_LINE_BYTES + 1024];
    wr.write_all(&big).await.unwrap();
    wr.write_all(b"\n").await.unwrap();
    let mut lines = BufReader::new(rd).lines();
    let v: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
    assert_eq!(v["error"]["code"], INVALID_REQUEST);
    // The next request is still served.
    wr.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":9,\"method\":\"status\"}\n")
        .await
        .unwrap();
    let v: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
    assert_eq!(v["id"], 9);
    h.shutdown().await;
}

#[tokio::test]
async fn handler_panic_replies_internal_error() {
    let env = Env::new();
    let h = Supervisor::start(env.opts(Duration::from_secs(600)).with_setup(|st| {
        st.router.register("boom", |_c, _p| async move {
            if true {
                panic!("boom");
            }
            Ok(Value::Null)
        });
    }))
    .await
    .unwrap();
    let c = connect(&h).await;
    hello(&c).await;
    let e = c.call::<_, Value>("boom", json!({})).await.unwrap_err();
    assert_eq!(rpc_code(e), so_core::proto::INTERNAL_ERROR);
    // The connection is still usable.
    c.call::<_, StatusResult>("status", json!({}))
        .await
        .unwrap();
    h.shutdown().await;
}

#[tokio::test]
async fn closed_fires_on_eof_and_inflight_handler_still_replies() {
    let env = Env::new();
    let seen = Arc::new(AtomicBool::new(false));
    let flag = seen.clone();
    let h = Supervisor::start(env.opts(Duration::from_secs(600)).with_setup(move |st| {
        st.router.register("wait_closed", move |ctx, _p| {
            let flag = flag.clone();
            async move {
                ctx.closed().await;
                flag.store(true, Ordering::SeqCst);
                Ok(json!("bye"))
            }
        });
    }))
    .await
    .unwrap();
    let stream = UnixStream::connect(&env.sock).await.unwrap();
    let (rd, mut wr) = stream.into_split();
    wr.write_all(
        b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"hello\",\"params\":{\"client\":\"t\",\"cwd\":\"/\"}}\n\
          {\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"wait_closed\"}\n",
    )
    .await
    .unwrap();
    let mut lines = BufReader::new(rd).lines();
    let hello_reply = lines.next_line().await.unwrap().unwrap();
    assert!(hello_reply.contains("session_id"));
    assert!(!seen.load(Ordering::SeqCst));
    wr.shutdown().await.unwrap(); // half-close: EOF for the server
    wait_until(|| seen.load(Ordering::SeqCst)).await;
    let v: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
    assert_eq!(v["id"], 2);
    assert_eq!(v["result"], "bye");
    h.shutdown().await;
}

#[tokio::test]
async fn call_after_connection_death_is_disconnected() {
    let env = Env::new();
    let h = env.start().await;
    let c = connect(&h).await;
    hello(&c).await;
    h.shutdown().await;
    let mut result = None;
    for _ in 0..100 {
        match c
            .call_timeout::<_, Value>("status", json!({}), Duration::from_secs(1))
            .await
        {
            Err(ClientError::Disconnected) => {
                result = Some(());
                break;
            }
            _ => tokio::time::sleep(Duration::from_millis(30)).await,
        }
    }
    assert!(result.is_some(), "expected Disconnected");
    assert_eq!(c.pending_calls(), 0);
}

#[tokio::test]
async fn timed_out_call_leaks_no_pending_entry() {
    let env = Env::new();
    let h = Supervisor::start(env.opts(Duration::from_secs(600)).with_setup(|st| {
        st.router.register("slow", |_c, _p| async move {
            tokio::time::sleep(Duration::from_secs(30)).await;
            Ok(Value::Null)
        });
    }))
    .await
    .unwrap();
    let c = connect(&h).await;
    hello(&c).await;
    let e = c
        .call_timeout::<_, Value>("slow", json!({}), Duration::from_millis(100))
        .await
        .unwrap_err();
    assert!(matches!(e, ClientError::CallTimeout(_)));
    assert_eq!(c.pending_calls(), 0);
    // A dropped (cancelled) future leaks nothing either.
    {
        let fut = c.call::<_, Value>("slow", json!({}));
        let _ = tokio::time::timeout(Duration::from_millis(50), fut).await;
    }
    assert_eq!(c.pending_calls(), 0);
    c.call::<_, StatusResult>("status", json!({}))
        .await
        .unwrap();
    h.shutdown().await;
}

#[tokio::test]
async fn existing_group_readable_socket_dir_is_refused() {
    let env = Env::new();
    let dir = env.dir.path().join("open");
    std::fs::create_dir(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    let opts = SupervisorOptions::new(env.paths.clone(), Config::default(), dir.join("s.sock"));
    match Supervisor::start(opts).await {
        Err(SupervisorError::InsecureSocketDir(_)) => {}
        other => panic!("expected InsecureSocketDir, got {:?}", other.err()),
    }
}

#[tokio::test]
async fn saturated_connection_replies_unavailable_and_keeps_reading() {
    let env = Env::new();
    let h = Supervisor::start(
        env.opts(Duration::from_secs(600))
            .with_max_inflight(1)
            .with_setup(|st| {
                st.router.register("slow", |_c, _p| async move {
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    Ok(Value::Null)
                });
            }),
    )
    .await
    .unwrap();
    let stream = UnixStream::connect(&env.sock).await.unwrap();
    let (rd, mut wr) = stream.into_split();
    wr.write_all(
        b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"hello\",\"params\":{\"client\":\"t\",\"cwd\":\"/\"}}\n\
          {\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"slow\"}\n\
          {\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"slow\"}\n",
    )
    .await
    .unwrap();
    let mut lines = BufReader::new(rd).lines();
    let _hello = lines.next_line().await.unwrap().unwrap();
    let v: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
    assert_eq!(v["id"], 3);
    assert_eq!(v["error"]["code"], so_core::proto::UNAVAILABLE);
    // EOF is still noticed while a handler is stuck: the session ends on shutdown.
    wr.shutdown().await.unwrap();
    h.shutdown().await;
}

#[tokio::test]
async fn respawn_right_after_idle_exit_succeeds() {
    let env = Env::new();
    let first = Supervisor::start(env.opts(Duration::from_millis(300)))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(250)).await;
    let spawn_opts = || {
        SupervisorOptions::new(env.paths.clone(), Config::default(), env.sock.clone())
            .with_idle_exit(Duration::from_secs(60))
    };
    let mut handles = Vec::new();
    let (c, r) = SupervisorClient::connect_or_spawn_hello(
        &env.sock,
        || {
            let opts = spawn_opts();
            handles.push(tokio::spawn(async move {
                if let Ok(h) = Supervisor::start(opts).await {
                    h.wait().await;
                }
            }));
            Ok(())
        },
        "t",
        "/w",
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(r.depth, 0);
    drop(c);
    first.wait().await;
}

#[tokio::test]
async fn oversized_request_is_rejected_by_client() {
    let env = Env::new();
    let h = env.start().await;
    let c = connect(&h).await;
    hello(&c).await;
    let big = "a".repeat(so_supervisor::MAX_LINE_BYTES);
    let e = c
        .call::<_, Value>("echo", json!({"x": big}))
        .await
        .unwrap_err();
    assert!(matches!(e, ClientError::RequestTooLarge));
    assert_eq!(c.pending_calls(), 0);
    h.shutdown().await;
}

#[tokio::test]
async fn list_harnesses_reports_accounts_load_and_choices() {
    use so_core::proto::HarnessesView;
    use so_core::{AccountFile, Choice};

    let env = Env::new();
    let home = env.dir.path().join("homes");
    let mut a = AccountFile::new(
        "alpha",
        Harness::Codex,
        &home.join("a").to_string_lossy(),
        "codex",
    )
    .unwrap();
    a.cap = 3;
    a.save(&env.paths).unwrap();
    std::fs::create_dir_all(home.join("a")).unwrap();
    std::fs::write(home.join("a/auth.json"), "{}").unwrap();
    let mut b = AccountFile::new(
        "beta",
        Harness::Codex,
        &home.join("b").to_string_lossy(),
        "codex",
    )
    .unwrap();
    b.enabled = false;
    b.save(&env.paths).unwrap();

    // A broken account file degrades to an entry with `error`; the others still list.
    std::fs::write(env.paths.accounts_dir().join("zeta.toml"), "not = [valid").unwrap();

    let store = env.store();
    let session = session_row();
    store.insert_session(&session).unwrap();
    let mut inter = session_row();
    inter.kind = SessionKind::Interactive;
    inter.account = Some("alpha".into());
    store.insert_session(&inter).unwrap();

    let mut config = Config::default();
    for (id, enabled) in [("c1", true), ("c2", false)] {
        config.choices.push(Choice {
            id: id.into(),
            harness: Harness::Codex,
            model: "m".into(),
            effort: None,
            roles: vec!["code".into()],
            description: String::new(),
            enabled,
        });
    }
    let opts = SupervisorOptions::new(env.paths.clone(), config, env.sock.clone())
        .with_idle_exit(Duration::from_secs(600))
        .with_lock_wait(Duration::ZERO)
        .with_host(so_accounts::HostEnv {
            home: env.dir.path().join("fakehome"),
            path_var: None,
            platform: so_core::Platform::Linux,
        })
        .with_setup(so_supervisor::default_handlers);
    let h = Supervisor::start(opts).await.unwrap();
    let c = connect(&h).await;
    hello(&c).await;
    // After start: reconcile would otherwise end the orphaned running run.
    let mut run = run_row(&session.id, RunState::Running, 0);
    run.account = Some("alpha".into());
    store.insert_run(&run).unwrap();
    let v: HarnessesView = c.call("list_harnesses", json!({})).await.unwrap();

    assert_eq!(v.accounts.len(), 3);
    let alpha = &v.accounts[0];
    assert_eq!(alpha.name, "alpha");
    assert_eq!(alpha.cap, 3);
    assert_eq!(alpha.signed_in, so_core::proto::SignInState::SignedIn);
    assert_eq!(alpha.active_runs, 2);
    assert!(alpha.enabled);
    assert!(!alpha.headroom.is_empty());
    assert_eq!(alpha.headroom[0].status, so_core::HeadroomStatus::Unknown);
    let beta = &v.accounts[1];
    assert_eq!(beta.name, "beta");
    assert_eq!(beta.signed_in, so_core::proto::SignInState::SignedOut);
    assert!(!beta.enabled);
    assert_eq!(beta.active_runs, 0);
    let zeta = &v.accounts[2];
    assert_eq!(zeta.name, "zeta");
    assert!(zeta.error.is_some());
    assert!(alpha.error.is_none());
    assert_eq!(alpha.headroom[0].observed_at, chrono::DateTime::UNIX_EPOCH);
    assert_eq!(v.choices.len(), 2);
    assert!(v.choices.iter().any(|c| c.id == "c2" && !c.enabled));
    h.shutdown().await;
}

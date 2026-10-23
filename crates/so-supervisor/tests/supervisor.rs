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

async fn hello_run(c: &SupervisorClient, store: &Store, run: &str, inspect: bool) -> HelloResult {
    // Hash independently of production code to check the persisted SHA-256 contract.
    use sha2::{Digest, Sha256};
    let token = "test-only-identity-token";
    store
        .set_run_token_hash(run, &Sha256::digest(token.as_bytes()))
        .unwrap();
    c.call("hello", json!({"client": "test", "cwd": "/w", "run_id": run, "run_token": token, "inspect": inspect})).await.unwrap()
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
async fn inspect_hello_opens_no_session_and_refuses_spawns() {
    let env = Env::new();
    let h = env.start().await;
    let c = connect(&h).await;
    c.hello_inspect("cli", "/w", Some(7)).await.unwrap();
    // Reads work without a session.
    let st: StatusResult = c.call("status", json!({})).await.unwrap();
    assert_eq!(st.sessions, 0);
    assert!(
        env.store()
            .list_sessions(so_store::SessionFilter::All, 100)
            .unwrap()
            .is_empty()
    );
    // Spawning needs a session.
    for m in ["spawn_agent", "run_workflow"] {
        let e = c.call::<_, Value>(m, json!({})).await.unwrap_err();
        assert!(
            matches!(e, ClientError::Rpc(ref r) if r.code == INVALID_REQUEST),
            "{m}: {e:?}"
        );
    }
    // Only an explicit allowlist of reads and controls is open to it.
    for m in ["send_message", "accept_run", "reject_run", "fork_agent"] {
        let e = c.call::<_, Value>(m, json!({})).await.unwrap_err();
        assert!(
            matches!(&e, ClientError::Rpc(r) if r.code == INVALID_REQUEST && r.message.contains("read-only")),
            "{m}: {e:?}"
        );
    }
    h.shutdown().await;
}

#[tokio::test]
async fn inspect_hello_with_a_run_id_still_gets_the_run_session() {
    let env = Env::new();
    let store = env.store();
    let s = session_row();
    store.insert_session(&s).unwrap();
    let run = run_row(&s.id, RunState::Running, 0);
    store.insert_run(&run).unwrap();
    let h = env.start().await;
    let c = connect(&h).await;
    let r = hello_run(&c, &store, &run.id, true).await;
    assert_eq!(r.session_id, s.id);
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
    let r = hello_run(&c, &store, &run.id, false).await;
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
    // / is owned by root.
    let env = Env::new();
    let opts = SupervisorOptions::new(
        env.paths.clone(),
        Config::default(),
        PathBuf::from("/so-test.sock"),
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

#[tokio::test]
async fn list_harnesses_brief_is_one_short_row_per_account_and_choice() {
    use so_core::{AccountFile, Choice, Headroom, HeadroomSource, HeadroomStatus};
    use so_store::FlagRow;

    let env = Env::new();
    let home = env.dir.path().join("homes");
    let account = |name: &str, harness: Harness, cap: u32, enabled: bool| {
        let mut a =
            AccountFile::new(name, harness, &home.join(name).to_string_lossy(), "bin").unwrap();
        a.cap = cap;
        a.enabled = enabled;
        a.save(&env.paths).unwrap();
    };
    account("codex-a", Harness::Codex, 1, true);
    account("oc-a", Harness::Opencode, 2, true);
    account("claude-a", Harness::Claude, 1, false);
    account("agy-a", Harness::Agy, 1, true);

    let store = env.store();
    let session = session_row();
    store.insert_session(&session).unwrap();
    let now = chrono::Utc::now();
    store
        .upsert_headroom(&Headroom {
            account: "oc-a".into(),
            pool: "default".into(),
            five_hour_remaining_pct: Some(79.6),
            five_hour_resets_at: None,
            weekly_remaining_pct: Some(60.0),
            weekly_resets_at: None,
            status: HeadroomStatus::Ok,
            source: HeadroomSource::Ledger,
            observed_at: now,
        })
        .unwrap();
    let until = now + chrono::Duration::hours(2);
    store
        .set_flag(&FlagRow {
            account: "oc-a".into(),
            pool: "default".into(),
            until,
            reason: "rate limit".into(),
        })
        .unwrap();

    let choice = |id: &str, harness: Harness, model: &str, enabled: bool| Choice {
        id: id.into(),
        harness,
        model: model.into(),
        effort: None,
        roles: vec!["worker".into()],
        description: String::new(),
        enabled,
    };
    let config = Config {
        choices: vec![
            choice("codex-c", Harness::Codex, "gpt-6.1-sol", true),
            choice(
                "free-c",
                Harness::Opencode,
                "opencode/deepseek-v4.1-flash-free",
                true,
            ),
            choice(
                "paid-c",
                Harness::Opencode,
                "opencode/claude-opus-5-5",
                true,
            ),
            choice("claude-c", Harness::Claude, "claude-sonnet-5-5", true),
            choice("agy-c", Harness::Agy, "gemini-3.8-flash-low", true),
            choice("off-c", Harness::Opencode, "opencode/beam-free", false),
        ],
        ..Config::default()
    };
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
    // The codex account runs one thing, which is its whole cap.
    let mut run = run_row(&session.id, RunState::Running, 0);
    run.account = Some("codex-a".into());
    store.insert_run(&run).unwrap();

    let v: Value = c
        .call("list_harnesses", json!({"brief": true}))
        .await
        .unwrap();
    let account = |name: &str| {
        v["accounts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["name"] == name)
            .unwrap()
            .clone()
    };
    let choice = |id: &str| {
        v["choices"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["id"] == id)
            .unwrap()
            .clone()
    };

    let oc = account("oc-a");
    assert_eq!(oc["headroom"], "ok 5h80 wk60");
    assert_eq!(oc["load"], "0/2");
    assert_eq!(
        oc["cooldown"],
        until.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    );
    assert_eq!(oc["signed_in"], false);
    // Nothing known about a pool: no headroom key; disabled is only shown when true.
    assert!(account("codex-a").get("headroom").is_none());
    assert_eq!(account("codex-a")["load"], "1/1");
    assert_eq!(account("claude-a")["disabled"], true);
    assert!(oc.get("disabled").is_none());

    assert_eq!(choice("codex-c")["eligible"], false);
    assert_eq!(choice("codex-c")["why"], "at cap");
    assert_eq!(choice("free-c")["free"], true);
    assert_eq!(choice("free-c")["why"], "cooldown");
    assert!(choice("paid-c").get("free").is_none());
    assert_eq!(choice("claude-c")["why"], "account disabled");
    assert!(
        choice("agy-c").get("eligible").is_none() && choice("agy-c").get("why").is_none(),
        "eligible is the default"
    );
    assert_eq!(choice("off-c")["why"], "disabled");
    assert!(
        choice("free-c").get("roles").is_none(),
        "worker is the default role"
    );

    let free = v["free_models"]["opencode"].as_array().unwrap();
    assert!(free.contains(&json!("opencode/deepseek-v4.1-flash-free")));
    assert!(!free.contains(&json!("opencode/claude-opus-5-5")));
    // The plain call keeps the full shape for the CLI and the web UI.
    let full: Value = c.call("list_harnesses", json!({})).await.unwrap();
    assert!(full["accounts"][0].get("flags").is_some());
    h.shutdown().await;
}

#[tokio::test]
async fn startup_removes_inactive_and_unknown_run_tokens_but_preserves_active_runs() {
    let env = Env::new();
    let store = env.store();
    let session = session_row();
    store.insert_session(&session).unwrap();
    let mut rows = Vec::new();
    for state in [
        RunState::Succeeded,
        RunState::Failed,
        RunState::Cancelled,
        RunState::Running,
        RunState::Queued,
        RunState::PendingReview,
    ] {
        let row = run_row(&session.id, state, 0);
        store.insert_run(&row).unwrap();
        rows.push(row);
    }
    let unknown = new_id();
    for id in rows.iter().map(|r| &r.id).chain(std::iter::once(&unknown)) {
        let dir = env.paths.run_dir(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("ws-token"), "stale capability").unwrap();
        std::fs::write(dir.join("transcript.jsonl"), "keep evidence").unwrap();
    }
    // Never traverse a run-dir symlink while doing cleanup.
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("ws-token"), "outside").unwrap();
    std::os::unix::fs::symlink(outside.path(), env.paths.run_dir("linked-run")).unwrap();
    let h = Supervisor::start(env.opts(Duration::from_secs(600)).with_setup(|st| {
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
    for row in rows {
        let dir = env.paths.run_dir(&row.id);
        assert_eq!(
            dir.join("ws-token").exists(),
            row.state.is_active(),
            "{}",
            row.state.as_str()
        );
        assert!(dir.join("transcript.jsonl").exists());
    }
    assert!(!env.paths.run_dir(&unknown).join("ws-token").exists());
    assert!(outside.path().join("ws-token").exists());
    h.shutdown().await;
}

#[tokio::test]
async fn startup_removes_token_after_failing_an_orphaned_run() {
    let env = Env::new();
    let store = env.store();
    let session = session_row();
    store.insert_session(&session).unwrap();
    let row = run_row(&session.id, RunState::Running, 0);
    store.insert_run(&row).unwrap();
    let dir = env.paths.run_dir(&row.id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("ws-token"), "stale capability").unwrap();
    let h = env.start().await;
    assert_eq!(
        store.get_run(&row.id).unwrap().unwrap().state,
        RunState::Failed
    );
    assert!(!dir.join("ws-token").exists());
    h.shutdown().await;
}

// GHSA-vw56-37xq-jrf9: a session must not act on another session's runs.
async fn cross_session_run_request(method: &str) {
    let env = Env::new();
    let h = Supervisor::start(
        env.opts(Duration::from_secs(600))
            .with_setup(so_supervisor::default_handlers),
    )
    .await
    .unwrap();
    let c = connect(&h).await;
    hello(&c).await;
    let store = env.store();
    let s = session_row();
    store.insert_session(&s).unwrap();
    let run = run_row(&s.id, RunState::PendingReview, 1);
    store.insert_run(&run).unwrap();
    let p = match method {
        "wait_agent" => json!({"runs": [run.id], "timeout_ms": 0}),
        "send_message" => json!({"run": run.id, "text": "forbidden"}),
        "reject_run" => json!({"run": run.id, "feedback": "forbidden"}),
        "fork_agent" => json!({"run": run.id, "prompt": "forbidden"}),
        _ => json!({"run": run.id}),
    };
    let e = c.call::<_, Value>(method, p.clone()).await.unwrap_err();
    let mut missing = p;
    if method == "wait_agent" {
        missing["runs"] = json!(["missing-run"]);
    } else {
        missing["run"] = json!("missing-run");
    }
    let absent = c.call::<_, Value>(method, missing).await.unwrap_err();
    match (e, absent) {
        (ClientError::Rpc(denied), ClientError::Rpc(absent)) => {
            assert_eq!(denied.code, so_core::proto::NOT_FOUND, "{method}");
            assert_eq!(
                (denied.code, denied.message),
                (absent.code, absent.message),
                "{method}"
            );
        }
        other => panic!("expected RPC errors: {other:?}"),
    }
    assert_eq!(
        store.get_run(&run.id).unwrap().unwrap().state,
        RunState::PendingReview
    );
    h.shutdown().await;
}

macro_rules! cross_session_test {
    ($name:ident, $method:literal) => {
        #[tokio::test]
        async fn $name() {
            cross_session_run_request($method).await;
        }
    };
}
cross_session_test!(cross_session_get_refused, "get_run");
cross_session_test!(cross_session_cancel_refused, "cancel_agent");
cross_session_test!(cross_session_join_refused, "join_run");
cross_session_test!(cross_session_wait_refused, "wait_agent");
cross_session_test!(cross_session_send_refused, "send_message");
cross_session_test!(cross_session_accept_refused, "accept_run");
cross_session_test!(cross_session_reject_refused, "reject_run");
cross_session_test!(cross_session_fork_refused, "fork_agent");

#[tokio::test]
async fn hello_missing_run_token_refused() {
    let env = Env::new();
    let store = env.store();
    let s = session_row();
    store.insert_session(&s).unwrap();
    let run = run_row(&s.id, RunState::Running, 1);
    store.insert_run(&run).unwrap();
    let h = env.start().await;
    let c = connect(&h).await;
    let e = c
        .hello("claimed", "/w", Some(run.id), None)
        .await
        .unwrap_err();
    assert_eq!(rpc_code(e), INVALID_REQUEST);
    // A refused identity must not greet the connection or let it read any messages.
    assert_eq!(
        rpc_code(
            c.call::<_, Value>("read_messages", json!({}))
                .await
                .unwrap_err()
        ),
        INVALID_REQUEST
    );
    h.shutdown().await;
}

#[tokio::test]
async fn review_startup_continues_after_a_token_cleanup_error() {
    let env = Env::new();
    let bad = env.paths.run_dir("bad");
    std::fs::create_dir_all(bad.join("ws-token")).unwrap();
    let good = env.paths.run_dir("good");
    std::fs::create_dir_all(&good).unwrap();
    std::fs::write(good.join("ws-token"), "stale").unwrap();
    let h = env.start().await;
    assert!(!good.join("ws-token").exists());
    assert!(bad.join("ws-token").is_dir());
    h.shutdown().await;
}

#[tokio::test]
async fn cross_session_workflow_requests_refused() {
    let env = Env::new();
    let h = Supervisor::start(
        env.opts(Duration::from_secs(600))
            .with_setup(so_supervisor::default_handlers),
    )
    .await
    .unwrap();
    let c = connect(&h).await;
    hello(&c).await;
    let row = so_store::WorkflowRunRow {
        id: new_id(),
        name: "private".into(),
        session_id: Some("other-session".into()),
        state: "running".into(),
        args_json: "{}".into(),
        started_at: chrono::Utc::now(),
        finished_at: None,
        result_json: None,
        error: None,
    };
    env.store().insert_workflow_run(&row).unwrap();
    for method in ["workflow_status", "cancel_workflow"] {
        let e = c
            .call::<_, Value>(method, json!({"id": row.id}))
            .await
            .unwrap_err();
        let absent = c
            .call::<_, Value>(method, json!({"id": "missing-workflow"}))
            .await
            .unwrap_err();
        match (e, absent) {
            (ClientError::Rpc(denied), ClientError::Rpc(absent)) => {
                assert_eq!(denied.code, so_core::proto::NOT_FOUND, "{method}");
                assert_eq!(
                    (denied.code, denied.message),
                    (absent.code, absent.message),
                    "{method}"
                );
            }
            other => panic!("expected RPC errors: {other:?}"),
        }
    }
    h.shutdown().await;
}

async fn sibling_request(method: &str) {
    let env = Env::new();
    let store = env.store();
    let h = Supervisor::start(
        env.opts(Duration::from_secs(600))
            .with_setup(so_supervisor::default_handlers),
    )
    .await
    .unwrap();

    let s = session_row();
    store.insert_session(&s).unwrap();
    let caller = run_row(&s.id, RunState::Running, 1);
    let sibling = run_row(&s.id, RunState::Running, 1);
    store.insert_run(&caller).unwrap();
    store.insert_run(&sibling).unwrap();
    let c = connect(&h).await;
    hello_run(&c, &store, &caller.id, false).await;
    let p = if method == "send_message" {
        json!({"run": sibling.id, "text": "forbidden"})
    } else {
        json!({"run": sibling.id})
    };
    let e = c.call::<_, Value>(method, p).await.unwrap_err();
    assert!(
        matches!(e, ClientError::Rpc(ref e) if e.message.contains("subtree")),
        "{method}: {e:?}"
    );
    assert_eq!(
        store.get_run(&sibling.id).unwrap().unwrap().state,
        RunState::Running
    );
    assert!(
        store
            .list_messages_by_run(&sibling.id, 100)
            .unwrap()
            .is_empty()
    );
    h.shutdown().await;
}

#[tokio::test]
async fn review_startup_continues_when_runs_directory_cannot_be_read() {
    let env = Env::new();
    std::fs::write(env.paths.runs_dir(), "not a directory").unwrap();
    let h = env.start().await;
    h.shutdown().await;
}

#[tokio::test]
async fn sibling_get_refused() {
    sibling_request("get_run").await;
}
#[tokio::test]
async fn sibling_cancel_refused() {
    sibling_request("cancel_agent").await;
}
#[tokio::test]
async fn sibling_send_refused() {
    sibling_request("send_message").await;
}

#[tokio::test]
async fn descendants_and_ancestor_messages_allowed_but_ancestor_controls_refused() {
    let env = Env::new();
    let store = env.store();
    let h = Supervisor::start(
        env.opts(Duration::from_secs(600))
            .with_setup(so_supervisor::default_handlers),
    )
    .await
    .unwrap();

    let s = session_row();
    store.insert_session(&s).unwrap();
    let ancestor = run_row(&s.id, RunState::Running, 1);
    let mut caller = run_row(&s.id, RunState::Running, 2);
    caller.parent_run_id = Some(ancestor.id.clone());
    let mut descendant = run_row(&s.id, RunState::Running, 3);
    descendant.parent_run_id = Some(caller.id.clone());
    for r in [&ancestor, &caller, &descendant] {
        store.insert_run(r).unwrap();
    }
    let c = connect(&h).await;
    hello_run(&c, &store, &caller.id, false).await;
    for run in [&caller.id, &descendant.id] {
        c.call::<_, Value>("get_run", json!({"run": run}))
            .await
            .unwrap();
        c.call::<_, Value>("send_message", json!({"run": run, "text": "allowed"}))
            .await
            .unwrap();
    }
    c.call::<_, Value>(
        "send_message",
        json!({"run": ancestor.id, "text": "report to parent"}),
    )
    .await
    .unwrap();
    assert_eq!(
        store.list_messages_by_run(&ancestor.id, 100).unwrap()[0].body,
        "report to parent"
    );
    for method in ["get_run", "cancel_agent"] {
        assert!(
            c.call::<_, Value>(method, json!({"run": ancestor.id}))
                .await
                .is_err(),
            "ancestor {method} must be refused"
        );
    }
    c.call::<_, Value>("cancel_agent", json!({"run": descendant.id}))
        .await
        .unwrap();
    assert_eq!(
        store.get_run(&descendant.id).unwrap().unwrap().state,
        RunState::Cancelled
    );
    h.shutdown().await;
}

#[tokio::test]
async fn depth_zero_session_client_can_access_all_its_runs() {
    let env = Env::new();
    let h = Supervisor::start(
        env.opts(Duration::from_secs(600))
            .with_setup(so_supervisor::default_handlers),
    )
    .await
    .unwrap();
    let c = connect(&h).await;
    let session = hello(&c).await;
    let store = env.store();
    let parent = run_row(&session.session_id, RunState::Queued, 1);
    let mut descendant = run_row(&session.session_id, RunState::Queued, 2);
    descendant.parent_run_id = Some(parent.id.clone());
    for r in [&parent, &descendant] {
        store.insert_run(r).unwrap();
        c.call::<_, Value>("get_run", json!({"run": r.id}))
            .await
            .unwrap();
        c.call::<_, Value>("send_message", json!({"run": r.id, "text": "allowed"}))
            .await
            .unwrap();
        c.call::<_, Value>("cancel_agent", json!({"run": r.id}))
            .await
            .unwrap();
    }
    h.shutdown().await;
}

#[tokio::test]
async fn hello_wrong_token_refused_then_correct_token_accepted() {
    // Hash independently of production code to check the persisted SHA-256 contract.
    use sha2::{Digest, Sha256};
    let env = Env::new();
    let h = env.start().await;
    let store = env.store();
    let s = session_row();
    store.insert_session(&s).unwrap();
    let run = run_row(&s.id, RunState::Running, 1);
    store.insert_run(&run).unwrap();
    store
        .set_run_token_hash(&run.id, &Sha256::digest(b"correct-test-token"))
        .unwrap();
    let c = connect(&h).await;
    for token in [
        None,
        Some(""),
        Some("wrong-test-token"),
        Some("correct-test-token-with-extra-suffix"),
    ] {
        let e = c
            .call::<_, Value>(
                "hello",
                json!({"client": "test", "cwd": "/", "run_id": run.id, "run_token": token}),
            )
            .await
            .unwrap_err();
        assert!(
            matches!(e, ClientError::Rpc(ref e) if e.code == INVALID_REQUEST && e.message == "invalid run identity token")
        );
    }
    let r: HelloResult = c.call("hello", json!({"client": "test", "cwd": "/", "run_id": run.id, "run_token": "correct-test-token"})).await.unwrap();
    assert_eq!(r.session_id, s.id);
    assert_eq!(r.run_id.as_deref(), Some(run.id.as_str()));
    assert_eq!(r.depth, 1);
    h.shutdown().await;
}

#[tokio::test]
async fn hello_token_for_another_run_and_legacy_claims_refused_even_with_inspect() {
    // Hash independently of production code to check the persisted SHA-256 contract.
    use sha2::{Digest, Sha256};
    let env = Env::new();
    let h = env.start().await;
    let store = env.store();
    let s = session_row();
    store.insert_session(&s).unwrap();
    let first = run_row(&s.id, RunState::Running, 1);
    let second = run_row(&s.id, RunState::Running, 1);
    let legacy = run_row(&s.id, RunState::Running, 1);
    for r in [&first, &second, &legacy] {
        store.insert_run(r).unwrap();
    }
    store
        .set_run_token_hash(&first.id, &Sha256::digest(b"first-token"))
        .unwrap();
    store
        .set_run_token_hash(&second.id, &Sha256::digest(b"second-token"))
        .unwrap();
    for id in [&second.id, &legacy.id] {
        let c = connect(&h).await;
        let e = c.call::<_, Value>("hello", json!({"client": "test", "cwd": "/", "run_id": id, "run_token": "first-token", "inspect": true})).await.unwrap_err();
        assert_eq!(rpc_code(e), INVALID_REQUEST);
    }
    h.shutdown().await;
}

#[tokio::test]
async fn sibling_and_orchestrator_workflows_are_outside_a_run_subtree() {
    let env = Env::new();
    let h = Supervisor::start(
        env.opts(Duration::from_secs(600))
            .with_setup(so_supervisor::default_handlers),
    )
    .await
    .unwrap();
    let orchestrator = connect(&h).await;
    let session = hello(&orchestrator).await;
    let store = env.store();
    let owner = run_row(&session.session_id, RunState::Running, 1);
    let sibling = run_row(&session.session_id, RunState::Running, 1);
    store.insert_run(&owner).unwrap();
    store.insert_run(&sibling).unwrap();
    let owner_client = connect(&h).await;
    hello_run(&owner_client, &store, &owner.id, false).await;
    let sibling_client = connect(&h).await;
    hello_run(&sibling_client, &store, &sibling.id, false).await;
    let owned: Value = owner_client
        .call("run_workflow", json!({"script": "return 1;"}))
        .await
        .unwrap();
    let top: Value = orchestrator
        .call("run_workflow", json!({"script": "return 2;"}))
        .await
        .unwrap();
    owner_client
        .call::<_, Value>("workflow_status", json!({"id": owned["id"]}))
        .await
        .unwrap();
    orchestrator
        .call::<_, Value>("workflow_status", json!({"id": owned["id"]}))
        .await
        .unwrap();
    for id in [&owned["id"], &top["id"]] {
        for method in ["workflow_status", "cancel_workflow"] {
            let e = sibling_client
                .call::<_, Value>(method, json!({"id": id}))
                .await
                .unwrap_err();
            assert!(
                matches!(e, ClientError::Rpc(ref e) if e.message.contains("subtree")),
                "{method}: {e:?}"
            );
        }
    }
    h.shutdown().await;
}

#[tokio::test]
async fn inspect_operator_keeps_run_read_wait_join_cancel_access() {
    let env = Env::new();
    let h = Supervisor::start(
        env.opts(Duration::from_secs(600))
            .with_setup(so_supervisor::default_handlers),
    )
    .await
    .unwrap();
    let store = env.store();
    let s = session_row();
    store.insert_session(&s).unwrap();
    let mut run = run_row(&s.id, RunState::Running, 1);
    run.account = Some("codex-a".into());
    run.native_session_id = Some("native-1".into());
    run.cwd = Some("/w".into());
    store.insert_run(&run).unwrap();
    let c = connect(&h).await;
    c.hello_inspect("cli", "/w", None).await.unwrap();
    for method in ["get_run", "join_run"] {
        c.call::<_, Value>(method, json!({"run": run.id}))
            .await
            .unwrap();
    }
    c.call::<_, Value>("wait_agent", json!({"runs": [run.id], "timeout_ms": 0}))
        .await
        .unwrap();
    c.call::<_, Value>("cancel_agent", json!({"run": run.id}))
        .await
        .unwrap();
    assert_eq!(
        store.get_run(&run.id).unwrap().unwrap().state,
        RunState::Cancelled
    );
    h.shutdown().await;
}

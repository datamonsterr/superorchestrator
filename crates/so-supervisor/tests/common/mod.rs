#![allow(dead_code, clippy::unwrap_used)]
//! Shared setup for the supervisor integration tests: the scripted fake harness behind
//! claude accounts, an in-process supervisor and a real git repository.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Once;
use std::time::Duration;

use serde_json::{Value, json};
use so_accounts::HostEnv;
use so_core::proto::{
    METHOD_GET_RUN, METHOD_SPAWN_AGENT, METHOD_WAIT_AGENT, RunRef, RunView, SpawnParams,
    SpawnResult, WaitParams, WaitResult,
};
use so_core::{Config, Harness, Paths, Platform, RunState, SessionKind, new_id};
use so_store::{RunRow, SessionRow, Store};
use so_supervisor::{
    ClientError, Supervisor, SupervisorClient, SupervisorHandle, SupervisorOptions,
    default_handlers,
};
use tempfile::TempDir;

pub const TRANSCRIPT: &str =
    include_str!("../../../so-harness/tests/fixtures/claude/transcript.jsonl");
pub const PROMPT: &str = "do the task";

pub fn fake_harness() -> PathBuf {
    static BUILD: Once = Once::new();
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
    let p = sibling_bin("fake-harness");
    assert!(p.exists(), "{} missing", p.display());
    p
}

/// Lines of the first recorded claude turn (up to and including its `result`).
pub fn first_turn() -> Vec<Value> {
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

pub fn script(steps: &[Value]) -> String {
    steps.iter().map(|s| format!("{s}\n")).collect()
}

pub fn stdout_steps(msgs: &[Value]) -> Vec<Value> {
    msgs.iter().map(|m| json!({"stdout": m})).collect()
}

/// A successful turn after the prompt was read.
pub fn ok_script(extra_before: &[Value]) -> String {
    let mut steps = vec![json!({"expect_stdin_contains": PROMPT})];
    steps.extend_from_slice(extra_before);
    steps.extend(stdout_steps(&first_turn()));
    script(&steps)
}

pub fn result_msg(extra: Value) -> Value {
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

pub fn init_msg() -> Value {
    json!({"type": "system", "subtype": "init", "session_id": "s"})
}

pub struct Env {
    pub dir: TempDir,
    pub paths: Paths,
    pub sock: PathBuf,
    pub repo: PathBuf,
    pub home: PathBuf,
    pub config: Config,
}

pub fn git(dir: &Path, args: &[&str]) -> String {
    let out = so_supervisor::runs::worktree::git_command(dir)
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
    pub fn new() -> Env {
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
    pub fn account(&self, name: &str, cap: u32, script_text: &str) {
        self.account_of(name, "claude", cap, script_text);
    }

    /// An agy account (with its `.gemini` dir) whose binary replays `script`. Linux only:
    /// macOS allows one agy account (ADR-0005, SPEC 6.4).
    #[cfg(target_os = "linux")]
    pub fn agy_account(&self, name: &str, cap: u32, script_text: &str) {
        std::fs::create_dir_all(self.home.join(format!(".{name}")).join(".gemini")).unwrap();
        self.account_of(name, "agy", cap, script_text);
    }

    fn account_of(&self, name: &str, harness: &str, cap: u32, script_text: &str) {
        let script_path = self.dir.path().join(format!("{name}.jsonl"));
        std::fs::write(&script_path, script_text).unwrap();
        let wrapper = self.dir.path().join(format!("{name}.sh"));
        std::fs::write(
            &wrapper,
            format!(
                "#!/bin/sh\nexport FAKE_HARNESS_SCRIPT={}\nexport FAKE_HARNESS_LOG={}\nexec {} \"$@\"\n",
                script_path.display(),
                self.launch_log(name).display(),
                fake_harness().display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::create_dir_all(self.paths.accounts_dir()).unwrap();
        std::fs::write(
            self.paths.account_file(name),
            format!(
                "name = \"{name}\"\nharness = \"{harness}\"\nhome = \"{}\"\nbinary = \"{}\"\ncap = {cap}\n",
                self.home.join(format!(".{name}")).display(),
                wrapper.display()
            ),
        )
        .unwrap();
    }

    /// An account of `harness` whose binary is the executable `binary`.
    pub fn account_with_binary(&self, name: &str, harness: &str, cap: u32, binary: &Path) {
        std::fs::create_dir_all(self.home.join(format!(".{name}"))).unwrap();
        std::fs::create_dir_all(self.paths.accounts_dir()).unwrap();
        std::fs::write(
            self.paths.account_file(name),
            format!(
                "name = \"{name}\"\nharness = \"{harness}\"\nhome = \"{}\"\nbinary = \"{}\"\ncap = {cap}\n",
                self.home.join(format!(".{name}")).display(),
                binary.display()
            ),
        )
        .unwrap();
    }

    /// Where the fake harness of `name` records each launch (argv and env keys).
    pub fn launch_log(&self, name: &str) -> PathBuf {
        self.dir.path().join(format!("{name}.log"))
    }

    /// The `argv` of every launch of the account's harness, oldest first.
    pub fn launches(&self, name: &str) -> Vec<Vec<String>> {
        self.log_records(name)
            .into_iter()
            .filter_map(|v| {
                v["argv"].as_array().map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect()
                })
            })
            .collect()
    }

    /// Every line the fake harness of `name` read from its stdin (prompts and messages).
    pub fn stdin_lines(&self, name: &str) -> Vec<String> {
        self.log_records(name)
            .into_iter()
            .filter_map(|v| v["stdin"].as_str().map(String::from))
            .collect()
    }

    /// Polls (bounded) until the fake harness of `name` has read at least `min` stdin
    /// lines, then returns them. The harness writes its log after the run is already
    /// `Running`, so a read right after a state change can come up empty.
    pub async fn wait_stdin_lines(&self, name: &str, min: usize) -> Vec<String> {
        for _ in 0..400 {
            let said = self.stdin_lines(name);
            if said.len() >= min {
                return said;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!(
            "{name} never read {min} stdin line(s): {:?}",
            self.stdin_lines(name)
        );
    }

    /// The complete JSON lines of the launch log; a partial trailing line (the fake
    /// harness may still be writing) is ignored.
    fn log_records(&self, name: &str) -> Vec<Value> {
        let text = std::fs::read_to_string(self.launch_log(name)).unwrap_or_default();
        let complete = match text.rfind('\n') {
            Some(i) => &text[..=i],
            None => "",
        };
        complete
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .collect()
    }

    pub fn set_enabled(&self, name: &str, enabled: bool) {
        let path = self.paths.account_file(name);
        let text = std::fs::read_to_string(&path).unwrap();
        let text: String = text
            .lines()
            .filter(|l| !l.starts_with("enabled"))
            .map(|l| format!("{l}\n"))
            .collect();
        std::fs::write(path, format!("{text}enabled = {enabled}\n")).unwrap();
    }

    pub fn store(&self) -> Store {
        Store::open(self.paths.state_db()).unwrap()
    }

    pub async fn start(&self) -> SupervisorHandle {
        self.start_reaping(Duration::from_secs(30)).await
    }

    /// Like [`Env::start`], ending dead interactive sessions every `reap`.
    pub async fn start_reaping(&self, reap: Duration) -> SupervisorHandle {
        let host = HostEnv {
            home: self.home.clone(),
            path_var: Some("/usr/bin:/bin".into()),
            platform: Platform::current(),
        };
        let opts =
            SupervisorOptions::new(self.paths.clone(), self.config.clone(), self.sock.clone())
                .with_lock_wait(Duration::ZERO)
                .with_idle_exit(Duration::from_secs(600))
                .with_reap_interval(reap)
                .with_host(host)
                .with_setup(default_handlers);
        Supervisor::start(opts).await.unwrap()
    }

    pub async fn client(&self, h: &SupervisorHandle) -> SupervisorClient {
        let c = SupervisorClient::connect(h.socket_path()).await.unwrap();
        c.hello("test", self.repo.to_str().unwrap(), None, None)
            .await
            .unwrap();
        c
    }

    pub async fn inspect_client(&self, h: &SupervisorHandle) -> SupervisorClient {
        let c = SupervisorClient::connect(h.socket_path()).await.unwrap();
        c.hello_inspect("test-operator", self.repo.to_str().unwrap(), None)
            .await
            .unwrap();
        c
    }

    pub fn spawn_params(&self, account: &str) -> SpawnParams {
        SpawnParams {
            prompt: Some(PROMPT.into()),
            harness: Some("claude".into()),
            account: Some(account.into()),
            cwd: Some(self.repo.to_str().unwrap().into()),
            ..SpawnParams::default()
        }
    }
}

pub async fn spawn(c: &SupervisorClient, p: SpawnParams) -> SpawnResult {
    c.call(METHOD_SPAWN_AGENT, p).await.unwrap()
}

pub async fn wait_for(c: &SupervisorClient, run: &str) -> WaitResult {
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

pub async fn view(c: &SupervisorClient, run: &str) -> RunView {
    c.call(METHOD_GET_RUN, RunRef { run: run.into() })
        .await
        .unwrap()
}

pub async fn wait_state(c: &SupervisorClient, run: &str, want: RunState) {
    for _ in 0..400 {
        if view(c, run).await.state == want {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("run {run} never reached {want}");
}

pub fn types(v: &RunView) -> Vec<&'static str> {
    v.recent_events
        .iter()
        .map(|e| e.event_type.as_str())
        .collect()
}

pub fn rpc_code(e: ClientError) -> i64 {
    match e {
        ClientError::Rpc(r) => r.code,
        other => panic!("expected rpc error, got {other:?}"),
    }
}

/// A session with one run of the given depth and state, written straight to the store.
pub fn insert_session_and_run(store: &Store, depth: i64, state: RunState) -> RunRow {
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
        isolation: so_core::Isolation::None,
        max_turns: None,
        worktree_base: None,
        worktree_repo: None,
        timeout_ms: None,
        created_at: chrono::Utc::now(),
        started_at: None,
        finished_at: None,
    };
    store.insert_run(&row).unwrap();
    row
}

/// A binary of this workspace's `debug` profile, found next to the running test (test
/// executables live in `<target>/debug/deps`). Works for any target directory.
pub fn sibling_bin(name: &str) -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let debug = exe.parent().unwrap().parent().unwrap();
    debug.join(format!("{name}{}", std::env::consts::EXE_SUFFIX))
}

/// Gives a manually inserted fixture the identity a real enqueue would create.
pub fn seed_run_identity(store: &Store, paths: &Paths, run: &str) {
    // Hash independently of production code to check the persisted SHA-256 contract.
    use sha2::{Digest, Sha256};
    let dir = paths.run_dir(run);
    std::fs::create_dir_all(&dir).unwrap();
    let token = "test-only-run-token";
    std::fs::write(dir.join("rpc-token"), token).unwrap();
    store
        .set_run_token_hash(run, &Sha256::digest(token.as_bytes()))
        .unwrap();
}

/// Authenticates a test run, using its real spawn token where available.
pub async fn hello_run(
    c: &SupervisorClient,
    store: &Store,
    paths: &Paths,
    run: &str,
) -> so_core::proto::HelloResult {
    // Hash independently of production code to check the persisted SHA-256 contract.
    use sha2::{Digest, Sha256};
    let token =
        std::fs::read_to_string(paths.run_dir(run).join("rpc-token")).unwrap_or_else(|_| {
            assert!(
                store.get_run_token_hash(run).unwrap().is_none(),
                "spawn token file must exist"
            );
            let token = "test-only-run-token".to_string();
            store
                .set_run_token_hash(run, &Sha256::digest(token.as_bytes()))
                .unwrap();
            token
        });
    c.call(
        "hello",
        json!({"client": "subagent-test", "cwd": "/", "run_id": run, "run_token": token}),
    )
    .await
    .unwrap()
}

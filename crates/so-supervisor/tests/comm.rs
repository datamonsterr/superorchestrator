#![allow(clippy::unwrap_used)]
//! Messages, nesting, the review gate, stalls, process death, notifications and fork,
//! against the scripted fake harness and an in-process supervisor.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, Once};
use std::time::Duration;

use serde_json::{Value, json};
use so_accounts::HostEnv;
use so_core::proto::{
    AckResult, AskParentParams, AskParentResult, ForkParams, METHOD_ACCEPT_RUN, METHOD_ASK_PARENT,
    METHOD_CANCEL_AGENT, METHOD_FORK_AGENT, METHOD_GET_RUN, METHOD_READ_MESSAGES,
    METHOD_REJECT_RUN, METHOD_REPORT_PROGRESS, METHOD_SEND_MESSAGE, METHOD_SPAWN_AGENT,
    METHOD_WAIT_AGENT, ReadMessagesResult, RejectParams, ReportProgressParams, RunRef, RunView,
    SendMessageParams, SpawnParams, SpawnResult, WaitParams, WaitResult,
};
use so_core::{
    Config, EventType, Harness, Isolation, MessageKind, Paths, Platform, RunState, new_id,
};
use so_store::{RunRow, Store};
use so_supervisor::notify::{Sink, SinkFuture};
use so_supervisor::{
    ClientError, Supervisor, SupervisorClient, SupervisorHandle, SupervisorOptions,
    default_handlers,
};
use tempfile::TempDir;

const TRANSCRIPT: &str = include_str!("../../so-harness/tests/fixtures/claude/transcript.jsonl");
const PROMPT: &str = "do the task";

fn fake_harness() -> PathBuf {
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
    let p = common::sibling_bin("fake-harness");
    assert!(p.exists(), "{} missing", p.display());
    p
}

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

fn turn() -> Vec<Value> {
    first_turn().iter().map(|m| json!({"stdout": m})).collect()
}

fn expect(text: &str) -> Value {
    json!({"expect_stdin_contains": text})
}

/// prompt, the steps in between, then one successful turn.
fn ok_script(between: &[Value]) -> String {
    let mut steps = vec![expect(PROMPT)];
    steps.extend_from_slice(between);
    steps.extend(turn());
    script(&steps)
}

/// A run that reads the prompt and then waits to be cancelled.
fn idle_script() -> String {
    // After its last step the fake harness drains stdin until it is closed.
    script(&[expect(PROMPT), json!({"stdout": first_turn()[0]})])
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
    assert!(out.status.success(), "git {args:?}");
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
        let mut config = Config::default();
        config.notifications.desktop = false;
        Env {
            paths: Paths::from_dir(cfg),
            sock: dir.path().join("run/s.sock"),
            repo,
            home,
            config,
            dir,
        }
    }

    /// A claude account. The fake harness replays `first`, or `resume` when started with
    /// `--resume`, or `fork` when also started with `--fork-session`. Every launch logs
    /// its argv and stdin to `<name>.log`.
    fn account(&self, name: &str, cap: u32, first: &str, resume: Option<&str>, fork: Option<&str>) {
        let write = |tag: &str, text: &str| {
            let p = self.dir.path().join(format!("{name}.{tag}.jsonl"));
            std::fs::write(&p, text).unwrap();
            p
        };
        let first_p = write("first", first);
        let resume_p = write("resume", resume.unwrap_or(first));
        let fork_p = write("fork", fork.unwrap_or(first));
        let wrapper = self.dir.path().join(format!("{name}.sh"));
        std::fs::write(
            &wrapper,
            format!(
                "#!/bin/sh\nexport FAKE_HARNESS_LOG={log}\nS={first}\ncase \" $* \" in\n  *\" --fork-session \"*) S={fork};;\n  *\" --resume \"*) S={resume};;\nesac\nexport FAKE_HARNESS_SCRIPT=$S\nexec {fake} \"$@\"\n",
                log = self.log_path(name).display(),
                first = first_p.display(),
                resume = resume_p.display(),
                fork = fork_p.display(),
                fake = fake_harness().display()
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

    fn simple(&self, name: &str, cap: u32, first: &str) {
        self.account(name, cap, first, None, None);
    }

    fn log_path(&self, name: &str) -> PathBuf {
        self.dir.path().join(format!("{name}.log"))
    }

    /// The recorded launches of an account: `(argv, stdin lines)`.
    fn launches(&self, name: &str) -> Vec<(Vec<String>, Vec<String>)> {
        let text = std::fs::read_to_string(self.log_path(name)).unwrap_or_default();
        let mut out: Vec<(Vec<String>, Vec<String>)> = Vec::new();
        for l in text.lines() {
            let v: Value = serde_json::from_str(l).unwrap();
            if let Some(a) = v["argv"].as_array() {
                let argv = a.iter().map(|s| s.as_str().unwrap().to_string()).collect();
                out.push((argv, Vec::new()));
            } else if let Some(s) = v["stdin"].as_str() {
                out.last_mut().unwrap().1.push(s.to_string());
            }
        }
        out
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

    /// What `superorchestrator mcp` does inside a run: a connection that acts for it.
    async fn as_run(&self, h: &SupervisorHandle, run: &str) -> SupervisorClient {
        let c = SupervisorClient::connect(h.socket_path()).await.unwrap();
        c.hello(
            "mcp",
            self.repo.to_str().unwrap(),
            Some(run.to_string()),
            None,
        )
        .await
        .unwrap();
        c
    }

    fn params(&self, account: &str) -> SpawnParams {
        SpawnParams {
            prompt: Some(PROMPT.into()),
            harness: Some("claude".into()),
            account: Some(account.into()),
            cwd: Some(self.repo.to_str().unwrap().into()),
            isolation: Some(Isolation::None),
            ..SpawnParams::default()
        }
    }
}

async fn spawn(c: &SupervisorClient, p: SpawnParams) -> String {
    c.call::<_, SpawnResult>(METHOD_SPAWN_AGENT, p)
        .await
        .unwrap()
        .run_id
}

async fn wait_for(c: &SupervisorClient, runs: &[&str], ms: u64) -> WaitResult {
    c.call(
        METHOD_WAIT_AGENT,
        WaitParams {
            runs: runs.iter().map(|r| r.to_string()).collect(),
            timeout_ms: Some(ms),
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

async fn wait_state(c: &SupervisorClient, run: &str, want: RunState) -> RunView {
    for _ in 0..400 {
        let v = view(c, run).await;
        if v.state == want {
            return v;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!(
        "run {run} never reached {want}; it is {}",
        view(c, run).await.state
    );
}

/// Reads the orchestrator's messages until the run's `result` message has arrived: it is
/// stored just after the run's state changes to `pending_review`.
async fn read_until_result(c: &SupervisorClient) -> ReadMessagesResult {
    let mut all = ReadMessagesResult::default();
    for _ in 0..200 {
        let r: ReadMessagesResult = c.call(METHOD_READ_MESSAGES, json!({})).await.unwrap();
        all.messages.extend(r.messages);
        if all.messages.iter().any(|m| m.kind == MessageKind::Result) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    all
}

async fn send(c: &SupervisorClient, run: &str, text: &str, kind: Option<MessageKind>) {
    let r: AckResult = c
        .call(
            METHOD_SEND_MESSAGE,
            SendMessageParams {
                run: run.into(),
                text: text.into(),
                kind,
            },
        )
        .await
        .unwrap();
    assert!(r.ok);
}

fn count(v: &RunView, t: EventType) -> usize {
    v.recent_events.iter().filter(|e| e.event_type == t).count()
}

async fn cancel(c: &SupervisorClient, run: &str) {
    let _: Value = c
        .call(METHOD_CANCEL_AGENT, RunRef { run: run.into() })
        .await
        .unwrap();
}

// ----------------------------------------------------------------- messages

#[tokio::test]
async fn send_message_reaches_a_live_run() {
    let env = Env::new();
    env.simple("claude-a", 2, &ok_script(&[expect("hello child")]));
    let h = env.start().await;
    let c = env.client(&h).await;
    let id = spawn(&c, env.params("claude-a")).await;
    wait_state(&c, &id, RunState::Running).await;
    send(&c, &id, "hello child", None).await;
    // The harness only finishes once it read the text from stdin.
    let w = wait_for(&c, &[&id], 20_000).await;
    assert_eq!(w.finished, std::slice::from_ref(&id));
    let v = wait_state(&c, &id, RunState::Succeeded).await;
    assert_eq!(count(&v, EventType::MessageSent), 1);
    let sent = v
        .recent_events
        .iter()
        .find(|e| e.event_type == EventType::MessageSent)
        .unwrap();
    assert_eq!(sent.data["kind"], "message");
    assert_eq!(sent.data["to"], id);
    assert_eq!(v.messages.len(), 1);
    assert_eq!(v.messages[0].body, "hello child");
    assert!(
        env.launches("claude-a")[0]
            .1
            .iter()
            .any(|l| l.contains("hello child"))
    );
    // Delivered live, so nothing is left unread for the run.
    let unread = env
        .store()
        .unread_messages(&v.messages_session(&env), Some(&id), 10)
        .unwrap();
    assert!(unread.is_empty());
    h.shutdown().await;
}

trait SessionOf {
    fn messages_session(&self, env: &Env) -> String;
}

impl SessionOf for RunView {
    fn messages_session(&self, env: &Env) -> String {
        env.store()
            .get_run(&self.run_id)
            .unwrap()
            .unwrap()
            .session_id
    }
}

#[tokio::test]
async fn send_message_to_a_finished_run_resumes_its_session() {
    let env = Env::new();
    let resume = script(
        &[expect("one more thing")]
            .into_iter()
            .chain(turn())
            .collect::<Vec<_>>(),
    );
    env.account("claude-a", 2, &ok_script(&[]), Some(&resume), None);
    let h = env.start().await;
    let c = env.client(&h).await;
    let id = spawn(&c, env.params("claude-a")).await;
    wait_for(&c, &[&id], 20_000).await;
    wait_state(&c, &id, RunState::Succeeded).await;
    let native = env
        .store()
        .get_run(&id)
        .unwrap()
        .unwrap()
        .native_session_id
        .unwrap();

    send(&c, &id, "one more thing", None).await;
    // Back to running, then finished again by the resumed session's turn.
    for _ in 0..200 {
        if env.launches("claude-a").len() == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let v = loop {
        let v = view(&c, &id).await;
        if v.state == RunState::Succeeded && count(&v, EventType::AttemptStarted) == 2 {
            break v;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    let launches = env.launches("claude-a");
    assert_eq!(launches.len(), 2);
    let argv = &launches[1].0;
    let i = argv.iter().position(|a| a == "--resume").unwrap();
    assert_eq!(argv[i + 1], native);
    assert!(!argv.contains(&"--fork-session".to_string()));
    assert!(launches[1].1.iter().any(|l| l.contains("one more thing")));
    assert!(
        v.recent_events
            .iter()
            .any(|e| e.event_type == EventType::StateChanged
                && e.data["from"] == "succeeded"
                && e.data["to"] == "running")
    );
    h.shutdown().await;
}

#[tokio::test]
async fn send_message_to_a_queued_run_is_delivered_at_start() {
    let env = Env::new();
    env.simple(
        "claude-a",
        1,
        &script(
            &[expect("queued note")]
                .into_iter()
                .chain(turn())
                .collect::<Vec<_>>(),
        ),
    );
    let h = env.start().await;
    let c = env.client(&h).await;
    // The first run holds the only slot.
    env.simple("claude-b", 1, &idle_script());
    let blocker = spawn(&c, env.params("claude-b")).await;
    wait_state(&c, &blocker, RunState::Running).await;
    // Same account cap: use claude-b for the queued run too.
    env.simple("claude-b", 1, &ok_script(&[]));
    let queued = spawn(&c, env.params("claude-b")).await;
    assert_eq!(view(&c, &queued).await.state, RunState::Queued);
    send(&c, &queued, "queued note", None).await;
    cancel(&c, &blocker).await;
    let w = wait_for(&c, &[&queued], 20_000).await;
    assert_eq!(w.finished, std::slice::from_ref(&queued));
    // The prompt the harness read carried the note.
    let launches = env.launches("claude-b");
    assert!(
        launches
            .last()
            .unwrap()
            .1
            .iter()
            .any(|l| l.contains("queued note"))
    );
    h.shutdown().await;
}

#[tokio::test]
async fn read_messages_returns_unread_once() {
    let env = Env::new();
    env.simple("claude-a", 2, &idle_script());
    let h = env.start().await;
    let c = env.client(&h).await;
    let id = spawn(&c, env.params("claude-a")).await;
    wait_state(&c, &id, RunState::Running).await;
    let child = env.as_run(&h, &id).await;
    let _: AckResult = child
        .call(
            METHOD_REPORT_PROGRESS,
            ReportProgressParams {
                step: "build".into(),
                status: "started".into(),
                note: Some("n".into()),
            },
        )
        .await
        .unwrap();
    let first: ReadMessagesResult = c.call(METHOD_READ_MESSAGES, json!({})).await.unwrap();
    assert_eq!(first.messages.len(), 1);
    assert_eq!(first.messages[0].kind, MessageKind::Progress);
    assert_eq!(first.messages[0].from_run.as_deref(), Some(id.as_str()));
    assert!(first.messages[0].body.contains("build"));
    let second: ReadMessagesResult = c.call(METHOD_READ_MESSAGES, json!({})).await.unwrap();
    assert!(second.messages.is_empty());
    // The child reads what is addressed to it.
    send(&c, &id, "psst", None).await;
    let theirs: ReadMessagesResult = child.call(METHOD_READ_MESSAGES, json!({})).await.unwrap();
    assert!(
        theirs
            .messages
            .iter()
            .all(|m| m.to_run.as_deref() == Some(id.as_str()))
    );
    cancel(&c, &id).await;
    h.shutdown().await;
}

#[tokio::test]
async fn report_progress_wakes_wait_agent() {
    let env = Env::new();
    env.simple("claude-a", 2, &idle_script());
    let h = env.start().await;
    let c = Arc::new(env.client(&h).await);
    let id = spawn(&c, env.params("claude-a")).await;
    wait_state(&c, &id, RunState::Running).await;
    let child = env.as_run(&h, &id).await;
    let waiter = {
        let (c, id) = (c.clone(), id.clone());
        tokio::spawn(async move { wait_for(&c, &[&id], 20_000).await })
    };
    // Progress posted before the waiter subscribed is not replayed, so keep reporting
    // until the wait has returned.
    let reporter = {
        let child = Arc::new(child);
        tokio::spawn(async move {
            loop {
                let _: AckResult = child
                    .call(
                        METHOD_REPORT_PROGRESS,
                        ReportProgressParams {
                            step: "tests".into(),
                            status: "passing".into(),
                            note: None,
                        },
                    )
                    .await
                    .unwrap();
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
    };
    let w = tokio::time::timeout(Duration::from_secs(10), waiter)
        .await
        .unwrap()
        .unwrap();
    reporter.abort();
    assert!(w.finished.is_empty() && !w.timed_out);
    assert!(!w.updates.is_empty());
    assert_eq!(w.updates[0].event_type, EventType::Progress);
    assert_eq!(w.updates[0].data["step"], "tests");
    cancel(&c, &id).await;
    h.shutdown().await;
}

// ------------------------------------------------------------------ ask_parent

#[tokio::test]
async fn ask_parent_blocks_until_answered() {
    let mut env = Env::new();
    env.config.runs.stall_ms = 400;
    env.simple("claude-a", 2, &idle_script());
    let h = env.start().await;
    let c = env.client(&h).await;
    let id = spawn(&c, env.params("claude-a")).await;
    wait_state(&c, &id, RunState::Running).await;
    let child = env.as_run(&h, &id).await;
    let asker = tokio::spawn(async move {
        child
            .call::<_, AskParentResult>(
                METHOD_ASK_PARENT,
                AskParentParams {
                    question: "which database?".into(),
                    options: Some(vec!["pg".into(), "sqlite".into()]),
                },
            )
            .await
            .unwrap()
    });
    let w = wait_for(&c, &[&id], 20_000).await;
    assert_eq!(w.updates.len(), 1);
    assert_eq!(w.updates[0].event_type, EventType::QuestionAsked);
    assert_eq!(w.updates[0].data["question"], "which database?");
    assert_eq!(view(&c, &id).await.state, RunState::Waiting);
    // A waiting run is not stall-checked, however long it takes.
    // (three stall periods; the state is checked all along, not just at the end)
    let until = tokio::time::Instant::now() + Duration::from_millis(1200);
    while tokio::time::Instant::now() < until {
        assert_eq!(view(&c, &id).await.state, RunState::Waiting);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(!asker.is_finished());
    // The unanswered question wakes the next wait at once.
    let again = wait_for(&c, &[&id], 20_000).await;
    assert_eq!(again.updates[0].event_type, EventType::QuestionAsked);

    send(&c, &id, "sqlite", Some(MessageKind::Answer)).await;
    let answer = tokio::time::timeout(Duration::from_secs(10), asker)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(answer.answer, "sqlite");
    assert!(!answer.timed_out);
    wait_state(&c, &id, RunState::Running).await;
    // (`question_answered` is recorded just after the run is running again.)
    let v = loop {
        let v = view(&c, &id).await;
        if count(&v, EventType::QuestionAnswered) == 1 {
            break v;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    assert_eq!(count(&v, EventType::QuestionAsked), 1);
    // Answered: the question no longer wakes a wait.
    let quiet = wait_for(&c, &[&id], 300).await;
    assert!(quiet.timed_out && quiet.updates.is_empty(), "{quiet:?}");
    cancel(&c, &id).await;
    h.shutdown().await;
}

#[tokio::test]
async fn ask_parent_times_out() {
    let mut env = Env::new();
    env.config.runs.ask_timeout = so_core::HumanDuration::from_millis(300);
    env.simple("claude-a", 2, &idle_script());
    let h = env.start().await;
    let c = env.client(&h).await;
    let id = spawn(&c, env.params("claude-a")).await;
    wait_state(&c, &id, RunState::Running).await;
    let child = env.as_run(&h, &id).await;
    let r: AskParentResult = child
        .call(
            METHOD_ASK_PARENT,
            AskParentParams {
                question: "anyone?".into(),
                options: None,
            },
        )
        .await
        .unwrap();
    assert!(r.timed_out);
    assert_eq!(r.answer, "timeout");
    let v = wait_state(&c, &id, RunState::Running).await;
    assert_eq!(count(&v, EventType::QuestionAsked), 1);
    cancel(&c, &id).await;
    h.shutdown().await;
}

// ------------------------------------------------------------------ nesting

#[tokio::test]
async fn a_depth_one_run_spawns_and_waits_for_a_depth_two_child() {
    let env = Env::new();
    env.simple("claude-a", 4, &idle_script());
    env.simple("claude-b", 4, &ok_script(&[]));
    let h = env.start().await;
    let c = env.client(&h).await;
    let parent = spawn(&c, env.params("claude-a")).await;
    wait_state(&c, &parent, RunState::Running).await;

    // The run's own MCP process: same session, acting for the run.
    let mine = Arc::new(env.as_run(&h, &parent).await);
    let mut p = env.params("claude-b");
    p.wait = true;
    let r: SpawnResult = mine.call(METHOD_SPAWN_AGENT, p).await.unwrap();
    assert_eq!(r.state, RunState::Succeeded);
    let kid = view(&c, &r.run_id).await;
    assert_eq!(kid.depth, 2);
    assert_eq!(kid.parent_run.as_deref(), Some(parent.as_str()));

    // Waiting on a child that is still running puts the run in `waiting`.
    env.simple("claude-c", 4, &idle_script());
    let mut p = env.params("claude-c");
    p.wait = false;
    let slow = spawn(&mine, p).await;
    wait_state(&c, &slow, RunState::Running).await;
    let waiter = {
        let (mine, slow) = (mine.clone(), slow.clone());
        tokio::spawn(async move { wait_for(&mine, &[&slow], 20_000).await })
    };
    wait_state(&c, &parent, RunState::Waiting).await;
    // Cancelling the parent covers its subtree.
    cancel(&c, &parent).await;
    wait_state(&c, &parent, RunState::Cancelled).await;
    wait_state(&c, &slow, RunState::Cancelled).await;
    drop(waiter);
    h.shutdown().await;
}

// -------------------------------------------------------------- review gate

#[tokio::test]
async fn review_passes_then_accept() {
    let env = Env::new();
    env.simple("claude-a", 2, &ok_script(&[]));
    let h = env.start().await;
    let c = env.client(&h).await;
    let mut p = env.params("claude-a");
    p.review = true;
    p.checks = vec!["cmd:true".into()];
    let id = spawn(&c, p).await;
    // The gate wakes wait_agent.
    let w = wait_for(&c, &[&id], 20_000).await;
    assert!(w.finished.is_empty());
    assert_eq!(w.updates.len(), 1);
    assert_eq!(w.updates[0].data["to"], "pending_review");
    let v = wait_state(&c, &id, RunState::PendingReview).await;
    let check = v
        .recent_events
        .iter()
        .find(|e| e.event_type == EventType::CheckResult)
        .unwrap();
    assert_eq!(check.data["name"], "cmd:true");
    assert_eq!(check.data["ok"], true);
    assert!(v.result.is_some());
    let msgs = read_until_result(&c).await;
    assert!(
        msgs.messages
            .iter()
            .any(|m| m.kind == MessageKind::Result && m.body.contains("ready for review"))
    );
    // Only an accepted run succeeds.
    assert_eq!(wait_for(&c, &[&id], 200).await.updates.len(), 1);

    let _: AckResult = c
        .call(METHOD_ACCEPT_RUN, RunRef { run: id.clone() })
        .await
        .unwrap();
    let v = wait_state(&c, &id, RunState::Succeeded).await;
    assert_eq!(count(&v, EventType::ReviewAccepted), 1);
    assert!(v.result.is_some());
    // Accepting twice is an error.
    let e = c
        .call::<_, AckResult>(METHOD_ACCEPT_RUN, RunRef { run: id.clone() })
        .await
        .unwrap_err();
    assert!(matches!(e, ClientError::Rpc(_)));
    h.shutdown().await;
}

#[tokio::test]
async fn failed_check_is_sent_back_once_then_reported() {
    let env = Env::new();
    let steps: Vec<Value> = [expect(PROMPT)]
        .into_iter()
        .chain(turn())
        .chain([expect("did not pass")])
        .chain(turn())
        .collect();
    env.simple("claude-a", 2, &script(&steps));
    let h = env.start().await;
    let c = env.client(&h).await;
    let mut p = env.params("claude-a");
    p.review = true;
    p.checks = vec!["cmd:echo broken >&2; false".into()];
    let id = spawn(&c, p).await;
    let v = wait_state(&c, &id, RunState::PendingReview).await;
    assert_eq!(count(&v, EventType::CheckResult), 2);
    assert!(
        v.recent_events
            .iter()
            .all(|e| e.event_type != EventType::CheckResult || e.data["ok"] == false)
    );
    let sent: Vec<String> = env.launches("claude-a")[0]
        .1
        .iter()
        .filter(|l| l.contains("did not pass"))
        .cloned()
        .collect();
    assert_eq!(sent.len(), 1, "the failure goes to the child once");
    assert!(sent[0].contains("broken"));
    let msgs = read_until_result(&c).await;
    let report = msgs
        .messages
        .iter()
        .find(|m| m.kind == MessageKind::Result)
        .unwrap();
    assert!(report.body.contains("checks failed") && report.body.contains("broken"));
    cancel(&c, &id).await;
    h.shutdown().await;
}

#[tokio::test]
async fn child_fixes_the_check_after_the_message() {
    let env = Env::new();
    let marker = env.repo.join("fixed.marker");
    let steps: Vec<Value> = [expect(PROMPT)]
        .into_iter()
        .chain(turn())
        .chain([
            expect("did not pass"),
            json!({"shell": format!("touch {}", marker.display())}),
        ])
        .chain(turn())
        .collect();
    env.simple("claude-a", 2, &script(&steps));
    let h = env.start().await;
    let c = env.client(&h).await;
    let mut p = env.params("claude-a");
    p.review = true;
    p.checks = vec![format!("cmd:test -f {}", marker.display())];
    let id = spawn(&c, p).await;
    let v = wait_state(&c, &id, RunState::PendingReview).await;
    let oks: Vec<bool> = v
        .recent_events
        .iter()
        .filter(|e| e.event_type == EventType::CheckResult)
        .map(|e| e.data["ok"].as_bool().unwrap())
        .collect();
    assert_eq!(oks, [false, true]);
    let msgs = read_until_result(&c).await;
    assert!(
        msgs.messages
            .iter()
            .any(|m| m.body.contains("ready for review") && !m.body.contains("failed"))
    );
    cancel(&c, &id).await;
    h.shutdown().await;
}

#[tokio::test]
async fn reject_sends_feedback_on_the_same_session() {
    let env = Env::new();
    let resume = script(
        &[expect("rename it")]
            .into_iter()
            .chain(turn())
            .collect::<Vec<_>>(),
    );
    env.account("claude-a", 2, &ok_script(&[]), Some(&resume), None);
    let h = env.start().await;
    let c = env.client(&h).await;
    let mut p = env.params("claude-a");
    p.review = true;
    let id = spawn(&c, p).await;
    wait_state(&c, &id, RunState::PendingReview).await;
    let native = env
        .store()
        .get_run(&id)
        .unwrap()
        .unwrap()
        .native_session_id
        .unwrap();

    let _: AckResult = c
        .call(
            METHOD_REJECT_RUN,
            RejectParams {
                run: id.clone(),
                feedback: "rename it".into(),
            },
        )
        .await
        .unwrap();
    // Running again on the resumed session, and at the gate again after its next turn.
    let v = loop {
        let v = view(&c, &id).await;
        if v.state == RunState::PendingReview && count(&v, EventType::AttemptStarted) == 2 {
            break v;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    assert_eq!(count(&v, EventType::ReviewRejected), 1);
    assert!(
        v.recent_events
            .iter()
            .any(|e| e.event_type == EventType::StateChanged
                && e.data["from"] == "pending_review"
                && e.data["to"] == "running")
    );
    let launches = env.launches("claude-a");
    let argv = &launches[1].0;
    let i = argv.iter().position(|a| a == "--resume").unwrap();
    assert_eq!(argv[i + 1], native);
    assert!(launches[1].1.iter().any(|l| l.contains("rename it")));
    let _: AckResult = c
        .call(METHOD_ACCEPT_RUN, RunRef { run: id.clone() })
        .await
        .unwrap();
    wait_state(&c, &id, RunState::Succeeded).await;
    h.shutdown().await;
}

// ------------------------------------------------------------------- stalls

struct Recorder(Arc<Mutex<Vec<(String, String)>>>);

impl Sink for Recorder {
    fn send<'a>(&'a self, title: &'a str, body: &'a str) -> SinkFuture<'a> {
        Box::pin(async move {
            self.0
                .lock()
                .unwrap()
                .push((title.to_string(), body.to_string()));
        })
    }
}

#[tokio::test]
async fn a_silent_run_is_nudged_three_times_then_escalated_and_recovers() {
    let mut env = Env::new();
    env.config.runs.stall_ms = 150;
    let steps = vec![
        expect(PROMPT),
        expect("Status?"),
        expect("Status?"),
        expect("Status?"),
        json!({"sleep_ms": 1000}),
    ];
    let steps: Vec<Value> = steps.into_iter().chain(turn()).collect();
    env.simple("claude-a", 2, &script(&steps));
    let h = env.start().await;
    let sent = Arc::new(Mutex::new(Vec::new()));
    h.state()
        .runs
        .set_notify_sinks(vec![Arc::new(Recorder(sent.clone()))]);
    let c = env.client(&h).await;
    let mut p = env.params("claude-a");
    p.name = Some("stally".into());
    let id = spawn(&c, p).await;

    wait_state(&c, &id, RunState::Stalled).await;
    wait_state(&c, &id, RunState::Escalated).await;
    // The `escalated` event and the notification follow the state change.
    let v = loop {
        let v = view(&c, &id).await;
        if count(&v, EventType::Escalated) == 1 && !sent.lock().unwrap().is_empty() {
            break v;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    assert_eq!(count(&v, EventType::StallDetected), 1);
    assert_eq!(count(&v, EventType::NudgeSent), 3);
    assert_eq!(count(&v, EventType::Escalated), 1);
    {
        let sent = sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, "superorchestrator: escalated");
        assert!(sent[0].1.starts_with("stally"));
        assert!(!sent[0].1.contains(PROMPT));
    }
    let msgs = env.store().list_messages_by_run(&id, 100).unwrap();
    assert_eq!(
        msgs.iter().filter(|m| m.kind == MessageKind::Nudge).count(),
        3
    );
    assert!(
        msgs.iter()
            .any(|m| m.kind == MessageKind::Message && m.body.contains("stalled"))
    );

    // The harness speaks again: the run is running (and then finishes).
    wait_state(&c, &id, RunState::Succeeded).await;
    let v = view(&c, &id).await;
    assert!(
        v.recent_events
            .iter()
            .any(|e| e.event_type == EventType::StateChanged
                && e.data["from"] == "escalated"
                && e.data["to"] == "running")
    );
    assert_eq!(
        env.launches("claude-a")[0]
            .1
            .iter()
            .filter(|l| l.contains("Status?"))
            .count(),
        3
    );
    h.shutdown().await;
}

// ------------------------------------------------------------ process death

#[tokio::test]
async fn a_dead_process_is_resumed_once() {
    let env = Env::new();
    let first = script(&[
        expect(PROMPT),
        json!({"stdout": first_turn()[0]}),
        json!({"exit": 1}),
    ]);
    let resume = script(
        &[expect("stopped unexpectedly")]
            .into_iter()
            .chain(turn())
            .collect::<Vec<_>>(),
    );
    env.account("claude-a", 2, &first, Some(&resume), None);
    let h = env.start().await;
    let c = env.client(&h).await;
    let id = spawn(&c, env.params("claude-a")).await;
    let w = wait_for(&c, &[&id], 20_000).await;
    assert_eq!(w.finished, std::slice::from_ref(&id));
    let v = wait_state(&c, &id, RunState::Succeeded).await;
    assert_eq!(count(&v, EventType::AttemptStarted), 2);
    assert_eq!(env.launches("claude-a").len(), 2);
    assert!(
        env.launches("claude-a")[1]
            .0
            .contains(&"--resume".to_string())
    );
    h.shutdown().await;
}

#[tokio::test]
async fn a_second_death_fails_the_run() {
    let env = Env::new();
    let dies = script(&[
        expect(PROMPT),
        json!({"stdout": first_turn()[0]}),
        json!({"exit": 1}),
    ]);
    let dies_again = script(&[expect("stopped unexpectedly"), json!({"exit": 1})]);
    env.account("claude-a", 2, &dies, Some(&dies_again), None);
    let h = env.start().await;
    let c = env.client(&h).await;
    let id = spawn(&c, env.params("claude-a")).await;
    let v = wait_state(&c, &id, RunState::Failed).await;
    assert_eq!(count(&v, EventType::AttemptStarted), 2);
    assert_eq!(env.launches("claude-a").len(), 2);
    h.shutdown().await;
}

// --------------------------------------------------------------------- fork

#[tokio::test]
async fn fork_resumes_a_copy_of_the_session() {
    let env = Env::new();
    let fork = script(
        &[expect("take another path")]
            .into_iter()
            .chain(turn())
            .collect::<Vec<_>>(),
    );
    env.account("claude-a", 2, &ok_script(&[]), None, Some(&fork));
    let h = env.start().await;
    let c = env.client(&h).await;
    let id = spawn(&c, env.params("claude-a")).await;
    wait_state(&c, &id, RunState::Succeeded).await;
    let native = env
        .store()
        .get_run(&id)
        .unwrap()
        .unwrap()
        .native_session_id
        .unwrap();

    let r: SpawnResult = c
        .call(
            METHOD_FORK_AGENT,
            ForkParams {
                run: id.clone(),
                prompt: "take another path".into(),
            },
        )
        .await
        .unwrap();
    assert_ne!(r.run_id, id);
    wait_state(&c, &r.run_id, RunState::Succeeded).await;
    let launches = env.launches("claude-a");
    assert_eq!(launches.len(), 2);
    let argv = &launches[1].0;
    let i = argv.iter().position(|a| a == "--resume").unwrap();
    assert_eq!(argv[i + 1], native);
    assert!(argv.contains(&"--fork-session".to_string()));
    let forked = env.store().get_run(&r.run_id).unwrap().unwrap();
    let source = env.store().get_run(&id).unwrap().unwrap();
    assert_eq!(forked.parent_run_id, source.parent_run_id);
    assert_eq!(forked.session_id, source.session_id);
    assert_eq!(forked.account, source.account);
    // The source is untouched.
    assert_eq!(source.state, RunState::Succeeded);
    h.shutdown().await;
}

#[tokio::test]
async fn fork_of_an_agy_run_names_the_harness() {
    let env = Env::new();
    let h = env.start().await;
    let c = env.client(&h).await;
    let store = env.store();
    let session = store
        .list_sessions(so_store::SessionFilter::Active, 10)
        .unwrap()
        .remove(0);
    let row = RunRow {
        id: new_id(),
        session_id: session.id,
        parent_run_id: None,
        depth: 1,
        name: None,
        agent: None,
        choice_id: None,
        harness: Harness::Agy,
        account: Some("agy-a".into()),
        model: None,
        effort: None,
        cwd: Some(env.repo.to_string_lossy().into_owned()),
        worktree_path: None,
        branch: None,
        native_session_id: Some("conv-1".into()),
        state: RunState::Succeeded,
        review: false,
        output_schema: None,
        result_json: None,
        error_kind: None,
        prompt: None,
        created_at: chrono::Utc::now(),
        started_at: None,
        finished_at: Some(chrono::Utc::now()),
        isolation: Isolation::None,
        max_turns: None,
        worktree_base: None,
        worktree_repo: None,
        timeout_ms: None,
    };
    store.insert_run(&row).unwrap();
    let e = c
        .call::<_, SpawnResult>(
            METHOD_FORK_AGENT,
            ForkParams {
                run: row.id.clone(),
                prompt: "x".into(),
            },
        )
        .await
        .unwrap_err();
    match e {
        ClientError::Rpc(r) => assert!(r.message.contains("agy"), "{}", r.message),
        other => panic!("{other:?}"),
    }
    h.shutdown().await;
}

// ------------------------------------------------------------ review follow-ups

fn commit_step() -> Value {
    json!({"shell": "git -c user.name=t -c user.email=t@e commit -q --allow-empty -m work"})
}

#[tokio::test]
async fn a_review_run_at_the_gate_frees_its_slot_for_the_queue() {
    let env = Env::new();
    // One slot: the reviewing run, then a queued run that needs it.
    env.account("claude-a", 1, &ok_script(&[]), None, None);
    let h = env.start().await;
    let c = env.client(&h).await;
    let mut p = env.params("claude-a");
    p.review = true;
    let reviewed = spawn(&c, p).await;
    let queued = spawn(&c, env.params("claude-a")).await;
    wait_state(&c, &reviewed, RunState::PendingReview).await;
    // The queued run starts and finishes without anyone touching the reviewed one.
    wait_state(&c, &queued, RunState::Succeeded).await;
    assert_eq!(view(&c, &reviewed).await.state, RunState::PendingReview);
    h.shutdown().await;
}

#[tokio::test]
async fn a_message_to_a_run_at_the_gate_counts_as_a_rejection() {
    let env = Env::new();
    let resume = script(
        &[expect("change it")]
            .into_iter()
            .chain(turn())
            .collect::<Vec<_>>(),
    );
    env.account("claude-a", 2, &ok_script(&[]), Some(&resume), None);
    let h = env.start().await;
    let c = env.client(&h).await;
    let mut p = env.params("claude-a");
    p.review = true;
    let id = spawn(&c, p).await;
    wait_state(&c, &id, RunState::PendingReview).await;
    send(&c, &id, "change it", None).await;
    let v = loop {
        let v = view(&c, &id).await;
        if count(&v, EventType::AttemptStarted) == 2 && v.state == RunState::PendingReview {
            break v;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    assert_eq!(count(&v, EventType::ReviewRejected), 1);
    h.shutdown().await;
}

#[tokio::test]
async fn accept_removes_a_kept_worktree_once_its_branch_is_merged() {
    let env = Env::new();
    env.simple("claude-a", 2, &ok_script(&[commit_step()]));
    let h = env.start().await;
    let c = env.client(&h).await;
    let mut p = env.params("claude-a");
    p.review = true;
    p.isolation = Some(Isolation::Worktree);
    let unmerged = spawn(&c, p.clone()).await;
    wait_state(&c, &unmerged, RunState::PendingReview).await;
    let _: AckResult = c
        .call(
            METHOD_ACCEPT_RUN,
            RunRef {
                run: unmerged.clone(),
            },
        )
        .await
        .unwrap();
    // Committed work that is neither merged nor pushed stays.
    let v = wait_state(&c, &unmerged, RunState::Succeeded).await;
    assert!(Path::new(&v.worktree.clone().unwrap()).is_dir());

    let merged = spawn(&c, p).await;
    let v = wait_state(&c, &merged, RunState::PendingReview).await;
    let (wt, branch) = (v.worktree.clone().unwrap(), v.branch.clone().unwrap());
    assert!(Path::new(&wt).is_dir());
    git(&env.repo, &["merge", "-q", "--no-edit", &branch]);
    let _: AckResult = c
        .call(
            METHOD_ACCEPT_RUN,
            RunRef {
                run: merged.clone(),
            },
        )
        .await
        .unwrap();
    let v = wait_state(&c, &merged, RunState::Succeeded).await;
    assert!(
        !Path::new(&wt).exists(),
        "merged worktree should be removed"
    );
    assert_eq!(count(&v, EventType::WorktreeRemoved), 1);
    h.shutdown().await;
}

// ------------------------------------------------------------- resume details

#[tokio::test]
async fn a_resumed_worktree_run_gets_its_worktree_back_from_its_branch() {
    let env = Env::new();
    let marker = env.dir.path().join("resume.cwd");
    let resume = script(
        &[
            expect("again"),
            json!({"shell": format!("pwd > {}", marker.display())}),
        ]
        .into_iter()
        .chain(turn())
        .collect::<Vec<_>>(),
    );
    env.account(
        "claude-a",
        2,
        &ok_script(&[commit_step()]),
        Some(&resume),
        None,
    );
    let h = env.start().await;
    let c = env.client(&h).await;
    let mut p = env.params("claude-a");
    p.isolation = Some(Isolation::Worktree);
    let id = spawn(&c, p).await;
    let v = wait_state(&c, &id, RunState::Succeeded).await;
    let wt = v.worktree.clone().unwrap();
    // The directory is lost (the branch, with the work, stays).
    git(&env.repo, &["worktree", "remove", "--force", &wt]);
    assert!(!Path::new(&wt).exists());

    send(&c, &id, "again", None).await;
    for _ in 0..200 {
        if marker.exists() && view(&c, &id).await.state == RunState::Succeeded {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let ran_in = std::fs::read_to_string(&marker).unwrap();
    assert_eq!(
        Path::new(ran_in.trim()).canonicalize().unwrap(),
        Path::new(&wt).canonicalize().unwrap(),
        "resumed in its own worktree, not the original directory"
    );
    h.shutdown().await;
}

#[tokio::test]
async fn an_auto_removed_worktree_is_recreated_at_its_base_for_a_resume() {
    let env = Env::new();
    let marker = env.dir.path().join("untouched.cwd");
    let resume = script(
        &[
            expect("again"),
            json!({"shell": format!("pwd > {}", marker.display())}),
        ]
        .into_iter()
        .chain(turn())
        .collect::<Vec<_>>(),
    );
    // No commits: the worktree and its branch are removed when the run finishes.
    env.account("claude-a", 2, &ok_script(&[]), Some(&resume), None);
    let h = env.start().await;
    let c = env.client(&h).await;
    let mut p = env.params("claude-a");
    p.isolation = Some(Isolation::Worktree);
    let id = spawn(&c, p).await;
    let v = wait_state(&c, &id, RunState::Succeeded).await;
    let (wt, branch) = (v.worktree.clone().unwrap(), v.branch.clone().unwrap());
    // The state is visible a moment before the wrap-up removes the worktree.
    for _ in 0..200 {
        if !Path::new(&wt).exists() && git(&env.repo, &["branch", "--list", &branch]).is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(
        !Path::new(&wt).exists(),
        "the untouched worktree was removed"
    );
    assert!(git(&env.repo, &["branch", "--list", &branch]).is_empty());

    send(&c, &id, "again", None).await;
    for _ in 0..200 {
        if marker.exists() && view(&c, &id).await.state == RunState::Succeeded {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let ran_in = std::fs::read_to_string(&marker).unwrap();
    // (It is removed again once this untouched run finishes, so compare the text.)
    assert_eq!(ran_in.trim(), wt);
    h.shutdown().await;
}

#[tokio::test]
async fn a_run_that_cannot_be_resumed_keeps_its_state() {
    let env = Env::new();
    env.account("claude-a", 2, &ok_script(&[]), None, None);
    let h = env.start().await;
    let c = env.client(&h).await;
    let mut p = env.params("claude-a");
    p.isolation = Some(Isolation::Worktree);
    let id = spawn(&c, p).await;
    let v = wait_state(&c, &id, RunState::Succeeded).await;
    // The wrap-up removes the untouched worktree just after the state changes.
    let wt = v.worktree.unwrap();
    for _ in 0..200 {
        if !Path::new(&wt).exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    // Worktree and branch are gone, and the base commit cannot be found.
    env.store()
        .patch_run(
            &id,
            &so_store::RunPatch {
                worktree_base: Some(Some("0000000000000000000000000000000000000001".into())),
                ..so_store::RunPatch::default()
            },
        )
        .unwrap();
    let e = c
        .call::<_, AckResult>(
            METHOD_SEND_MESSAGE,
            SendMessageParams {
                run: id.clone(),
                text: "again".into(),
                kind: None,
            },
        )
        .await
        .unwrap_err();
    match e {
        ClientError::Rpc(r) => assert!(r.message.contains("no longer exists"), "{}", r.message),
        other => panic!("{other:?}"),
    }
    // Not failed, not reopened.
    let v = view(&c, &id).await;
    assert_eq!(v.state, RunState::Succeeded);
    assert_eq!(count(&v, EventType::AttemptStarted), 1);
    h.shutdown().await;
}

#[tokio::test]
async fn a_reject_right_after_the_gate_resumes_cleanly() {
    let env = Env::new();
    // The process lingers for a while after its input closes, so the teardown at the gate
    // takes time.
    let first = script(
        &[expect(PROMPT)]
            .into_iter()
            .chain(turn())
            .chain([json!({"sleep_ms": 800})])
            .collect::<Vec<_>>(),
    );
    let resume = script(
        &[expect("redo"), json!({"sleep_ms": 1500})]
            .into_iter()
            .chain(turn())
            .collect::<Vec<_>>(),
    );
    env.account("claude-a", 2, &first, Some(&resume), None);
    let h = env.start().await;
    // The gate stays busy announcing for 800 ms after the state change.
    h.state().runs.set_gate_delay(Duration::from_millis(800));
    let c = env.client(&h).await;
    let mut p = env.params("claude-a");
    p.review = true;
    let id = spawn(&c, p).await;
    wait_state(&c, &id, RunState::PendingReview).await;
    let asked = std::time::Instant::now();
    let _: AckResult = c
        .call(
            METHOD_REJECT_RUN,
            RejectParams {
                run: id.clone(),
                feedback: "redo".into(),
            },
        )
        .await
        .unwrap();
    // The state was visible while the gate was still being announced; the reject waited.
    assert!(
        asked.elapsed() >= Duration::from_millis(400),
        "reject did not wait for the gate: {:?}",
        asked.elapsed()
    );
    // The new process is the run's live one and its attempt is open, not torn down by
    // the old process's teardown.
    assert_eq!(h.state().runs.live_count(), 1);
    let attempts = env.store().list_attempts(&id, 10).unwrap();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0].outcome.as_deref(), Some("pending_review"));
    assert!(attempts[1].ended_at.is_none());
    // And it runs on to the gate again.
    let v = wait_state(&c, &id, RunState::PendingReview).await;
    // The gate is announced before the reject: pending_review, then running again.
    let moves: Vec<(String, String)> = v
        .recent_events
        .iter()
        .filter(|e| e.event_type == EventType::StateChanged)
        .map(|e| {
            (
                e.data["from"].as_str().unwrap_or("").to_string(),
                e.data["to"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect();
    let gate = moves.iter().position(|m| m.1 == "pending_review").unwrap();
    let back = moves
        .iter()
        .position(|m| m == &("pending_review".to_string(), "running".to_string()))
        .unwrap();
    assert!(gate < back, "{moves:?}");
    let events: Vec<EventType> = v.recent_events.iter().map(|e| e.event_type).collect();
    let gate_at = events
        .iter()
        .position(|t| *t == EventType::StateChanged)
        .unwrap();
    let rejected_at = events
        .iter()
        .position(|t| *t == EventType::ReviewRejected)
        .unwrap();
    assert!(gate_at < rejected_at);
    let rejected = v
        .recent_events
        .iter()
        .find(|e| e.event_type == EventType::ReviewRejected)
        .unwrap();
    assert_eq!(rejected.data["actor"], "orchestrator");
    // The parent was told about the gate before the reject happened.
    let msgs = read_until_result(&c).await;
    let result = msgs
        .messages
        .iter()
        .find(|m| m.kind == MessageKind::Result)
        .unwrap();
    assert!(result.created_at <= rejected.ts);
    h.shutdown().await;
}

#[tokio::test]
async fn a_message_sent_while_the_gate_tears_the_process_down_is_not_lost() {
    let env = Env::new();
    let first = script(
        &[expect(PROMPT)]
            .into_iter()
            .chain(turn())
            .chain([json!({"sleep_ms": 1200})])
            .collect::<Vec<_>>(),
    );
    let resume = script(
        &[expect("redo"), expect("late")]
            .into_iter()
            .chain(turn())
            .collect::<Vec<_>>(),
    );
    env.account("claude-a", 2, &first, Some(&resume), None);
    let h = env.start().await;
    let c = env.client(&h).await;
    let mut p = env.params("claude-a");
    p.review = true;
    let id = spawn(&c, p).await;
    // The turn is over once its usage is reported; the process then lingers 1.2 s.
    for _ in 0..400 {
        if view(&c, &id)
            .await
            .recent_events
            .iter()
            .any(|e| e.event_type == EventType::Usage)
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    // The gate has taken the process out of service but the run is not yet at the gate.
    for _ in 0..400 {
        if view(&c, &id).await.state == RunState::Running && h.state().runs.live_count() == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        view(&c, &id).await.state,
        RunState::Running,
        "still tearing down"
    );
    send(&c, &id, "late", None).await;
    wait_state(&c, &id, RunState::PendingReview).await;
    let _: AckResult = c
        .call(
            METHOD_REJECT_RUN,
            RejectParams {
                run: id.clone(),
                feedback: "redo".into(),
            },
        )
        .await
        .unwrap();
    wait_state(&c, &id, RunState::PendingReview).await;
    let launches = env.launches("claude-a");
    assert_eq!(launches.len(), 2);
    let stdin = &launches[1].1;
    assert!(stdin.iter().any(|l| l.contains("redo")));
    assert!(stdin.iter().any(|l| l.contains("late")), "{stdin:?}");
    h.shutdown().await;
}

#[tokio::test]
async fn a_message_to_a_finished_run_waits_in_the_queue_when_the_account_is_full() {
    let env = Env::new();
    let resume = script(
        &[expect("later")]
            .into_iter()
            .chain(turn())
            .collect::<Vec<_>>(),
    );
    env.account("claude-a", 1, &ok_script(&[]), Some(&resume), None);
    let h = env.start().await;
    let c = env.client(&h).await;
    let done = spawn(&c, env.params("claude-a")).await;
    wait_state(&c, &done, RunState::Succeeded).await;
    // Fill the only slot with a run that does not finish. It starts fresh, so give the
    // account a first script that idles.
    env.account("claude-a", 1, &idle_script(), Some(&resume), None);
    let blocker = spawn(&c, env.params("claude-a")).await;
    wait_state(&c, &blocker, RunState::Running).await;

    send(&c, &done, "later", None).await;
    let v = view(&c, &done).await;
    assert_eq!(v.state, RunState::Queued, "no free slot");
    // Cancelling the blocker frees the slot: the queue resumes the finished run.
    cancel(&c, &blocker).await;
    let v = loop {
        let v = view(&c, &done).await;
        if v.state == RunState::Succeeded && count(&v, EventType::AttemptStarted) == 2 {
            break v;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    let launches = env.launches("claude-a");
    let resumed = launches
        .iter()
        .find(|(argv, _)| argv.contains(&"--resume".to_string()))
        .expect("a resumed launch");
    assert!(resumed.1.iter().any(|l| l.contains("later")));
    assert!(
        v.recent_events
            .iter()
            .any(|e| e.event_type == EventType::StateChanged
                && e.data["from"] == "succeeded"
                && e.data["to"] == "queued")
    );
    h.shutdown().await;
}

#[tokio::test]
async fn a_message_sent_while_the_harness_is_launching_is_handed_over_when_it_is_up() {
    let env = Env::new();
    env.simple("claude-a", 2, &ok_script(&[expect("late note")]));
    let h = env.start().await;
    h.state().runs.set_launch_delay(Duration::from_millis(700));
    let c = env.client(&h).await;
    let id = spawn(&c, env.params("claude-a")).await;
    wait_state(&c, &id, RunState::Starting).await;
    // The prompt is already built; the message misses it and waits for the process.
    send(&c, &id, "late note", None).await;
    wait_state(&c, &id, RunState::Succeeded).await;
    assert!(
        env.launches("claude-a")[0]
            .1
            .iter()
            .any(|l| l.contains("late note"))
    );
    h.shutdown().await;
}

// ----------------------------------------------------------------- notifications

#[tokio::test]
async fn an_unanswered_question_raises_question_waiting() {
    let mut env = Env::new();
    env.config.runs.ask_timeout = so_core::HumanDuration::from_millis(1500);
    env.simple("claude-a", 2, &idle_script());
    let h = env.start().await;
    let sent = Arc::new(Mutex::new(Vec::new()));
    h.state()
        .runs
        .set_notify_sinks(vec![Arc::new(Recorder(sent.clone()))]);
    h.state()
        .runs
        .set_question_notify_after(Duration::from_millis(200));
    let c = env.client(&h).await;
    let mut p = env.params("claude-a");
    p.name = Some("asker".into());
    let id = spawn(&c, p).await;
    wait_state(&c, &id, RunState::Running).await;
    let child = env.as_run(&h, &id).await;
    let call = tokio::spawn(async move {
        child
            .call::<_, AskParentResult>(
                METHOD_ASK_PARENT,
                AskParentParams {
                    question: "secret question text".into(),
                    options: None,
                },
            )
            .await
            .unwrap()
    });
    for _ in 0..200 {
        if !sent.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    {
        let sent = sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, "superorchestrator: question_waiting_10m");
        assert!(sent[0].1.starts_with("asker"));
        assert!(
            !sent[0].1.contains("secret question"),
            "no task text in notifications"
        );
    }
    assert!(call.await.unwrap().timed_out);
    cancel(&c, &id).await;
    h.shutdown().await;
}

// ------------------------------------------------------------------------ fork

#[tokio::test]
async fn fork_respects_the_child_cap_and_the_parent() {
    let mut env = Env::new();
    env.config.limits.max_children = 1;
    env.account("claude-a", 4, &ok_script(&[]), None, None);
    env.simple("claude-b", 4, &idle_script());
    let h = env.start().await;
    let c = env.client(&h).await;
    let parent = spawn(&c, env.params("claude-b")).await;
    wait_state(&c, &parent, RunState::Running).await;
    let mine = env.as_run(&h, &parent).await;
    let done = spawn(&mine, env.params("claude-a")).await;
    wait_state(&c, &done, RunState::Succeeded).await;
    let busy = spawn(&mine, env.params("claude-b")).await;
    wait_state(&c, &busy, RunState::Running).await;
    let fork = |run: &str| ForkParams {
        run: run.into(),
        prompt: "again".into(),
    };
    // The parent already has its one active child.
    let e = mine
        .call::<_, SpawnResult>(METHOD_FORK_AGENT, fork(&done))
        .await
        .unwrap_err();
    match e {
        ClientError::Rpc(r) => assert_eq!(r.code, so_core::proto::CAP_EXCEEDED),
        other => panic!("{other:?}"),
    }
    // Only the source's parent can fork it: not the orchestrator above it.
    let e = c
        .call::<_, SpawnResult>(METHOD_FORK_AGENT, fork(&done))
        .await
        .unwrap_err();
    match e {
        ClientError::Rpc(r) => assert_eq!(r.code, so_core::proto::INVALID_PARAMS),
        other => panic!("{other:?}"),
    }
    cancel(&c, &parent).await;
    h.shutdown().await;
}

#[tokio::test]
async fn a_reject_from_the_ui_is_recorded_as_such() {
    let env = Env::new();
    let resume = script(
        &[expect("redo")]
            .into_iter()
            .chain(turn())
            .collect::<Vec<_>>(),
    );
    env.account("claude-a", 2, &ok_script(&[]), Some(&resume), None);
    let h = env.start().await;
    let c = env.client(&h).await;
    let mut p = env.params("claude-a");
    p.review = true;
    let id = spawn(&c, p).await;
    wait_state(&c, &id, RunState::PendingReview).await;
    let session = env.store().get_run(&id).unwrap().unwrap().session_id;
    let ctx = so_supervisor::ConnCtx::acting_in(&session);
    h.state()
        .runs
        .reject_run(
            &ctx,
            RejectParams {
                run: id.clone(),
                feedback: "redo".into(),
            },
        )
        .await
        .unwrap();
    let v = wait_state(&c, &id, RunState::PendingReview).await;
    let rejected = v
        .recent_events
        .iter()
        .find(|e| e.event_type == EventType::ReviewRejected)
        .unwrap();
    assert_eq!(rejected.data["actor"], "ui");
    h.shutdown().await;
}

#[tokio::test]
async fn a_failed_store_of_the_result_still_announces_the_gate() {
    let env = Env::new();
    env.simple("claude-a", 2, &ok_script(&[]));
    let h = env.start().await;
    h.state().runs.set_fault_gate_result(true);
    let c = env.client(&h).await;
    let mut p = env.params("claude-a");
    p.review = true;
    let id = spawn(&c, p).await;
    let v = wait_state(&c, &id, RunState::PendingReview).await;
    let msgs = read_until_result(&c).await;
    assert!(msgs.messages.iter().any(|m| m.kind == MessageKind::Result));
    let v2 = view(&c, &id).await;
    assert!(
        v2.recent_events
            .iter()
            .any(|e| e.event_type == EventType::StateChanged && e.data["to"] == "pending_review"),
        "{:?}",
        v.recent_events.len()
    );
    // The run did not get stuck: it can be accepted.
    let _: AckResult = c
        .call(METHOD_ACCEPT_RUN, RunRef { run: id.clone() })
        .await
        .unwrap();
    wait_state(&c, &id, RunState::Succeeded).await;
    h.shutdown().await;
}

#[tokio::test]
async fn a_cancel_during_the_gate_teardown_leaves_a_cancelled_attempt() {
    let env = Env::new();
    // The process lingers after the turn, so the teardown at the gate takes a while.
    let first = script(
        &[expect(PROMPT)]
            .into_iter()
            .chain(turn())
            .chain([json!({"sleep_ms": 1500})])
            .collect::<Vec<_>>(),
    );
    env.simple("claude-a", 2, &first);
    let h = env.start().await;
    let c = env.client(&h).await;
    let mut p = env.params("claude-a");
    p.review = true;
    let id = spawn(&c, p).await;
    // In the teardown: still running, but its process was taken out of service.
    for _ in 0..400 {
        if view(&c, &id).await.state == RunState::Running && h.state().runs.live_count() == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    cancel(&c, &id).await;
    wait_state(&c, &id, RunState::Cancelled).await;
    // Give the gate time to finish its (lost) state change.
    for _ in 0..200 {
        let a = env.store().list_attempts(&id, 10).unwrap();
        if a.len() == 1 && a[0].ended_at.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let attempts = env.store().list_attempts(&id, 10).unwrap();
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].outcome.as_deref(), Some("cancelled"));
    assert_eq!(view(&c, &id).await.state, RunState::Cancelled);
    h.shutdown().await;
}

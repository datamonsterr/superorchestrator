#![allow(clippy::unwrap_used)]
//! Messages, nesting, the review gate, stalls, process death, notifications and fork,
//! against the scripted fake harness and an in-process supervisor.

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

fn git(dir: &Path, args: &[&str]) {
    let st = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .status()
        .unwrap();
    assert!(st.success());
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
    tokio::time::sleep(Duration::from_millis(200)).await;
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
    let w = tokio::time::timeout(Duration::from_secs(10), waiter)
        .await
        .unwrap()
        .unwrap();
    assert!(w.finished.is_empty() && !w.timed_out);
    assert_eq!(w.updates.len(), 1);
    assert_eq!(w.updates[0].event_type, EventType::Progress);
    assert_eq!(w.updates[0].data["step"], "tests");
    cancel(&c, &id).await;
    h.shutdown().await;
}

// ------------------------------------------------------------------ ask_parent

#[tokio::test]
async fn ask_parent_blocks_until_answered() {
    let mut env = Env::new();
    env.config.runs.stall_ms = 200;
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
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert_eq!(view(&c, &id).await.state, RunState::Waiting);
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
    let v = wait_state(&c, &id, RunState::Running).await;
    assert_eq!(count(&v, EventType::QuestionAsked), 1);
    assert_eq!(count(&v, EventType::QuestionAnswered), 1);
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
    let msgs: ReadMessagesResult = c.call(METHOD_READ_MESSAGES, json!({})).await.unwrap();
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
    let msgs: ReadMessagesResult = c.call(METHOD_READ_MESSAGES, json!({})).await.unwrap();
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
    let msgs: ReadMessagesResult = c.call(METHOD_READ_MESSAGES, json!({})).await.unwrap();
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
    tokio::time::sleep(Duration::from_millis(50)).await;
    let v = view(&c, &id).await;
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

#![allow(clippy::unwrap_used)]
//! `monitor` snapshots from a seeded store and a supervisor that may not be running.
//!
//! Everything here reads the store the way the mod does: no supervisor is started unless a
//! test asks for one, and the accounts are plain files in a temporary config dir.

mod common;

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use common::Env;
use serde_json::Value;
use so_core::{
    Event, EventType, Harness, Headroom, HeadroomSource, HeadroomStatus, Isolation, RunState,
    SessionKind, new_id,
};
use so_store::{FlagRow, RunRow, SessionRow};

// ── fixtures ─────────────────────────────────────────────────────────────────

/// An isolated environment whose config dir exists, so `Env::store` can create `state.db`.
fn env() -> Env {
    let e = Env::new();
    std::fs::create_dir_all(&e.cfg).unwrap();
    e
}

fn ts(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn at(base: DateTime<Utc>, minutes: i64) -> DateTime<Utc> {
    base + Duration::minutes(minutes)
}

fn session(id: &str, client: &str, started: DateTime<Utc>) -> SessionRow {
    SessionRow {
        id: id.into(),
        kind: SessionKind::Orchestrator,
        client: client.into(),
        cwd: None,
        account: None,
        pid: None,
        started_at: started,
        ended_at: None,
        status: "active".into(),
    }
}

fn save_session(e: &Env, s: &SessionRow) {
    e.store().insert_session(s).unwrap();
}

struct RunSpec<'a> {
    id: &'a str,
    name: &'a str,
    parent: Option<&'a str>,
    depth: i64,
    state: RunState,
    account: Option<&'a str>,
    created: DateTime<Utc>,
    started: Option<DateTime<Utc>>,
    finished: Option<DateTime<Utc>>,
}

impl<'a> RunSpec<'a> {
    fn new(id: &'a str, state: RunState, created: DateTime<Utc>) -> Self {
        Self {
            id,
            name: id,
            parent: None,
            depth: 0,
            state,
            account: None,
            created,
            started: None,
            finished: None,
        }
    }

    fn child(mut self, parent: &'a str, depth: i64) -> Self {
        self.parent = Some(parent);
        self.depth = depth;
        self
    }

    fn account(mut self, account: &'a str) -> Self {
        self.account = Some(account);
        self
    }

    fn started(mut self, t: DateTime<Utc>) -> Self {
        self.started = Some(t);
        self
    }

    fn finished(mut self, t: DateTime<Utc>) -> Self {
        self.finished = Some(t);
        self
    }
}

fn save_run(e: &Env, session: &str, spec: RunSpec<'_>) -> RunRow {
    let row = RunRow {
        id: spec.id.into(),
        session_id: session.into(),
        parent_run_id: spec.parent.map(str::to_string),
        depth: spec.depth,
        name: Some(spec.name.into()),
        agent: None,
        choice_id: None,
        harness: Harness::Codex,
        account: spec.account.map(str::to_string),
        model: None,
        effort: None,
        cwd: None,
        worktree_path: None,
        branch: None,
        native_session_id: None,
        state: spec.state,
        review: false,
        output_schema: None,
        result_json: None,
        error_kind: None,
        prompt: None,
        created_at: spec.created,
        started_at: spec.started,
        finished_at: spec.finished,
        isolation: Isolation::None,
        max_turns: None,
        worktree_base: None,
        worktree_repo: None,
        timeout_ms: None,
    };
    e.store().insert_run(&row).unwrap();
    row
}

fn append(
    e: &Env,
    session: &str,
    run: &str,
    when: DateTime<Utc>,
    event_type: EventType,
    data: Value,
) {
    e.store()
        .append_event(&Event {
            id: new_id(),
            session_id: session.into(),
            run_id: Some(run.into()),
            ts: when,
            event_type,
            data,
        })
        .unwrap();
}

fn changed(e: &Env, session: &str, run: &str, when: DateTime<Utc>, from: &str, to: &str) {
    append(
        e,
        session,
        run,
        when,
        EventType::StateChanged,
        serde_json::json!({"from": from, "to": to}),
    );
}

fn write_account(e: &Env, name: &str, harness: &str, home: &str) {
    let dir = e.cfg.join("accounts");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(format!("{name}.toml")),
        format!("name = \"{name}\"\nharness = \"{harness}\"\nhome = \"{home}\"\nbinary = \"{harness}\"\n"),
    )
    .unwrap();
}

// ── snapshot helpers ─────────────────────────────────────────────────────────

fn monitor(e: &Env, args: &[&str]) -> Value {
    let mut cmd = e.cmd();
    cmd.arg("monitor").arg("--json").args(args);
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

fn keys(v: &Value) -> Vec<String> {
    let mut k: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
    k.sort();
    k
}

fn find_run<'a>(v: &'a Value, id: &str) -> &'a Value {
    v["runs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == id)
        .unwrap_or_else(|| panic!("run {id} is not in the snapshot: {v}"))
}

fn run_ids(v: &Value) -> Vec<String> {
    v["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap().to_string())
        .collect()
}

// ── the snapshot shape ───────────────────────────────────────────────────────

#[test]
fn a_bare_snapshot_has_every_key_and_null_where_nothing_is_known() {
    let e = env();
    let v = monitor(&e, &[]);

    assert_eq!(
        keys(&v),
        ["accounts", "now", "runs", "session", "supervisor", "window"]
    );
    assert!(
        v["now"].is_string() && v["window"]["from"].is_string() && v["window"]["to"].is_string()
    );
    assert_eq!(keys(&v["window"]), ["from", "to"]);
    assert!(v["accounts"].as_array().unwrap().is_empty());
    assert!(v["session"].is_null());
    assert!(v["runs"].as_array().unwrap().is_empty());

    // No supervisor: `running` is false and everything else is null, never missing.
    assert_eq!(
        keys(&v["supervisor"]),
        [
            "active_runs",
            "pid",
            "running",
            "sessions",
            "stale",
            "uptime_s",
            "warning"
        ]
    );
    assert_eq!(v["supervisor"]["running"], false);
    for key in [
        "pid",
        "uptime_s",
        "stale",
        "warning",
        "active_runs",
        "sessions",
    ] {
        assert!(v["supervisor"][key].is_null(), "{key} is not null: {v}");
    }
}

// ── session matching ─────────────────────────────────────────────────────────

#[test]
fn session_matches_by_exact_id_unique_prefix_or_nothing() {
    let e = env();
    let base = Utc::now();
    save_session(&e, &session("sess-aaaaaaaa", "claude-code", at(base, -30)));
    save_session(&e, &session("sess-bbbbbbbb", "claude-code", at(base, -20)));
    save_session(&e, &session("dup-1", "claude-code", at(base, -10)));
    save_session(&e, &session("dup-2", "claude-code", at(base, -9)));

    let v = monitor(&e, &["--session", "sess-aaaaaaaa"]);
    assert_eq!(v["session"]["id"], "sess-aaaaaaaa");
    assert_eq!(v["session"]["matched_by"], "id");
    assert_eq!(
        keys(&v["session"]),
        ["cwd", "id", "matched_by", "started_at"]
    );
    assert!(v["session"]["cwd"].is_null());

    let v = monitor(&e, &["--session", "sess-bbb"]);
    assert_eq!(v["session"]["id"], "sess-bbbbbbbb");

    // An ambiguous prefix is no match, and neither is a fresh id.
    let v = monitor(&e, &["--session", "dup"]);
    assert!(v["session"].is_null() && v["runs"].as_array().unwrap().is_empty());
    let v = monitor(&e, &["--session", "no-such-session"]);
    assert!(v["session"].is_null());
}

#[test]
fn session_matches_by_cwd_and_skips_web_sessions() {
    let e = env();
    let base = Utc::now();
    let mut mine = session("sess-mine", "claude-code", at(base, -30));
    mine.cwd = Some("/work/one".into());
    save_session(&e, &mine);
    // The web UI's workflow session has the same cwd but belongs to no conversation.
    let mut web = session("sess-web", "web", at(base, -5));
    web.cwd = Some("/work/one".into());
    save_session(&e, &web);

    let v = monitor(&e, &["--cwd", "/work/one"]);
    assert_eq!(v["session"]["id"], "sess-mine");
    assert_eq!(v["session"]["matched_by"], "cwd");
    assert_eq!(v["session"]["cwd"], "/work/one");

    assert!(monitor(&e, &["--cwd", "/work/two"])["session"].is_null());
}

#[test]
fn session_matches_the_bridge_child_of_the_client_pid() {
    let e = env();
    let base = Utc::now();
    // A stand-in for the MCP bridge: its parent is this test process, the client.
    let mut child = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let mut bridge = session("sess-bridge", "claude-code", at(base, -5));
    bridge.pid = Some(i64::from(child.id()));
    save_session(&e, &bridge);
    // An ended session with a pid must not match.
    let mut old = session("sess-old", "claude-code", at(base, -60));
    old.pid = Some(i64::from(child.id()));
    old.ended_at = Some(at(base, -30));
    save_session(&e, &old);

    let client = i64::from(std::process::id());
    let v = monitor(&e, &["--client-pid", &client.to_string()]);
    assert_eq!(v["session"]["id"], "sess-bridge");
    assert_eq!(v["session"]["matched_by"], "pid");

    let missing = monitor(&e, &["--client-pid", &i32::MAX.to_string()]);
    assert!(missing["session"].is_null());

    child.kill().unwrap();
    child.wait().unwrap();
}

// ── runs, windows and the tree ───────────────────────────────────────────────

#[test]
fn runs_come_parent_first_and_only_when_inside_the_window() {
    let e = env();
    let base = Utc::now();
    let s = session("sess-1", "claude-code", at(base, -180));
    save_session(&e, &s);

    save_run(
        &e,
        &s.id,
        RunSpec::new("b", RunState::Running, at(base, -120)).started(at(base, -119)),
    );
    save_run(
        &e,
        &s.id,
        RunSpec::new("a", RunState::Running, at(base, -110)).started(at(base, -109)),
    );
    save_run(
        &e,
        &s.id,
        RunSpec::new("a1", RunState::Running, at(base, -100))
            .child("a", 1)
            .started(at(base, -99)),
    );
    save_run(
        &e,
        &s.id,
        RunSpec::new("a1x", RunState::PendingReview, at(base, -90))
            .child("a1", 2)
            .started(at(base, -89)),
    );
    // Finished ten minutes ago: outside a five minute window, inside an hour.
    save_run(
        &e,
        &s.id,
        RunSpec::new("old", RunState::Succeeded, at(base, -15))
            .started(at(base, -14))
            .finished(at(base, -10)),
    );
    // Finished three minutes ago: inside even the shortest window.
    save_run(
        &e,
        &s.id,
        RunSpec::new("recent", RunState::Failed, at(base, -8))
            .started(at(base, -7))
            .finished(at(base, -3)),
    );

    let v = monitor(&e, &["--session", &s.id, "--window", "5"]);
    assert_eq!(run_ids(&v), ["b", "a", "a1", "a1x", "recent"]);
    assert_eq!(find_run(&v, "a1")["parent_run"], "a");
    assert_eq!(find_run(&v, "a1")["depth"], 1);
    assert_eq!(find_run(&v, "a1x")["parent_run"], "a1");
    assert_eq!(find_run(&v, "a1x")["depth"], 2);
    assert_eq!(find_run(&v, "a1x")["state"], "pending_review");
    assert_eq!(find_run(&v, "a1x")["name"], "a1x");
    assert_eq!(find_run(&v, "a1x")["harness"], "codex");

    let v = monitor(&e, &["--session", &s.id, "--window", "60"]);
    assert_eq!(run_ids(&v), ["b", "a", "a1", "a1x", "old", "recent"]);
}

#[test]
fn an_orphan_run_is_a_root_of_its_own() {
    let e = env();
    let base = Utc::now();
    let s = session("sess-1", "claude-code", at(base, -10));
    save_session(&e, &s);
    save_run(
        &e,
        &s.id,
        RunSpec::new("kid", RunState::Running, at(base, -5))
            .child("gone", 1)
            .started(at(base, -4)),
    );
    let v = monitor(&e, &["--session", &s.id]);
    assert_eq!(run_ids(&v), ["kid"]);
}

// ── timeline ─────────────────────────────────────────────────────────────────

#[test]
fn segments_clip_to_the_window_keep_the_last_one_open_and_skip_the_terminal_state() {
    let e = env();
    let base = Utc::now();
    let s = session("sess-1", "claude-code", at(base, -180));
    save_session(&e, &s);

    // An active run that started two hours ago: one segment, clipped to the window start.
    save_run(
        &e,
        &s.id,
        RunSpec::new("live", RunState::Running, at(base, -120)).started(at(base, -119)),
    );
    append(
        &e,
        &s.id,
        "live",
        at(base, -120),
        EventType::RunQueued,
        serde_json::json!({}),
    );
    changed(&e, &s.id, "live", at(base, -119), "queued", "running");

    // A finished run: the terminal `succeeded` state never becomes a segment, and the last
    // segment ends at the run's `finished_at`.
    save_run(
        &e,
        &s.id,
        RunSpec::new("done", RunState::Succeeded, at(base, -10))
            .started(at(base, -9))
            .finished(at(base, -3)),
    );
    append(
        &e,
        &s.id,
        "done",
        at(base, -10),
        EventType::RunQueued,
        serde_json::json!({}),
    );
    changed(&e, &s.id, "done", at(base, -9), "queued", "running");
    changed(&e, &s.id, "done", at(base, -5), "running", "pending_review");
    changed(
        &e,
        &s.id,
        "done",
        at(base, -3),
        "pending_review",
        "succeeded",
    );
    append(
        &e,
        &s.id,
        "done",
        at(base, -3) + Duration::milliseconds(20),
        EventType::RunFinished,
        serde_json::json!({"status": "succeeded"}),
    );

    let v = monitor(&e, &["--session", &s.id, "--window", "60"]);
    assert_eq!(
        keys(find_run(&v, "live")),
        [
            "account",
            "attention",
            "created_at",
            "depth",
            "finished_at",
            "harness",
            "id",
            "marks",
            "model",
            "name",
            "parent_run",
            "question",
            "segments",
            "started_at",
            "state"
        ]
    );

    let live = &find_run(&v, "live")["segments"];
    assert_eq!(keys(&live[0]), ["from", "state", "to"]);
    assert_eq!(live.as_array().unwrap().len(), 1);
    assert_eq!(live[0]["state"], "running");
    assert_eq!(live[0]["from"], v["window"]["from"]);
    assert!(live[0]["to"].is_null());

    let done = find_run(&v, "done");
    assert_eq!(done["state"], "succeeded");
    assert_eq!(done["finished_at"], ts(at(base, -3)));
    let segs = done["segments"].as_array().unwrap();
    let states: Vec<&str> = segs.iter().map(|s| s["state"].as_str().unwrap()).collect();
    assert_eq!(states, ["queued", "running", "pending_review"]);
    assert_eq!(segs.last().unwrap()["to"], done["finished_at"]);
    assert_eq!(done["started_at"], ts(at(base, -9)));
}

#[test]
fn a_question_marks_the_run_and_drives_attention() {
    let e = env();
    let base = Utc::now();
    let s = session("sess-1", "claude-code", at(base, -120));
    save_session(&e, &s);

    // A root run waiting on a question asked outside the window: still open, still `claude`.
    save_run(
        &e,
        &s.id,
        RunSpec::new("root", RunState::Waiting, at(base, -20)).started(at(base, -19)),
    );
    append(
        &e,
        &s.id,
        "root",
        at(base, -6),
        EventType::QuestionAsked,
        serde_json::json!({"question": "which one?"}),
    );
    append(
        &e,
        &s.id,
        "root",
        at(base, -2),
        EventType::CheckResult,
        serde_json::json!({"name": "tests", "ok": false}),
    );
    append(
        &e,
        &s.id,
        "root",
        at(base, -1),
        EventType::NudgeSent,
        serde_json::json!({}),
    );

    // A child waiting on a question waits for its parent, not for Claude.
    save_run(
        &e,
        &s.id,
        RunSpec::new("kid", RunState::Waiting, at(base, -4))
            .child("root", 1)
            .started(at(base, -3)),
    );
    append(
        &e,
        &s.id,
        "kid",
        at(base, -1),
        EventType::QuestionAsked,
        serde_json::json!({"question": "child?"}),
    );

    save_run(
        &e,
        &s.id,
        RunSpec::new("review", RunState::PendingReview, at(base, -4)).started(at(base, -3)),
    );
    save_run(
        &e,
        &s.id,
        RunSpec::new("failed", RunState::Failed, at(base, -4))
            .started(at(base, -3))
            .finished(at(base, -1)),
    );
    save_run(
        &e,
        &s.id,
        RunSpec::new("stuck", RunState::Stalled, at(base, -4)).started(at(base, -3)),
    );
    save_run(
        &e,
        &s.id,
        RunSpec::new("escalated", RunState::Escalated, at(base, -4)).started(at(base, -3)),
    );

    let v = monitor(&e, &["--session", &s.id, "--window", "5"]);
    let root = find_run(&v, "root");
    assert_eq!(root["attention"], "claude");
    assert_eq!(root["question"], "which one?");
    // Only the marks inside the window, in time order; the answered-less question is older.
    let marks = root["marks"].as_array().unwrap();
    let kinds: Vec<&str> = marks.iter().map(|m| m["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["check_failed", "nudge"]);
    assert_eq!(keys(&marks[0]), ["kind", "ts"]);
    assert_eq!(marks[0]["ts"], ts(at(base, -2)));

    assert_eq!(find_run(&v, "kid")["attention"], Value::Null);
    assert_eq!(find_run(&v, "kid")["question"], "child?");
    assert_eq!(find_run(&v, "review")["attention"], "claude");
    assert_eq!(find_run(&v, "failed")["attention"], "failed");
    assert_eq!(find_run(&v, "stuck")["attention"], "stuck");
    assert_eq!(find_run(&v, "escalated")["attention"], "stuck");

    let text = {
        let out = e
            .cmd()
            .args(["monitor", "--session", &s.id, "--window", "5"])
            .output()
            .unwrap();
        assert!(out.status.success());
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    assert!(text.contains("supervisor: not running"), "{text}");
    assert!(
        text.contains("attention: claude=2 stuck=2 failed=1"),
        "{text}"
    );
    assert!(text.contains("root waiting"), "{text}");
}

// ── accounts and headroom ────────────────────────────────────────────────────

#[test]
fn accounts_carry_rounded_headroom_and_an_unknown_default_pool() {
    let e = env();
    let base = Utc::now();
    write_account(&e, "codex-a", "codex", "/homes/codex-a");
    write_account(&e, "claude-b", "claude", "/homes/claude-b");

    let store = e.store();
    store
        .upsert_headroom(&Headroom {
            account: "codex-a".into(),
            pool: "default".into(),
            five_hour_remaining_pct: Some(64.4),
            five_hour_resets_at: Some(at(base, 120)),
            weekly_remaining_pct: Some(12.5),
            weekly_resets_at: None,
            status: HeadroomStatus::Ok,
            source: HeadroomSource::Probe,
            observed_at: at(base, -1),
        })
        .unwrap();
    // The epoch sentinel is "never observed", which the wire calls null.
    store
        .upsert_headroom(&Headroom {
            account: "codex-a".into(),
            pool: "weekly".into(),
            five_hour_remaining_pct: None,
            five_hour_resets_at: None,
            weekly_remaining_pct: Some(3.0),
            weekly_resets_at: None,
            status: HeadroomStatus::Low,
            source: HeadroomSource::Ledger,
            observed_at: DateTime::<Utc>::UNIX_EPOCH,
        })
        .unwrap();
    store
        .set_flag(&FlagRow {
            account: "codex-a".into(),
            pool: "default".into(),
            until: at(base, 30),
            reason: "cooldown".into(),
        })
        .unwrap();

    let v = monitor(&e, &[]);
    let accounts = v["accounts"].as_array().unwrap();
    assert_eq!(accounts.len(), 2);
    let mine = accounts.iter().find(|a| a["name"] == "codex-a").unwrap();
    assert_eq!(
        keys(mine),
        [
            "active_runs",
            "cap",
            "enabled",
            "harness",
            "name",
            "pools",
            "signed_in"
        ]
    );
    assert_eq!(mine["harness"], "codex");
    assert_eq!(mine["enabled"], true);
    assert_eq!(mine["signed_in"], "no");
    assert_eq!(mine["active_runs"], 0);
    assert_eq!(mine["cap"], 2);

    let pools = mine["pools"].as_array().unwrap();
    assert_eq!(pools.len(), 2);
    assert_eq!(
        keys(&pools[0]),
        [
            "cooldown_until",
            "five_hour_pct",
            "five_hour_resets_at",
            "observed_at",
            "pool",
            "source",
            "status",
            "weekly_pct",
            "weekly_resets_at"
        ]
    );
    assert_eq!(pools[0]["pool"], "default");
    assert_eq!(pools[0]["status"], "ok");
    assert_eq!(pools[0]["five_hour_pct"], 64);
    assert_eq!(pools[0]["weekly_pct"], 13);
    assert_eq!(pools[0]["five_hour_resets_at"], ts(at(base, 120)));
    assert!(pools[0]["weekly_resets_at"].is_null());
    assert_eq!(pools[0]["cooldown_until"], ts(at(base, 30)));
    assert_eq!(pools[0]["source"], "probe");
    assert_eq!(pools[0]["observed_at"], ts(at(base, -1)));

    assert_eq!(pools[1]["pool"], "weekly");
    assert_eq!(pools[1]["status"], "low");
    assert_eq!(pools[1]["weekly_pct"], 3);
    assert_eq!(pools[1]["source"], "ledger");
    assert!(pools[1]["observed_at"].is_null());

    // No headroom rows: one `default` pool that knows nothing.
    let other = accounts.iter().find(|a| a["name"] == "claude-b").unwrap();
    let own = other["pools"].as_array().unwrap();
    assert_eq!(own.len(), 1);
    assert_eq!(own[0]["pool"], "default");
    assert_eq!(own[0]["status"], "unknown");
    assert!(own[0]["five_hour_pct"].is_null());
    assert!(own[0]["weekly_pct"].is_null());
    assert!(own[0]["source"].is_null());
    assert!(own[0]["observed_at"].is_null());
    assert!(own[0]["cooldown_until"].is_null());
}

#[test]
fn active_runs_are_counted_per_account() {
    let e = env();
    let base = Utc::now();
    write_account(&e, "codex-a", "codex", "/homes/codex-a");
    write_account(&e, "claude-b", "claude", "/homes/claude-b");
    let s = session("sess-1", "claude-code", at(base, -10));
    save_session(&e, &s);
    save_run(
        &e,
        &s.id,
        RunSpec::new("one", RunState::Running, at(base, -5))
            .account("codex-a")
            .started(at(base, -4)),
    );
    save_run(
        &e,
        &s.id,
        RunSpec::new("done", RunState::Succeeded, at(base, -5))
            .account("codex-a")
            .started(at(base, -4))
            .finished(at(base, -1)),
    );
    let v = monitor(&e, &["--session", &s.id]);
    let accounts = v["accounts"].as_array().unwrap();
    let codex = accounts.iter().find(|a| a["name"] == "codex-a").unwrap();
    let claude = accounts.iter().find(|a| a["name"] == "claude-b").unwrap();
    assert_eq!(codex["active_runs"], 1);
    assert_eq!(claude["active_runs"], 0);
    assert_eq!(find_run(&v, "one")["account"], "codex-a");
}

// ── usage errors and a live supervisor ───────────────────────────────────────

#[test]
fn a_window_outside_five_to_1440_minutes_is_a_usage_error() {
    let e = env();
    for window in ["2", "0", "1441"] {
        e.cmd()
            .args(["monitor", "--window", window])
            .assert()
            .code(2);
    }
    e.cmd()
        .args(["monitor", "--window", "not-a-number"])
        .assert()
        .code(2);
    e.cmd()
        .args(["monitor", "--window", "5", "--json"])
        .assert()
        .success();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_running_supervisor_fills_the_supervisor_block() {
    let e = env();
    let sup = common::start_supervisor(&e).await;
    let (out, ok) = common::cli(&e, &["monitor", "--json"]).await;
    assert!(ok, "{out}");
    let v: Value = serde_json::from_str(&out).unwrap();
    let s = &v["supervisor"];
    assert_eq!(s["running"], true);
    assert!(s["pid"].is_u64() && s["pid"].as_u64().unwrap() > 0);
    assert!(s["uptime_s"].is_u64());
    assert_eq!(s["stale"], false);
    // The supervisor lives in the test executable, so its build id differs from the CLI's.
    assert!(s["warning"].is_string(), "{s}");
    assert_eq!(s["active_runs"], 0);
    sup.shutdown().await;
}

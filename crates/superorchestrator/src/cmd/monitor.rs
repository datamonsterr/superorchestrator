//! `monitor [--session ID] [--client-pid PID] [--cwd DIR] [--window MINUTES]`
//!
//! One compact snapshot for a mod that polls every two seconds: supervisor health, the
//! headroom of every account and pool, and the state timeline of the matched session's runs.
//! It only reads the store and asks a running supervisor for its status; it never starts the
//! supervisor, and it exits 0 when the supervisor is down or no session matches.

use std::collections::{HashMap, HashSet};
use std::io::{self, Write};
use std::process::ExitCode;

use anyhow::Result;
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use serde_json::{Value, json};
use so_core::procinfo::parent_pid;
use so_core::proto::StatusResult;
use so_core::{Event, EventType, Headroom, RunState, SessionKind};
use so_store::{FlagRow, RunRow, SessionFilter, SessionRow, Store};

use crate::cli::MonitorArgs;
use crate::cmd::status::{daemon_warning, supervisor_line, supervisor_status};
use crate::ctx::Ctx;
use crate::views::{AccountView, collect_views_with, open_existing_store, sign_in_text};

/// Most runs one snapshot carries.
const MAX_RUNS: usize = 50;
/// Rows fetched per list query. A monitor reads; a larger store than this is not a
/// two-second snapshot's job.
const LIST_LIMIT: usize = 100_000;
/// Events read for one run, one query per run, never one per segment.
const EVENT_LIMIT: usize = 100_000;

// ── attention ────────────────────────────────────────────────────────────────

/// Which party has to act on a run: a pure function of its state, whether a question is
/// open and whether it is a root run.
fn attention(
    state: RunState,
    open_question: bool,
    parent_run: Option<&str>,
) -> Option<&'static str> {
    match state {
        RunState::PendingReview => Some("claude"),
        RunState::Waiting if open_question && parent_run.is_none() => Some("claude"),
        RunState::Stalled | RunState::Escalated => Some("stuck"),
        RunState::Failed => Some("failed"),
        _ => None,
    }
}

// ── segments and marks ───────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
struct Segment {
    state: String,
    from: DateTime<Utc>,
    to: Option<DateTime<Utc>>,
}

/// One state over `[from, to)`, built from the run's lifecycle events.
///
/// `run_queued` opens `queued`; `state_changed{to}` closes the open segment and opens `to`;
/// `run_started` turns a `queued` or `starting` segment into `running` (the supervisor emits
/// that transition first) and opens `running` when nothing is open. Terminal states are not
/// segments: the run's `state` field carries them, and a zero-length or negative segment is
/// never emitted. A finished run's last segment ends at `finished_at`; an active run's stays
/// open. Everything is then clipped to `window_from`.
fn segments(events: &[Event], run: &RunRow, window_from: DateTime<Utc>) -> Vec<Segment> {
    let last_finish = events
        .iter()
        .rposition(|e| e.event_type == EventType::RunFinished);
    let mut segs: Vec<Segment> = Vec::new();
    let mut open: Option<(String, DateTime<Utc>)> = None;
    for (i, e) in events.iter().enumerate() {
        match e.event_type {
            EventType::RunQueued => {
                if open.is_none() {
                    open = Some(("queued".to_string(), e.ts));
                }
            }
            EventType::RunStarted => match open.take() {
                None => open = Some(("running".to_string(), e.ts)),
                Some((state, from)) if state == "queued" || state == "starting" => {
                    segs.push(Segment {
                        state,
                        from,
                        to: Some(e.ts),
                    });
                    open = Some(("running".to_string(), e.ts));
                }
                Some(other) => open = Some(other),
            },
            EventType::StateChanged => {
                let Some(to) = e
                    .data
                    .get("to")
                    .and_then(Value::as_str)
                    .and_then(|s| s.parse::<RunState>().ok())
                else {
                    continue;
                };
                // The run's own state is the terminal word; a terminal segment would be
                // zero-length (the finish event follows within microseconds).
                if to.is_terminal() {
                    continue;
                }
                match open.take() {
                    Some((state, from)) if state == to.as_str() => open = Some((state, from)),
                    Some((state, from)) => {
                        segs.push(Segment {
                            state,
                            from,
                            to: Some(e.ts),
                        });
                        open = Some((to.as_str().to_string(), e.ts));
                    }
                    None => open = Some((to.as_str().to_string(), e.ts)),
                }
            }
            EventType::RunFinished => {
                let end = if last_finish == Some(i) {
                    run.finished_at.unwrap_or(e.ts)
                } else {
                    // An earlier life of a resumed run ends at its own finish event.
                    e.ts
                };
                if let Some((state, from)) = open.take() {
                    segs.push(Segment {
                        state,
                        from,
                        to: Some(end),
                    });
                }
            }
            _ => {}
        }
    }
    if let Some((state, from)) = open {
        let to = if run.state.is_active() {
            None
        } else {
            run.finished_at
        };
        segs.push(Segment { state, from, to });
    }
    clip(segs, window_from)
}

/// Drops what the window does not show and clips a segment that began before it.
fn clip(mut segs: Vec<Segment>, window_from: DateTime<Utc>) -> Vec<Segment> {
    for s in &mut segs {
        if s.from < window_from {
            s.from = window_from;
        }
    }
    segs.retain(|s| s.to.is_none_or(|to| to > s.from));
    segs
}

/// The text of the latest `question_asked` with no `question_answered` after it.
fn open_question(events: &[Event]) -> Option<String> {
    let mut open = None;
    for e in events {
        match e.event_type {
            EventType::QuestionAsked => {
                open = Some(
                    e.data
                        .get("question")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                );
            }
            EventType::QuestionAnswered => open = None,
            _ => {}
        }
    }
    open
}

/// An event that marks the timeline, as the wire name, or `None`.
fn mark_kind(e: &Event) -> Option<&'static str> {
    match e.event_type {
        EventType::QuestionAsked => Some("question"),
        EventType::NudgeSent => Some("nudge"),
        EventType::Escalated => Some("escalated"),
        EventType::Failover => Some("failover"),
        EventType::ReviewRejected => Some("review_rejected"),
        EventType::ReviewAccepted => Some("review_accepted"),
        EventType::CheckResult if e.data.get("ok").and_then(Value::as_bool) == Some(false) => {
            Some("check_failed")
        }
        _ => None,
    }
}

// ── session matching ─────────────────────────────────────────────────────────

struct MatchedSession {
    row: SessionRow,
    matched_by: &'static str,
}

/// A session an MCP client opened. The web UI's workflow sessions are also `orchestrator`
/// rows (`client = "web"`) but belong to no conversation; inspect connections leave no row.
fn is_mcp_session(s: &SessionRow) -> bool {
    s.kind == SessionKind::Orchestrator && s.client != "web"
}

fn match_session(store: &Store, args: &MonitorArgs) -> Option<MatchedSession> {
    if let Some(id) = args.session.as_deref() {
        return by_id(store, id);
    }
    if let Some(pid) = args.client_pid {
        return by_pid(store, pid);
    }
    args.cwd.as_deref().and_then(|cwd| by_cwd(store, cwd))
}

fn by_id(store: &Store, id: &str) -> Option<MatchedSession> {
    if let Ok(Some(row)) = store.get_session(id) {
        return Some(MatchedSession {
            row,
            matched_by: "id",
        });
    }
    let found: Vec<SessionRow> = store
        .list_sessions(SessionFilter::All, LIST_LIMIT)
        .ok()?
        .into_iter()
        .filter(|s| s.id.starts_with(id))
        .collect();
    match found.as_slice() {
        [row] => Some(MatchedSession {
            row: row.clone(),
            matched_by: "id",
        }),
        _ => None,
    }
}

fn by_pid(store: &Store, pid: i64) -> Option<MatchedSession> {
    let parent = parent_pid(pid);
    store
        .list_sessions(SessionFilter::Active, LIST_LIMIT)
        .ok()?
        .into_iter()
        .filter(is_mcp_session)
        .filter(|s| {
            let Some(bridge) = s.pid else { return false };
            let Some(bridge_parent) = parent_pid(bridge) else {
                return false;
            };
            // The bridge's parent is the client itself, or the client's parent when the
            // mod runs in a child worker of the coding harness.
            bridge_parent == pid || Some(bridge_parent) == parent
        })
        .max_by(|a, b| {
            a.started_at
                .cmp(&b.started_at)
                .then_with(|| a.id.cmp(&b.id))
        })
        .map(|row| MatchedSession {
            row,
            matched_by: "pid",
        })
}

fn by_cwd(store: &Store, cwd: &str) -> Option<MatchedSession> {
    store
        .list_sessions(SessionFilter::Active, LIST_LIMIT)
        .ok()?
        .into_iter()
        .filter(is_mcp_session)
        .filter(|s| s.cwd.as_deref() == Some(cwd))
        .max_by(|a, b| {
            a.started_at
                .cmp(&b.started_at)
                .then_with(|| a.id.cmp(&b.id))
        })
        .map(|row| MatchedSession {
            row,
            matched_by: "cwd",
        })
}

// ── run tree ─────────────────────────────────────────────────────────────────

/// Parent before children, siblings by `created_at`. A child whose parent is not in the set
/// is a root of its own. At most [`MAX_RUNS`] runs; active ones are kept first when cutting.
fn tree_order(runs: Vec<RunRow>) -> Vec<RunRow> {
    let ordered = dfs_order(runs);
    if ordered.len() <= MAX_RUNS {
        return ordered;
    }
    let (active, finished): (Vec<RunRow>, Vec<RunRow>) =
        ordered.into_iter().partition(|r| r.state.is_active());
    let kept: Vec<RunRow> = active.into_iter().chain(finished).take(MAX_RUNS).collect();
    dfs_order(kept)
}

fn dfs_order(mut runs: Vec<RunRow>) -> Vec<RunRow> {
    runs.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    let ids: HashSet<&str> = runs.iter().map(|r| r.id.as_str()).collect();
    let mut children: HashMap<&str, Vec<&RunRow>> = HashMap::new();
    let mut roots: Vec<&RunRow> = Vec::new();
    for r in &runs {
        match r.parent_run_id.as_deref().filter(|p| ids.contains(p)) {
            Some(parent) => children.entry(parent).or_default().push(r),
            None => roots.push(r),
        }
    }
    fn walk<'a>(
        run: &'a RunRow,
        children: &HashMap<&str, Vec<&'a RunRow>>,
        out: &mut Vec<&'a RunRow>,
    ) {
        out.push(run);
        if let Some(kids) = children.get(run.id.as_str()) {
            for child in kids {
                walk(child, children, out);
            }
        }
    }
    let mut out: Vec<&RunRow> = Vec::with_capacity(runs.len());
    for root in roots {
        walk(root, &children, &mut out);
    }
    out.into_iter().cloned().collect()
}

// ── JSON ─────────────────────────────────────────────────────────────────────

fn ts(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn opt_ts(t: Option<DateTime<Utc>>) -> Option<String> {
    t.map(ts)
}

fn segment_json(s: &Segment) -> Value {
    json!({
        "state": s.state,
        "from": ts(s.from),
        "to": opt_ts(s.to),
    })
}

fn run_json(
    run: &RunRow,
    events: &[Event],
    window_from: DateTime<Utc>,
    window_to: DateTime<Utc>,
) -> Value {
    let question = open_question(events);
    let marks: Vec<Value> = events
        .iter()
        .filter(|e| e.ts >= window_from && e.ts <= window_to)
        .filter_map(|e| {
            mark_kind(e).map(|kind| {
                json!({
                    "ts": ts(e.ts),
                    "kind": kind,
                })
            })
        })
        .collect();
    json!({
        "id": run.id,
        "name": run.name,
        "parent_run": run.parent_run_id,
        "depth": run.depth,
        "harness": run.harness.as_str(),
        "account": run.account,
        "model": run.choice_id.as_deref().or(run.model.as_deref()),
        "branch": run.branch,
        "state": run.state.as_str(),
        "attention": attention(run.state, question.is_some(), run.parent_run_id.as_deref()),
        "question": question,
        "created_at": ts(run.created_at),
        "started_at": opt_ts(run.started_at),
        "finished_at": opt_ts(run.finished_at),
        "segments": segments(events, run, window_from).iter().map(segment_json).collect::<Vec<_>>(),
        "marks": marks,
    })
}

/// A remaining percent as an integer 0 to 100, or `null`.
fn pct(p: Option<f64>) -> Value {
    p.map_or(Value::Null, |p| json!(p.round().clamp(0.0, 100.0) as i64))
}

fn pool_json(h: &Headroom, cooldown: Option<DateTime<Utc>>) -> Value {
    json!({
        "pool": h.pool.as_str(),
        "status": h.status.as_str(),
        "five_hour_pct": pct(h.five_hour_remaining_pct),
        "five_hour_resets_at": opt_ts(h.five_hour_resets_at),
        "weekly_pct": pct(h.weekly_remaining_pct),
        "weekly_resets_at": opt_ts(h.weekly_resets_at),
        "cooldown_until": opt_ts(cooldown),
        // The Unix epoch is the internal "never observed" sentinel.
        "source": h.source.as_str(),
        "observed_at": (h.observed_at != DateTime::<Utc>::UNIX_EPOCH).then(|| ts(h.observed_at)),
    })
}

fn unknown_pool(cooldown: Option<DateTime<Utc>>) -> Value {
    json!({
        "pool": "default",
        "status": "unknown",
        "five_hour_pct": Value::Null,
        "five_hour_resets_at": Value::Null,
        "weekly_pct": Value::Null,
        "weekly_resets_at": Value::Null,
        "cooldown_until": opt_ts(cooldown),
        "source": Value::Null,
        "observed_at": Value::Null,
    })
}

/// The latest cooldown flag of one (account, pool), which is how flags are keyed in the
/// store and everywhere else that reads them.
fn cooldown(flags: &[FlagRow], account: &str, pool: &str) -> Option<DateTime<Utc>> {
    flags
        .iter()
        .filter(|f| f.account == account && f.pool == pool)
        .map(|f| f.until)
        .max()
}

fn accounts_json(views: &[AccountView], store: Option<&Store>, now: DateTime<Utc>) -> Vec<Value> {
    let (headroom, flags) = match store {
        Some(s) => (
            s.list_headroom(LIST_LIMIT).unwrap_or_default(),
            s.active_flags(now, LIST_LIMIT).unwrap_or_default(),
        ),
        None => (Vec::new(), Vec::new()),
    };
    views
        .iter()
        .map(|view| {
            let name = view.account.name.as_str();
            let rows: Vec<&Headroom> = headroom.iter().filter(|h| h.account == name).collect();
            let pools: Vec<Value> = if rows.is_empty() {
                vec![unknown_pool(cooldown(&flags, name, "default"))]
            } else {
                rows.iter()
                    .map(|h| pool_json(h, cooldown(&flags, name, &h.pool)))
                    .collect()
            };
            json!({
                "name": name,
                "harness": view.account.harness.as_str(),
                "enabled": view.account.enabled,
                "signed_in": sign_in_text(view.signed_in),
                "active_runs": view.active_runs.unwrap_or(0),
                "cap": view.account.cap,
                "pools": pools,
            })
        })
        .collect()
}

fn supervisor_json(sup: Option<&StatusResult>) -> Value {
    match sup {
        Some(s) => json!({
            "running": true,
            "pid": s.pid,
            "uptime_s": s.uptime_s,
            "stale": s.stale,
            "warning": daemon_warning(s),
            "active_runs": s.active_runs,
            "sessions": s.sessions,
        }),
        None => json!({
            "running": false,
            "pid": Value::Null,
            "uptime_s": Value::Null,
            "stale": Value::Null,
            "warning": Value::Null,
            "active_runs": Value::Null,
            "sessions": Value::Null,
        }),
    }
}

fn session_json(matched: Option<&MatchedSession>) -> Value {
    match matched {
        Some(m) => json!({
            "id": m.row.id,
            "cwd": m.row.cwd,
            "started_at": ts(m.row.started_at),
            "matched_by": m.matched_by,
        }),
        None => Value::Null,
    }
}

// ── text output ──────────────────────────────────────────────────────────────

fn write_text(
    w: &mut dyn Write,
    sup: Option<&StatusResult>,
    runs: &[(RunRow, Vec<Event>)],
    now: DateTime<Utc>,
) -> io::Result<()> {
    match sup {
        Some(s) => {
            writeln!(w, "{}", supervisor_line(s))?;
            if let Some(warning) = daemon_warning(s) {
                writeln!(w, "warning: {warning}")?;
            }
        }
        None => writeln!(w, "supervisor: not running")?,
    }
    let mut claude = 0;
    let mut stuck = 0;
    let mut failed = 0;
    for (run, events) in runs {
        match attention(
            run.state,
            open_question(events).is_some(),
            run.parent_run_id.as_deref(),
        ) {
            Some("claude") => claude += 1,
            Some("stuck") => stuck += 1,
            Some("failed") => failed += 1,
            _ => {}
        }
    }
    writeln!(
        w,
        "attention: claude={claude} stuck={stuck} failed={failed}"
    )?;
    for (run, _) in runs {
        let name = run.name.as_deref().unwrap_or(&run.id);
        let duration = match run.started_at {
            Some(start) => format!(
                "{}s",
                (run.finished_at.unwrap_or(now) - start)
                    .num_seconds()
                    .max(0)
            ),
            None => "-".to_string(),
        };
        writeln!(w, "{name} {} {duration}", run.state.as_str())?;
    }
    Ok(())
}

// ── entry point ──────────────────────────────────────────────────────────────

/// Account views, or none with one warning: a two-second poll must always print a snapshot,
/// so an unreadable account file or a host that cannot be detected degrades the accounts
/// block instead of failing the command.
fn account_views(ctx: &Ctx) -> (Vec<AccountView>, Option<String>) {
    let host = match ctx.host() {
        Ok(host) => host,
        Err(e) => return (Vec::new(), Some(format!("{e:#}"))),
    };
    match collect_views_with(ctx, &host) {
        Ok(views) => (views, None),
        Err(e) => (Vec::new(), Some(format!("{e:#}"))),
    }
}

pub fn run(ctx: &Ctx, args: MonitorArgs) -> Result<ExitCode> {
    let now = Utc::now();
    let window_from = now - Duration::minutes(i64::from(args.window));

    let sup = supervisor_status();
    let store = open_existing_store(&ctx.paths);
    let (views, warning) = account_views(ctx);
    if let Some(warning) = warning {
        eprintln!("warning: {warning}");
    }
    let matched = store.as_ref().and_then(|s| match_session(s, &args));

    let mut runs: Vec<(RunRow, Vec<Event>)> = Vec::new();
    if let (Some(store), Some(matched)) = (store.as_ref(), matched.as_ref()) {
        let candidates: Vec<RunRow> = store
            .list_runs_by_session(&matched.row.id, LIST_LIMIT)
            .unwrap_or_default()
            .into_iter()
            .filter(|r| r.state.is_active() || r.finished_at.is_some_and(|at| at >= window_from))
            .collect();
        for run in tree_order(candidates) {
            let events = store
                .list_events_by_run(&run.id, EVENT_LIMIT)
                .unwrap_or_default();
            runs.push((run, events));
        }
    }

    let value = json!({
        "now": ts(now),
        "window": { "from": ts(window_from), "to": ts(now) },
        "supervisor": supervisor_json(sup.as_ref()),
        "accounts": accounts_json(&views, store.as_ref(), now),
        "session": session_json(matched.as_ref()),
        "runs": runs
            .iter()
            .map(|(run, events)| run_json(run, events, window_from, now))
            .collect::<Vec<_>>(),
    });
    ctx.emit(&value, |w| write_text(w, sup.as_ref(), &runs, now))?;
    Ok(ExitCode::SUCCESS)
}

// ── unit tests for the pure parts ────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use so_core::{Harness, Isolation, new_id};

    fn at(secs: i64) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-10-25T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
            + Duration::seconds(secs)
    }

    fn run_row(state: RunState) -> RunRow {
        RunRow {
            id: "r1".into(),
            session_id: "s1".into(),
            parent_run_id: None,
            depth: 0,
            name: Some("demo".into()),
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
            created_at: at(0),
            started_at: None,
            finished_at: None,
            isolation: Isolation::None,
            max_turns: None,
            worktree_base: None,
            worktree_repo: None,
            timeout_ms: None,
        }
    }

    fn event(run: &str, secs: i64, kind: EventType, data: Value) -> Event {
        Event {
            id: new_id(),
            session_id: "s1".into(),
            run_id: Some(run.into()),
            ts: at(secs),
            event_type: kind,
            data,
        }
    }

    fn changed(run: &str, secs: i64, from: &str, to: &str) -> Event {
        event(
            run,
            secs,
            EventType::StateChanged,
            json!({"from": from, "to": to}),
        )
    }

    #[test]
    fn attention_rules() {
        assert_eq!(
            attention(RunState::PendingReview, false, None),
            Some("claude")
        );
        assert_eq!(
            attention(RunState::PendingReview, false, Some("p")),
            Some("claude")
        );
        assert_eq!(attention(RunState::Waiting, true, None), Some("claude"));
        assert_eq!(attention(RunState::Waiting, true, Some("p")), None);
        assert_eq!(attention(RunState::Waiting, false, None), None);
        for state in [RunState::Stalled, RunState::Escalated] {
            assert_eq!(attention(state, false, None), Some("stuck"), "{state}");
        }
        assert_eq!(attention(RunState::Failed, false, None), Some("failed"));
        for state in [
            RunState::Queued,
            RunState::Starting,
            RunState::Running,
            RunState::Succeeded,
            RunState::Cancelled,
        ] {
            assert_eq!(attention(state, true, None), None, "{state}");
        }
    }

    #[test]
    fn segments_follow_the_lifecycle_events() {
        let events = vec![
            event("r1", 0, EventType::RunQueued, json!({})),
            changed("r1", 60, "queued", "starting"),
            event("r1", 61, EventType::RunStarted, json!({})),
            changed("r1", 120, "starting", "running"),
            changed("r1", 300, "running", "waiting"),
        ];
        let segs = segments(&events, &run_row(RunState::Waiting), at(-600));
        assert_eq!(
            segs,
            vec![
                Segment {
                    state: "queued".into(),
                    from: at(0),
                    to: Some(at(60))
                },
                Segment {
                    state: "starting".into(),
                    from: at(60),
                    to: Some(at(61))
                },
                Segment {
                    state: "running".into(),
                    from: at(61),
                    to: Some(at(300))
                },
                Segment {
                    state: "waiting".into(),
                    from: at(300),
                    to: None
                },
            ]
        );
    }

    #[test]
    fn segments_skip_the_terminal_state_and_end_at_finished_at() {
        let mut run = run_row(RunState::Succeeded);
        run.finished_at = Some(at(420));
        let events = vec![
            event("r1", 0, EventType::RunQueued, json!({})),
            changed("r1", 60, "queued", "running"),
            changed("r1", 300, "running", "pending_review"),
            changed("r1", 420, "pending_review", "succeeded"),
            event(
                "r1",
                421,
                EventType::RunFinished,
                json!({"status": "succeeded"}),
            ),
        ];
        let segs = segments(&events, &run, at(-600));
        assert_eq!(
            segs,
            vec![
                Segment {
                    state: "queued".into(),
                    from: at(0),
                    to: Some(at(60))
                },
                Segment {
                    state: "running".into(),
                    from: at(60),
                    to: Some(at(300))
                },
                Segment {
                    state: "pending_review".into(),
                    from: at(300),
                    to: Some(at(420))
                },
            ]
        );
    }

    #[test]
    fn segments_clip_to_the_window_and_keep_the_last_one_open() {
        let events = vec![
            event("r1", -3600, EventType::RunQueued, json!({})),
            changed("r1", -3540, "queued", "running"),
        ];
        let segs = segments(&events, &run_row(RunState::Running), at(-60));
        assert_eq!(
            segs,
            vec![Segment {
                state: "running".into(),
                from: at(-60),
                to: None
            }]
        );
    }

    #[test]
    fn run_started_alone_opens_running() {
        let events = vec![event("r1", 0, EventType::RunStarted, json!({}))];
        let segs = segments(&events, &run_row(RunState::Running), at(-600));
        assert_eq!(
            segs,
            vec![Segment {
                state: "running".into(),
                from: at(0),
                to: None
            }]
        );
    }

    #[test]
    fn open_question_is_the_latest_unanswered_one() {
        let ev = |secs, kind, text| event("r1", secs, kind, json!({"question": text}));
        assert_eq!(
            open_question(&[ev(0, EventType::QuestionAsked, "first?")]).as_deref(),
            Some("first?")
        );
        assert_eq!(
            open_question(&[
                ev(0, EventType::QuestionAsked, "first?"),
                ev(1, EventType::QuestionAnswered, ""),
            ]),
            None
        );
        assert_eq!(
            open_question(&[
                ev(0, EventType::QuestionAsked, "first?"),
                ev(1, EventType::QuestionAnswered, ""),
                ev(2, EventType::QuestionAsked, "second?"),
            ])
            .as_deref(),
            Some("second?")
        );
    }

    #[test]
    fn marks_map_only_the_interesting_events() {
        let cases = [
            (
                EventType::QuestionAsked,
                json!({"question": "q"}),
                Some("question"),
            ),
            (EventType::NudgeSent, json!({}), Some("nudge")),
            (EventType::Escalated, json!({}), Some("escalated")),
            (EventType::Failover, json!({}), Some("failover")),
            (
                EventType::ReviewRejected,
                json!({}),
                Some("review_rejected"),
            ),
            (
                EventType::ReviewAccepted,
                json!({}),
                Some("review_accepted"),
            ),
            (
                EventType::CheckResult,
                json!({"ok": false}),
                Some("check_failed"),
            ),
            (EventType::CheckResult, json!({"ok": true}), None),
            (EventType::Progress, json!({"step": 1}), None),
        ];
        for (kind, data, expected) in cases {
            assert_eq!(mark_kind(&event("r1", 0, kind, data)), expected, "{kind}");
        }
    }

    #[test]
    fn tree_order_puts_a_parent_before_its_children_at_any_depth() {
        let mut child = run_row(RunState::Running);
        child.id = "a1".into();
        child.parent_run_id = Some("a".into());
        child.depth = 1;
        child.created_at = at(20);
        let mut grandchild = run_row(RunState::Running);
        grandchild.id = "a1x".into();
        grandchild.parent_run_id = Some("a1".into());
        grandchild.depth = 2;
        grandchild.created_at = at(10);
        let mut first = run_row(RunState::Running);
        first.id = "a".into();
        first.created_at = at(30);
        let mut other = run_row(RunState::Running);
        other.id = "b".into();
        other.created_at = at(5);
        let ordered = tree_order(vec![grandchild, child, first, other]);
        let ids: Vec<&str> = ordered.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["b", "a", "a1", "a1x"]);
    }

    #[test]
    fn tree_order_keeps_active_runs_when_cutting_to_fifty() {
        let mut runs: Vec<RunRow> = (0..50)
            .map(|i| {
                let mut r = run_row(RunState::Succeeded);
                r.id = format!("f{i:02}");
                r.created_at = at(i);
                r.finished_at = Some(at(i + 1));
                r
            })
            .collect();
        let mut active = run_row(RunState::Running);
        active.id = "live".into();
        active.created_at = at(100);
        runs.push(active);

        let ordered = tree_order(runs);
        assert_eq!(ordered.len(), MAX_RUNS);
        // The active run survives the cut; the subset is then ordered by creation again.
        assert!(ordered.iter().any(|r| r.id == "live"));
        assert_eq!(ordered.last().unwrap().id, "live");
        assert!(ordered.iter().all(|r| r.id != "f49"));
    }

    #[test]
    fn percentages_round_to_integers() {
        assert_eq!(pct(Some(64.4)), json!(64));
        assert_eq!(pct(Some(64.5)), json!(65));
        assert_eq!(pct(Some(0.0)), json!(0));
        assert_eq!(pct(Some(100.0)), json!(100));
        assert_eq!(pct(Some(-3.0)), json!(0));
        assert_eq!(pct(None), Value::Null);
    }

    #[test]
    fn is_mcp_session_skips_wrapper_and_web_sessions() {
        let row = |kind, client: &str| SessionRow {
            id: "s".into(),
            kind,
            client: client.into(),
            cwd: None,
            account: None,
            pid: None,
            started_at: at(0),
            ended_at: None,
            status: "active".into(),
        };
        assert!(is_mcp_session(&row(SessionKind::Orchestrator, "mcp")));
        assert!(is_mcp_session(&row(
            SessionKind::Orchestrator,
            "claude-code"
        )));
        assert!(!is_mcp_session(&row(SessionKind::Orchestrator, "web")));
        assert!(!is_mcp_session(&row(SessionKind::Interactive, "wrapper")));
    }
}

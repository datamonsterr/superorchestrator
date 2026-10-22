//! Short views of supervisor replies for an orchestrator.
//!
//! The supervisor's replies feed the CLI and the web UI too, so they stay complete. What an
//! orchestrator model reads is projected here: nulls and empty values are left out (a missing
//! key means null, empty or false), timestamps are cut to seconds, ids are not repeated inside
//! nested objects, and noise (tool-call events, session ids, scores) is dropped. `get_run`
//! with `verbose: true` returns the full record.

use serde_json::{Map, Value, json};

/// Events `get_run` shows by default: the last few that matter, not every tool call.
const KEY_EVENTS: usize = 5;
/// Event types worth showing; the rest (tool calls, usage, assistant text) is noise next to
/// `state`, `result` and `usage`.
const SIGNIFICANT: &[&str] = &[
    "failover",
    "stall_detected",
    "nudge_sent",
    "escalated",
    "question_asked",
    "question_answered",
    "check_result",
    "review_accepted",
    "review_rejected",
    "run_finished",
    "progress",
];
/// Longest string kept inside an event's data.
const EVENT_TEXT: usize = 300;
const BASE_CHARS: usize = 10;

/// `ts` without the fractional seconds: `2026-10-22T22:29:18.048969Z` becomes
/// `2026-10-22T22:29:18Z`.
pub fn seconds(ts: &str) -> String {
    match ts.split_once('.') {
        Some((head, tail)) if head.len() == 19 && tail.ends_with('Z') => format!("{head}Z"),
        _ => ts.to_string(),
    }
}

/// `v` without nulls and empty arrays or objects, at any depth.
pub fn prune(v: Value) -> Value {
    match v {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(k, v)| (k, prune(v)))
                .filter(|(_, v)| !is_empty(v))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.into_iter().map(prune).collect()),
        other => other,
    }
}

fn is_empty(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
        _ => false,
    }
}

/// Moves `keys` of `from` into a new object, in order, skipping missing ones.
fn pick(from: &mut Map<String, Value>, keys: &[&str]) -> Map<String, Value> {
    let mut out = Map::new();
    for k in keys {
        if let Some(v) = from.remove(*k) {
            out.insert((*k).into(), v);
        }
    }
    out
}

/// The elements of the array at `key`, none when it is missing or not an array.
fn items<'a>(m: &'a Map<String, Value>, key: &str) -> std::slice::Iter<'a, Value> {
    m.get(key)
        .map_or(&[][..], |v| v.as_array().map_or(&[][..], Vec::as_slice))
        .iter()
}

fn cut(s: &str) -> String {
    if s.chars().count() <= EVENT_TEXT {
        return s.to_string();
    }
    let head: String = s.chars().take(EVENT_TEXT).collect();
    format!("{head}...")
}

fn short_strings(v: Value) -> Value {
    match v {
        Value::String(s) => Value::String(cut(&s)),
        Value::Object(m) => {
            Value::Object(m.into_iter().map(|(k, v)| (k, short_strings(v))).collect())
        }
        Value::Array(a) => Value::Array(a.into_iter().map(short_strings).collect()),
        other => other,
    }
}

/// The short view of one run, from a `get_run` reply (or the same fields of a `RunView`).
/// With `brief` only what a caller needs to act after waiting: id, name, state, outcome.
pub fn run_view(raw: Value, brief: bool) -> Value {
    let Value::Object(mut run) = raw else {
        return raw;
    };
    let mut out = pick(
        &mut run,
        &["run_id", "name", "state", "harness", "account", "model"],
    );
    if brief {
        out.remove("harness");
        out.remove("account");
        out.remove("model");
    }
    let state = out
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if let Some(r) = run.remove("result").filter(|r| !r.is_null()) {
        out.insert("result".into(), r);
    }
    out.extend(pick(&mut run, &["error_kind", "branch"]));
    if brief {
        return prune_keep_result(out);
    }
    out.extend(pick(&mut run, &["worktree", "parent_run"]));
    if let Some(Value::Object(u)) = run.remove("usage") {
        let tokens = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
        let cost = u.get("cost").and_then(Value::as_f64).unwrap_or(0.0);
        if tokens("input_tokens") + tokens("output_tokens") > 0 || cost > 0.0 {
            out.insert(
                "usage".into(),
                json!({
                    "input_tokens": tokens("input_tokens"),
                    "output_tokens": tokens("output_tokens"),
                    "cost": (cost * 10_000.0).round() / 10_000.0,
                }),
            );
        }
    }
    if let Some(t) = run.get("finished_at").and_then(Value::as_str) {
        out.insert("finished_at".into(), json!(seconds(t)));
    }
    if let Some(Value::Object(d)) = run.remove("diff") {
        let mut d = d;
        d.remove("branch"); // the run's branch is above
        if let Some(Value::String(base)) = d.get("base") {
            let short: String = base.chars().take(BASE_CHARS).collect();
            d.insert("base".into(), json!(short));
        }
        if d.get("truncated") == Some(&json!(false)) {
            d.remove("truncated");
        }
        out.insert("diff".into(), prune(Value::Object(d)));
    }
    // A waiting run's question, so the caller can answer without reading messages.
    if matches!(state.as_str(), "waiting" | "escalated")
        && let Some(q) = items(&run, "messages")
            .rev()
            .find(|m| m["kind"] == "question" && Some(&m["from_run"]) == out.get("run_id"))
            .and_then(|m| m["body"].as_str())
    {
        out.insert("question".into(), json!(q));
    }
    let events: Vec<Value> = items(&run, "recent_events")
        .filter(|e| is_key_event(e))
        .map(key_event)
        .collect();
    let skip = events.len().saturating_sub(KEY_EVENTS);
    out.insert(
        "events".into(),
        Value::Array(events.into_iter().skip(skip).collect()),
    );
    prune_keep_result(out)
}

fn is_key_event(e: &Value) -> bool {
    let kind = e["type"].as_str().unwrap_or("");
    SIGNIFICANT.contains(&kind)
        || (kind == "state_changed"
            && e["data"]["failed_checks"]
                .as_array()
                .is_some_and(|f| !f.is_empty()))
}

fn key_event(e: &Value) -> Value {
    let mut data = e["data"].clone();
    if let Value::Object(m) = &mut data {
        m.remove("transcript_offset");
    }
    json!({
        "ts": seconds(e["ts"].as_str().unwrap_or("")),
        "type": e["type"],
        "data": short_strings(prune(data)),
    })
}

/// Prunes everything but `result`, which is the agent's own data.
fn prune_keep_result(mut out: Map<String, Value>) -> Value {
    let result = out.remove("result");
    let mut v = prune(Value::Object(out));
    if let (Some(r), Value::Object(m)) = (result, &mut v) {
        m.insert("result".into(), r);
    }
    v
}

/// `spawn_agent` and `fork_agent`: `{run_id, state, result?, warnings?, error_kind?}`.
pub fn spawn_view(raw: Value) -> Value {
    let Value::Object(mut m) = raw else {
        return raw;
    };
    let result = m.remove("result");
    let mut out = pick(&mut m, &["run_id", "state"]);
    if let Some(r) = result.filter(|r| !r.is_null()) {
        out.insert("result".into(), r);
    }
    out.extend(pick(&mut m, &["warnings", "error_kind"]));
    prune_keep_result(out)
}

/// One `wait_agent` update: the run id and what happened, with the event's data flattened.
fn update_view(e: &Value) -> Value {
    let mut out = Map::new();
    if let Value::Object(data) = &e["data"] {
        for (k, v) in data {
            out.insert(k.clone(), v.clone());
        }
    }
    out.remove("transcript_offset");
    let kind = match e["type"].as_str() {
        Some("question_asked") => "question",
        Some(other) => other,
        None => "event",
    };
    out.insert("run_id".into(), e["run_id"].clone());
    out.insert("type".into(), json!(kind));
    prune(Value::Object(out))
}

/// `wait_agent`: `finished` holds one short view per run (see [`run_view`], `brief`), which
/// the caller supplies; `updates` are flattened events; `timed_out` only when true.
pub fn wait_view(raw: &Value, finished: Vec<Value>) -> Value {
    let mut out = Map::new();
    out.insert("finished".into(), Value::Array(finished));
    out.insert(
        "updates".into(),
        Value::Array(
            raw["updates"]
                .as_array()
                .into_iter()
                .flatten()
                .map(update_view)
                .collect(),
        ),
    );
    if raw["timed_out"] == json!(true) {
        out.insert("timed_out".into(), json!(true));
    }
    prune_keep_result(out)
}

/// `route`: the pick and why. Scores, token usage, fingerprints and decider details are
/// left out; `alternatives` are choice ids.
pub fn route_view(raw: Value) -> Value {
    let Value::Object(mut m) = raw else {
        return raw;
    };
    let mut out = pick(
        &mut m,
        &[
            "choice_id",
            "harness",
            "account",
            "model",
            "effort",
            "source",
            "rationale",
        ],
    );
    if let Some(Value::Array(ranked)) = m.remove("ranked_accounts")
        && ranked.len() > 1
    {
        out.insert("ranked_accounts".into(), Value::Array(ranked));
    }
    let alternatives: Vec<Value> = items(&m, "alternatives")
        .filter_map(|a| a.get("choice_id").cloned())
        .collect();
    out.insert("alternatives".into(), Value::Array(alternatives));
    prune(Value::Object(out))
}

/// `accept_run`: `{merged, branch?, merge_command?, note?}`; the call fails when it is refused.
pub fn accept_view(raw: Value) -> Value {
    let Value::Object(mut m) = raw else {
        return raw;
    };
    let merged = m.remove("merged").unwrap_or(json!(false));
    let mut out = Map::new();
    out.insert("merged".into(), merged);
    out.extend(pick(&mut m, &["branch", "merge_command", "note"]));
    prune(Value::Object(out))
}

/// `read_messages`: messages without their ids and the caller as recipient.
pub fn messages_view(raw: Value) -> Value {
    if !raw["messages"].is_array() {
        return raw;
    }
    let messages: Vec<Value> = raw["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|m| {
            let mut m = m.clone();
            if let Value::Object(o) = &mut m {
                o.remove("id");
                o.remove("to_run");
                if let Some(t) = o.get("created_at").and_then(Value::as_str) {
                    let t = seconds(t);
                    o.insert("created_at".into(), json!(t));
                }
            }
            prune(m)
        })
        .collect();
    json!({ "messages": messages })
}

/// `list_agents`: the agents without their file paths.
pub fn agents_view(raw: Value) -> Value {
    if !raw["agents"].is_array() {
        return raw;
    }
    let agents: Vec<Value> = raw["agents"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|a| {
            let mut a = a.clone();
            if let Value::Object(o) = &mut a {
                o.remove("path");
            }
            prune(a)
        })
        .collect();
    json!({ "agents": agents })
}

/// `run_workflow`: `{id, state, result?, error?}`.
pub fn workflow_run_view(raw: Value) -> Value {
    let Value::Object(mut m) = raw else {
        return raw;
    };
    let result = m.remove("result");
    let mut out = pick(&mut m, &["id", "state"]);
    if let Some(r) = result.filter(|r| !r.is_null()) {
        out.insert("result".into(), r);
    }
    out.extend(pick(&mut m, &["error"]));
    prune_keep_result(out)
}

/// `workflow_status`: the run and its steps in order, without session id, step timestamps or
/// indexes.
pub fn workflow_status_view(raw: Value) -> Value {
    let Value::Object(mut m) = raw else {
        return raw;
    };
    let result = m.remove("result");
    let mut out = pick(&mut m, &["id", "name", "state"]);
    if let Some(r) = result.filter(|r| !r.is_null()) {
        out.insert("result".into(), r);
    }
    out.extend(pick(&mut m, &["error"]));
    for k in ["started_at", "finished_at"] {
        if let Some(t) = m.get(k).and_then(Value::as_str) {
            out.insert(k.into(), json!(seconds(t)));
        }
    }
    let steps: Vec<Value> = items(&m, "steps")
        .map(|s| {
            let mut s = s.clone();
            if let Value::Object(o) = &mut s {
                for k in ["index", "started_at", "finished_at"] {
                    o.remove(k);
                }
            }
            prune(s)
        })
        .collect();
    out.insert("steps".into(), Value::Array(steps));
    prune_keep_result(out)
}

/// The short view of `tool`'s reply. Tools not listed are returned as they are.
pub fn project(tool: &str, args: &Value, raw: Value) -> Value {
    match tool {
        "get_run" if args["verbose"] == json!(true) => raw,
        "get_run" => run_view(raw, false),
        "spawn_agent" | "fork_agent" => spawn_view(raw),
        "route" => route_view(raw),
        "accept_run" => accept_view(raw),
        "read_messages" => messages_view(raw),
        "list_agents" => agents_view(raw),
        "run_workflow" => workflow_run_view(raw),
        "workflow_status" => workflow_status_view(raw),
        _ => raw,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_lose_their_fraction() {
        assert_eq!(
            seconds("2026-10-22T22:29:18.048969Z"),
            "2026-10-22T22:29:18Z"
        );
        assert_eq!(seconds("2026-10-22T22:29:18Z"), "2026-10-22T22:29:18Z");
        assert_eq!(seconds("soon"), "soon");
    }

    #[test]
    fn prune_drops_nulls_and_empties_but_not_false_or_zero() {
        let v = prune(json!({"a": null, "b": [], "c": {}, "d": false, "e": 0, "f": {"g": null}}));
        assert_eq!(v, json!({"d": false, "e": 0}));
    }

    fn run() -> Value {
        json!({
            "run_id": "r1", "session_id": "s", "state": "pending_review", "harness": "claude",
            "account": "a", "model": null, "name": "feature", "depth": 1, "cwd": "/repo",
            "result": "done", "branch": "so/f", "worktree": "/wt", "join_command": "x",
            "usage": {"input_tokens": 10, "output_tokens": 2, "cost": 0.0082745},
            "created_at": "2026-01-01T00:00:00.5Z", "finished_at": "2026-01-01T00:00:09.123Z",
            "messages": [{"id": "m", "kind": "result", "body": "ready", "from_run": "r1"}],
            "recent_events": [
                {"type": "tool_call_started", "ts": "2026-01-01T00:00:01Z", "data": {"name": "Bash"}},
                {"type": "check_result", "ts": "2026-01-01T00:00:02.9Z", "data": {"check": "cmd", "ok": true, "transcript_offset": 5}},
                {"type": "usage", "ts": "2026-01-01T00:00:03Z", "data": {"cost": 1}},
            ],
        })
    }

    #[test]
    fn get_run_default_keeps_what_the_caller_acts_on() {
        let v = run_view(run(), false);
        assert_eq!(v["run_id"], "r1");
        assert_eq!(v["state"], "pending_review");
        assert_eq!(v["result"], "done");
        assert_eq!(v["branch"], "so/f");
        assert_eq!(v["worktree"], "/wt");
        assert_eq!(v["usage"]["cost"], 0.0083);
        assert_eq!(v["finished_at"], "2026-01-01T00:00:09Z");
        assert_eq!(
            v["events"],
            json!([{"ts": "2026-01-01T00:00:02Z", "type": "check_result",
                     "data": {"check": "cmd", "ok": true}}])
        );
        for gone in [
            "session_id",
            "depth",
            "cwd",
            "join_command",
            "messages",
            "created_at",
            "model",
            "recent_events",
        ] {
            assert!(v.get(gone).is_none(), "{gone} should be left out");
        }
    }

    #[test]
    fn get_run_shows_only_the_last_few_key_events() {
        let mut r = run();
        r["recent_events"] = Value::Array(
            (0..9)
                .map(|i| json!({"type": "progress", "ts": "2026-01-01T00:00:00Z", "data": {"step": i}}))
                .collect(),
        );
        let v = run_view(r, false);
        let steps: Vec<_> = v["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["data"]["step"].clone())
            .collect();
        assert_eq!(steps, [4, 5, 6, 7, 8].map(Value::from));
    }

    #[test]
    fn a_waiting_run_shows_its_question() {
        let mut r = run();
        r["state"] = json!("waiting");
        r["messages"] = json!([
            {"kind": "question", "body": "which db?", "from_run": "r1"},
            {"kind": "question", "body": "other run", "from_run": "r2"}
        ]);
        assert_eq!(run_view(r, false)["question"], "which db?");
    }

    #[test]
    fn diff_does_not_repeat_the_branch() {
        let mut r = run();
        r["diff"] = json!({"base": "0123456789abcdef", "branch": "so/f", "stat": "s", "patch": "p", "truncated": false});
        let v = run_view(r, false);
        assert_eq!(
            v["diff"],
            json!({"base": "0123456789", "stat": "s", "patch": "p"})
        );
    }

    #[test]
    fn verbose_get_run_is_the_raw_reply() {
        let raw = run();
        assert_eq!(
            project("get_run", &json!({"verbose": true}), raw.clone()),
            raw
        );
        assert_ne!(project("get_run", &json!({}), raw.clone()), raw);
    }

    #[test]
    fn a_result_that_is_null_inside_is_kept_as_the_agent_wrote_it() {
        let v = spawn_view(
            json!({"run_id": "r", "state": "succeeded", "result": {"a": null, "b": []}}),
        );
        assert_eq!(v["result"], json!({"a": null, "b": []}));
        let v =
            spawn_view(json!({"run_id": "r", "state": "starting", "result": null, "warnings": []}));
        assert_eq!(v, json!({"run_id": "r", "state": "starting"}));
    }

    #[test]
    fn wait_updates_flatten_events() {
        let raw = json!({"finished": ["r1"], "timed_out": false, "updates": [
            {"type": "question_asked", "run_id": "r2", "ts": "t", "data": {"question": "q?", "options": ["a", "b"]}},
            {"type": "progress", "run_id": "r2", "data": {"step": "build", "status": "running"}},
        ]});
        let v = wait_view(&raw, vec![json!({"run_id": "r1", "state": "succeeded"})]);
        assert_eq!(v["finished"][0]["state"], "succeeded");
        assert_eq!(
            v["updates"][0],
            json!({"run_id": "r2", "type": "question", "question": "q?", "options": ["a", "b"]})
        );
        assert_eq!(v["updates"][1]["step"], "build");
        assert!(v.get("timed_out").is_none());
        assert_eq!(
            wait_view(&json!({"timed_out": true}), vec![])["timed_out"],
            true
        );
    }

    #[test]
    fn route_keeps_the_pick_and_alternative_ids() {
        let v = route_view(json!({
            "choice_id": "c", "harness": "codex", "account": "a", "model": "m", "effort": null,
            "source": "heuristic", "rationale": "why", "confidence": null, "probabilities": {},
            "usage": {"cost": 0.0}, "config_fingerprint": "f", "ranked_accounts": ["a"],
            "alternatives": [{"choice_id": "d", "account": "b", "score": 0.5}],
        }));
        assert_eq!(
            v,
            json!({"choice_id": "c", "harness": "codex", "account": "a", "model": "m",
                    "source": "heuristic", "rationale": "why", "alternatives": ["d"]})
        );
    }

    #[test]
    fn accept_says_merged_and_how_to_land_unmerged_work() {
        assert_eq!(
            accept_view(json!({"ok": true, "merged": true, "note": "merged by fast-forward"})),
            json!({"merged": true, "note": "merged by fast-forward"})
        );
        assert_eq!(
            accept_view(
                json!({"ok": true, "merged": false, "branch": "b", "merge_command": "git merge b"})
            ),
            json!({"merged": false, "branch": "b", "merge_command": "git merge b"})
        );
    }

    #[test]
    fn workflow_status_drops_ids_and_step_times() {
        let v = workflow_status_view(json!({
            "id": "w", "name": "n", "state": "succeeded", "result": "done", "session_id": "s",
            "started_at": "2026-01-01T00:00:00.1Z", "finished_at": "2026-01-01T00:00:01.2Z",
            "steps": [{"index": 0, "name": "step 1", "state": "succeeded", "run_id": "r",
                        "started_at": "x", "finished_at": "y"}]
        }));
        assert_eq!(
            v,
            json!({"id": "w", "name": "n", "state": "succeeded", "result": "done",
                    "started_at": "2026-01-01T00:00:00Z", "finished_at": "2026-01-01T00:00:01Z",
                    "steps": [{"name": "step 1", "state": "succeeded", "run_id": "r"}]})
        );
    }

    #[test]
    fn empty_message_and_agent_lists_stay_visible() {
        assert_eq!(
            messages_view(json!({"messages": []})),
            json!({"messages": []})
        );
        assert_eq!(agents_view(json!({"agents": []})), json!({"agents": []}));
    }
}

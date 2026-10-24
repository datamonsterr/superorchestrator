#![allow(clippy::unwrap_used)]
//! Token-cost report for the MCP surface: the bytes of `tools/list` and of each response of a
//! canonical orchestrator flow, against a fake-harness supervisor.
//!
//! Run `cargo test -p so-supervisor --test token_report -- --nocapture` to see the table.
//! The "rpc" column is what the supervisor answers (what the MCP bridge used to forward);
//! the "mcp" column is what an orchestrator receives now.

mod common;

use common::*;
use serde_json::{Value, json};
use so_core::proto::{
    METHOD_ACCEPT_RUN, METHOD_GET_RUN, METHOD_LIST_HARNESSES, METHOD_ROUTE, METHOD_RUN_WORKFLOW,
    METHOD_SPAWN_AGENT, METHOD_WAIT_AGENT, METHOD_WORKFLOW_STATUS,
};
use so_supervisor::SupervisorClient;

struct Row {
    name: String,
    rpc: usize,
    mcp: usize,
}

struct Report {
    rows: Vec<Row>,
}

impl Report {
    /// Calls `method` and records the size of the raw reply and of what the MCP bridge hands
    /// the orchestrator for it (`so_mcp::rpc_args` and `so_mcp::mcp_reply`, the bridge's own
    /// code). Returns the raw reply.
    async fn call(&mut self, label: &str, c: &SupervisorClient, method: &str, p: Value) -> Value {
        let raw: Value = c.call(method, &p).await.unwrap();
        // list_harnesses is asked for its short view; the raw column is the full reply.
        let sent = so_mcp::rpc_args(method, &p);
        let reply = if sent == p {
            raw.clone()
        } else {
            c.call::<_, Value>(method, &sent).await.unwrap()
        };
        let mcp = so_mcp::mcp_reply(c, method, &p, reply).await;
        if std::env::var("SHOW").is_ok() {
            println!(
                "== {label}\n{}",
                serde_json::to_string_pretty(&mcp).unwrap()
            );
        }
        self.rows.push(Row {
            name: label.into(),
            rpc: raw.to_string().len(),
            mcp: mcp.to_string().len(),
        });
        raw
    }
}

fn tools_list_rows(rows: &mut Vec<Row>) {
    let mut total = 0;
    let mut lines = Vec::new();
    for spec in so_mcp::tool_specs(false) {
        let n = serde_json::to_string(&spec.tool()).unwrap().len();
        total += n;
        lines.push((spec.name, n));
    }
    println!("tools/list per tool:");
    for (name, n) in &lines {
        println!("  {name:<18}{n:>6}");
    }
    rows.push(Row {
        name: "tools/list (all tools)".into(),
        rpc: total,
        mcp: total,
    });
}

#[tokio::test]
async fn canonical_flow_report() {
    let mut env = Env::new();
    env.config.router.jev.key_env = "SO_TOKEN_REPORT_NO_SUCH_KEY".into();
    env.config.router.levels = vec![
        level(
            "low",
            0,
            37,
            &[
                "opencode/opencode/deepseek-v4.1-flash-free",
                "claude/claude-haiku-5-5/medium",
            ],
        ),
        level(
            "mid",
            38,
            62,
            &[
                "claude/claude-sonnet-5-5/medium",
                "opencode/opencode/claude-opus-5-5",
            ],
        ),
        level("high", 63, 100, &["codex/gpt-6.1-sol/high"]),
    ];
    let work = json!({"shell": "printf '%s' 'b' > b.txt && git add b.txt && git -c user.name=t -c user.email=t@e commit -q -m work"});
    env.account("claude-a", 3, &ok_script(&[work]));
    env.account_with_binary("codex-a", "codex", 2, &env.noop_binary("codex-a"));
    env.account_with_binary("opencode-a", "opencode", 2, &env.noop_binary("opencode-a"));
    let h = env.start().await;
    let c = env.client(&h).await;
    let repo = env.repo.to_str().unwrap().to_string();
    let mut r = Report { rows: Vec::new() };
    tools_list_rows(&mut r.rows);

    r.call("list_harnesses", &c, METHOD_LIST_HARNESSES, json!({}))
        .await;
    r.call(
        "route",
        &c,
        METHOD_ROUTE,
        json!({"prompt": "Add a unit test for the parser"}),
    )
    .await;
    let spawned = r
        .call(
            "spawn_agent (wait, review)",
            &c,
            METHOD_SPAWN_AGENT,
            json!({"prompt": PROMPT, "harness": "claude", "account": "claude-a", "cwd": repo,
                   "review": true, "isolation": "worktree", "wait": true, "name": "feature"}),
        )
        .await;
    let id = spawned["run_id"].as_str().unwrap().to_string();
    r.call("get_run", &c, METHOD_GET_RUN, json!({"run": id}))
        .await;
    r.call(
        "get_run (diff)",
        &c,
        METHOD_GET_RUN,
        json!({"run": id, "diff": true}),
    )
    .await;
    let second = r
        .call(
            "spawn_agent (no wait)",
            &c,
            METHOD_SPAWN_AGENT,
            json!({"prompt": PROMPT, "harness": "claude", "account": "claude-a", "cwd": repo,
                   "isolation": "none"}),
        )
        .await;
    r.call(
        "wait_agent",
        &c,
        METHOD_WAIT_AGENT,
        json!({"runs": [second["run_id"]], "timeout_ms": 20000}),
    )
    .await;
    r.call(
        "accept_run (merge)",
        &c,
        METHOD_ACCEPT_RUN,
        json!({"run": id, "merge": true}),
    )
    .await;
    let wf = r
        .call(
            "run_workflow",
            &c,
            METHOD_RUN_WORKFLOW,
            json!({"script": "return await agent('do the task', {harness: 'claude', isolation: 'none'});"}),
        )
        .await;
    r.call(
        "workflow_status",
        &c,
        METHOD_WORKFLOW_STATUS,
        json!({"id": wf["id"]}),
    )
    .await;

    let (mut rpc, mut mcp) = (0, 0);
    println!("{:<28}{:>8}{:>8}", "call", "rpc", "mcp");
    for row in &r.rows {
        println!("{:<28}{:>8}{:>8}", row.name, row.rpc, row.mcp);
        if !row.name.starts_with("tools/list") {
            rpc += row.rpc;
            mcp += row.mcp;
        }
    }
    println!("{:<28}{:>8}{:>8}", "responses total", rpc, mcp);
    // Budgets: at least 40% under the sizes measured before token tuning. 9712 and 11997 are
    // this report's "rpc" totals at commit f763e0e, before any of the MCP changes (see
    // docs/plans/2026-10-23-token-tuning.md).
    let list = r.rows[0].mcp;
    assert!(list <= 9712 * 60 / 100, "tools/list is {list} bytes");
    assert!(mcp <= 11997 * 60 / 100, "responses total {mcp} bytes");
    h.shutdown().await;
}

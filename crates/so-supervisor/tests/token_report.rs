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
use so_core::{Choice, Harness};
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
    /// Calls `method`, records the size of the raw reply, and returns it.
    async fn call(&mut self, label: &str, c: &SupervisorClient, method: &str, p: Value) -> Value {
        let raw: Value = c.call(method, &p).await.unwrap();
        if std::env::var("SHOW").is_ok() {
            println!(
                "== {label}\n{}",
                serde_json::to_string_pretty(&raw).unwrap()
            );
        }
        self.rows.push(Row {
            name: label.into(),
            rpc: raw.to_string().len(),
            mcp: raw.to_string().len(),
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
    let choice = |id: &str, harness, model: &str, effort: Option<&str>, desc: &str| Choice {
        id: id.into(),
        harness,
        model: model.into(),
        effort: effort.map(Into::into),
        roles: vec!["worker".into()],
        description: desc.into(),
        enabled: true,
    };
    env.config.choices = vec![
        choice(
            "claude-mid",
            Harness::Claude,
            "claude-sonnet-5-5",
            Some("medium"),
            "Balanced Claude model for most coding tasks",
        ),
        choice(
            "claude-fast",
            Harness::Claude,
            "claude-haiku-5-5",
            Some("medium"),
            "Fast Claude model for mechanical edits",
        ),
        choice(
            "codex-big",
            Harness::Codex,
            "gpt-6.1-sol",
            Some("high"),
            "Strongest Codex model for hard problems",
        ),
        choice(
            "oc-free",
            Harness::Opencode,
            "opencode/deepseek-v4.1-flash-free",
            None,
            "Free opencode model for simple tasks",
        ),
        choice(
            "oc-paid",
            Harness::Opencode,
            "opencode/claude-opus-5-5",
            None,
            "Paid opencode model, high tier",
        ),
    ];
    let work = json!({"shell": "printf '%s' 'b' > b.txt && git add b.txt && git -c user.name=t -c user.email=t@e commit -q -m work"});
    env.account("claude-a", 3, &ok_script(&[work]));
    env.account_with_binary("codex-a", "codex", 2, std::path::Path::new("/bin/true"));
    env.account_with_binary(
        "opencode-a",
        "opencode",
        2,
        std::path::Path::new("/bin/true"),
    );
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
    h.shutdown().await;
}

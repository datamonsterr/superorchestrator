#![allow(clippy::unwrap_used)]
//! Model refs (`harness/model[/effort]`): routing pins them, `spawn_agent` resolves them and
//! `list_harnesses` shows them.

mod common;

use std::path::Path;

use common::*;
use serde_json::{Value, json};
use so_core::proto::{INVALID_PARAMS, RouteResult, RouteSource};
use so_core::{Harness, Isolation};

const CLAUDE: &str = "claude/claude-sonnet-5-5/high";
const AGY: &str = "agy/gemini-3.8-flash-low";
const CODEX: &str = "codex/gpt-6.1-sol/high";
const OPENCODE: &str = "opencode/opencode/deepseek-v4.1-flash-free";

/// One fake account per harness and a config whose levels list one ref per harness.
fn env() -> Env {
    let mut env = Env::new();
    env.config.router.levels = levels(&[CLAUDE, AGY, CODEX, OPENCODE]);
    env.config.router.jev.key_env = "SO_MODEL_REFS_NO_SUCH_KEY".into();
    env.account("claude-a", 2, &ok_script(&[]));
    #[cfg(target_os = "linux")]
    env.agy_account("agy-a", 2, &ok_script(&[]));
    env.account_with_binary("codex-a", "codex", 2, Path::new("/bin/true"));
    env.account_with_binary("opencode-a", "opencode", 2, Path::new("/bin/true"));
    env
}

fn parts(r: &str) -> (&'static str, &'static str, Option<&'static str>) {
    match r {
        CLAUDE => ("claude", "claude-sonnet-5-5", Some("high")),
        AGY => ("agy", "gemini-3.8-flash-low", None),
        CODEX => ("codex", "gpt-6.1-sol", Some("high")),
        OPENCODE => ("opencode", "opencode/deepseek-v4.1-flash-free", None),
        _ => unreachable!(),
    }
}

#[tokio::test]
async fn route_pins_each_harness_by_ref() {
    let env = env();
    let h = env.start().await;
    let c = env.client(&h).await;
    for (r, account) in [
        (CLAUDE, "claude-a"),
        #[cfg(target_os = "linux")]
        (AGY, "agy-a"),
        (CODEX, "codex-a"),
        (OPENCODE, "opencode-a"),
    ] {
        let got: RouteResult = c
            .call("route", json!({"prompt": "do the task", "model": r}))
            .await
            .unwrap();
        let (harness, model, effort) = parts(r);
        assert_eq!(got.choice_id, r);
        assert_eq!(got.harness.as_str(), harness, "{r}");
        assert_eq!(got.model, model, "{r}");
        assert_eq!(got.effort.as_deref(), effort, "{r}");
        assert_eq!(got.account, account, "{r}");
        assert_eq!(got.source, RouteSource::Pinned, "{r}");
    }
    h.shutdown().await;
}

/// Cancels `run` and waits for it to end, so that nothing is in flight at teardown.
async fn settle(c: &so_supervisor::SupervisorClient, run: &str) {
    let _: Value = c
        .call(
            so_core::proto::METHOD_CANCEL_AGENT,
            so_core::proto::RunRef { run: run.into() },
        )
        .await
        .unwrap_or_default();
    wait_for(c, run).await;
}

/// Spawns by ref without waiting and returns the stored run's `(choice_id, harness, model,
/// effort)`.
async fn spawn_by_ref(
    env: &Env,
    c: &so_supervisor::SupervisorClient,
    model: &str,
) -> (Option<String>, Harness, Option<String>, Option<String>) {
    let mut p = env.spawn_params("unused");
    p.harness = None;
    p.account = None;
    p.model = Some(model.into());
    p.isolation = Some(Isolation::None);
    let r = spawn(c, p).await;
    let row = env.store().get_run(&r.run_id).unwrap().unwrap();
    settle(c, &r.run_id).await;
    (row.choice_id, row.harness, row.model, row.effort)
}

#[tokio::test]
async fn spawn_by_ref_stores_choice_harness_model_and_effort() {
    let env = env();
    let h = env.start().await;
    let c = env.client(&h).await;
    // Claude and agy run to completion on the scripted harness; codex and opencode accounts
    // here are `/bin/true`, so those runs start and fail, but the stored route is the same.
    let mut refs = vec![CLAUDE, CODEX, OPENCODE];
    if cfg!(target_os = "linux") {
        refs.push(AGY);
    }
    for r in refs {
        let (choice, harness, model, effort) = spawn_by_ref(&env, &c, r).await;
        let (eh, em, ee) = parts(r);
        assert_eq!(choice.as_deref(), Some(r));
        assert_eq!(harness.as_str(), eh, "{r}");
        assert_eq!(model.as_deref(), Some(em), "{r}");
        assert_eq!(effort.as_deref(), ee, "{r}");
    }
    h.shutdown().await;
}

#[tokio::test]
async fn a_claude_ref_run_reaches_the_harness_with_model_and_effort() {
    let env = env();
    let h = env.start().await;
    let c = env.client(&h).await;
    let mut p = env.spawn_params("unused");
    p.harness = None;
    p.account = None;
    p.model = Some(CLAUDE.into());
    p.isolation = Some(Isolation::None);
    p.wait = true;
    let r = spawn(&c, p).await;
    assert_eq!(r.state, so_core::RunState::Succeeded, "{r:?}");
    let argv = env.launches("claude-a").pop().unwrap();
    assert!(
        argv.windows(2)
            .any(|w| w == ["--model", "claude-sonnet-5-5"]),
        "{argv:?}"
    );
    assert!(
        argv.windows(2).any(|w| w == ["--effort", "high"]),
        "{argv:?}"
    );
    h.shutdown().await;
}

async fn spawn_error(
    c: &so_supervisor::SupervisorClient,
    p: so_core::proto::SpawnParams,
) -> (i64, String) {
    match c.call::<_, Value>("spawn_agent", p).await.unwrap_err() {
        so_supervisor::ClientError::Rpc(e) => (e.code, e.message),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn a_ref_that_contradicts_harness_or_effort_is_invalid_params() {
    let env = env();
    let h = env.start().await;
    let c = env.client(&h).await;
    let mut p = env.spawn_params("unused");
    p.account = None;
    p.model = Some(CODEX.into());
    p.harness = Some("agy".into());
    let (code, msg) = spawn_error(&c, p.clone()).await;
    assert_eq!(code, INVALID_PARAMS);
    assert_eq!(msg, "model ref names codex but harness is agy");
    p.harness = None;
    p.effort = Some("low".into());
    let (code, msg) = spawn_error(&c, p.clone()).await;
    assert_eq!(code, INVALID_PARAMS);
    assert_eq!(msg, "model ref has effort high but effort is low");
    p.effort = None;
    p.model = Some("codex/".into());
    let (code, _) = spawn_error(&c, p).await;
    assert_eq!(code, INVALID_PARAMS);
    h.shutdown().await;
}

#[tokio::test]
async fn opencode_with_one_segment_is_a_plain_model_name() {
    let env = env();
    let h = env.start().await;
    let c = env.client(&h).await;
    let mut p = env.spawn_params("unused");
    p.account = None;
    p.harness = Some("opencode".into());
    p.model = Some("opencode/deepseek-v4.1-flash-free".into());
    p.isolation = Some(Isolation::None);
    let r = spawn(&c, p).await;
    let row = env.store().get_run(&r.run_id).unwrap().unwrap();
    settle(&c, &r.run_id).await;
    assert_eq!(row.harness, Harness::Opencode);
    assert_eq!(
        row.model.as_deref(),
        Some("opencode/deepseek-v4.1-flash-free")
    );
    h.shutdown().await;
}

#[tokio::test]
async fn an_agent_file_ref_pins_harness_model_and_effort() {
    let env = env();
    let agents = env.paths.agents_dir();
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::write(
        agents.join("pinned.md"),
        format!("---\nname: pinned\ndescription: d\nmodel: {CODEX}\n---\nbody\n"),
    )
    .unwrap();
    let h = env.start().await;
    let c = env.client(&h).await;
    let mut p = env.spawn_params("unused");
    p.harness = None;
    p.account = None;
    p.agent = Some("pinned".into());
    p.isolation = Some(Isolation::None);
    let r = spawn(&c, p).await;
    let row = env.store().get_run(&r.run_id).unwrap().unwrap();
    settle(&c, &r.run_id).await;
    assert_eq!(row.choice_id.as_deref(), Some(CODEX));
    assert_eq!(row.harness, Harness::Codex);
    assert_eq!(row.model.as_deref(), Some("gpt-6.1-sol"));
    assert_eq!(row.effort.as_deref(), Some("high"));
    h.shutdown().await;
}

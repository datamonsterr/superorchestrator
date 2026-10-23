#![allow(clippy::unwrap_used)]
//! GHSA-8hc9-wp33-mmx9: execution privileges and working directories.
mod common;

use common::*;
use serde_json::{Value, json};
use so_core::proto::{
    INVALID_PARAMS, METHOD_RUN_WORKFLOW, METHOD_SPAWN_AGENT, RunWorkflowResult, SpawnResult,
};
use so_core::{Isolation, RunState};
use so_store::RunPatch;
use so_supervisor::{ClientError, SupervisorClient};

fn assert_rejected(error: ClientError, message: &str) {
    match error {
        ClientError::Rpc(e) => {
            assert_eq!(e.code, INVALID_PARAMS);
            assert!(e.message.contains(message), "{e:?}");
        }
        other => panic!("expected RPC rejection: {other:?}"),
    }
}

#[tokio::test]
async fn ghsa_subagent_checks_allow_only_ci_and_pr_threads() {
    let env = Env::new();
    env.account("claude-a", 8, &ok_script(&[]));
    let h = env.start().await;
    let parent = insert_session_and_run_in(&env.store(), 1, RunState::Running, &env.repo);
    let c = SupervisorClient::connect(h.socket_path()).await.unwrap();
    hello_run(&c, &env.store(), &env.paths, &parent.id).await;
    let mut p = env.spawn_params("claude-a");
    p.review = true;
    p.isolation = Some(Isolation::None);
    p.checks = vec!["cmd:true".into()];
    let error = c
        .call::<_, SpawnResult>(METHOD_SPAWN_AGENT, p.clone())
        .await
        .unwrap_err();
    assert_rejected(error, "only ci and pr_threads");
    assert!(env.launches("claude-a").is_empty());
    p.cwd = None;
    for check in ["ci", "pr_threads"] {
        p.checks = vec![check.into()];
        let r = spawn(&c, p.clone()).await;
        assert_eq!(view(&c, &r.run_id).await.cwd.as_deref(), env.repo.to_str());
    }
    h.shutdown().await;
}

#[tokio::test]
async fn ghsa_orchestrator_keeps_shell_checks_and_unconfined_cwd() {
    let env = Env::new();
    env.account("claude-a", 2, &ok_script(&[]));
    let outside = tempfile::tempdir().unwrap();
    let h = env.start().await;
    let c = env.client(&h).await;
    let mut p = env.spawn_params("claude-a");
    p.cwd = Some(outside.path().to_string_lossy().into_owned());
    p.isolation = Some(Isolation::None);
    p.review = true;
    p.checks = vec!["cmd:true".into()];
    let r = spawn(&c, p).await;
    wait_state(&c, &r.run_id, RunState::PendingReview).await;
    assert_eq!(
        view(&c, &r.run_id).await.cwd.as_deref(),
        outside.path().to_str()
    );
    h.shutdown().await;
}

#[tokio::test]
async fn ghsa_subagent_cwd_is_confined_to_its_worktree() {
    let env = Env::new();
    env.account("claude-a", 8, &ok_script(&[]));
    let base = env.repo.join("worktree");
    std::fs::create_dir_all(base.join("inside")).unwrap();
    std::os::unix::fs::symlink(&env.repo, base.join("escape")).unwrap();
    let h = env.start().await;
    let parent = insert_session_and_run_in(&env.store(), 1, RunState::Running, &env.repo);
    env.store()
        .patch_run(
            &parent.id,
            &RunPatch {
                worktree_path: Some(Some(base.to_string_lossy().into_owned())),
                ..Default::default()
            },
        )
        .unwrap();
    let c = SupervisorClient::connect(h.socket_path()).await.unwrap();
    hello_run(&c, &env.store(), &env.paths, &parent.id).await;
    let mut p = env.spawn_params("claude-a");
    p.isolation = Some(Isolation::None);
    for cwd in [
        "..".to_string(),
        env.repo.to_string_lossy().into_owned(),
        "escape".into(),
    ] {
        p.cwd = Some(cwd);
        let e = c
            .call::<_, SpawnResult>(METHOD_SPAWN_AGENT, p.clone())
            .await
            .unwrap_err();
        assert_rejected(e, "inside");
    }
    assert!(env.launches("claude-a").is_empty());
    for cwd in [
        None,
        Some("inside/..".into()),
        Some(base.join("inside").to_string_lossy().into_owned()),
    ] {
        p.cwd = cwd.clone();
        let r = spawn(&c, p.clone()).await;
        wait_state(&c, &r.run_id, RunState::Succeeded).await;
        let expected = if cwd.as_ref().is_some_and(|v| v.ends_with("inside")) {
            base.join("inside")
        } else {
            base.clone()
        };
        assert_eq!(view(&c, &r.run_id).await.cwd.as_deref(), expected.to_str());
    }
    h.shutdown().await;
}

#[tokio::test]
async fn ghsa_subagent_workflow_rejects_script_but_runs_saved_steps_in_run_cwd() {
    let env = Env::new();
    env.account("claude-a", 4, &ok_script(&[]));
    let base = env.repo.join("worktree");
    std::fs::create_dir_all(&base).unwrap();
    let scripts = env.paths.workflows_dir();
    std::fs::create_dir_all(&scripts).unwrap();
    std::fs::write(scripts.join("saved.js"), "return await agent('do the task', {harness:'claude', account:'claude-a', isolation:'none'});").unwrap();
    let h = env.start().await;
    let parent = insert_session_and_run_in(&env.store(), 1, RunState::Running, &env.repo);
    env.store()
        .patch_run(
            &parent.id,
            &RunPatch {
                worktree_path: Some(Some(base.to_string_lossy().into_owned())),
                ..Default::default()
            },
        )
        .unwrap();
    let c = SupervisorClient::connect(h.socket_path()).await.unwrap();
    hello_run(&c, &env.store(), &env.paths, &parent.id).await;
    let e = c
        .call::<_, Value>(METHOD_RUN_WORKFLOW, json!({"script":"return 1;"}))
        .await
        .unwrap_err();
    assert_rejected(e, "saved workflow by name");
    assert!(env.store().list_workflow_runs(10).unwrap().is_empty());
    let r: RunWorkflowResult = c
        .call(METHOD_RUN_WORKFLOW, json!({"name":"saved"}))
        .await
        .unwrap();
    assert_eq!(r.state.as_deref(), Some("succeeded"), "{r:?}");
    let steps = env.store().list_workflow_steps(&r.id).unwrap();
    let run = env
        .store()
        .get_run(steps[0].run_id.as_deref().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(run.depth, 2);
    assert_eq!(run.cwd.as_deref(), base.to_str());
    h.shutdown().await;
}

#[tokio::test]
async fn ghsa_subagent_saved_workflows_ignore_project_scripts_and_shadowing() {
    let env = Env::new();
    let base = env.repo.join("worktree");
    let project = base.join(".superorchestrator/workflows");
    let config = env.paths.workflows_dir();
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(project.join("evil.js"), "return 'evil';").unwrap();
    std::fs::write(project.join("trusted.js"), "return 'shadow';").unwrap();
    std::fs::write(config.join("trusted.js"), "return 'config';").unwrap();
    let h = env.start().await;
    let parent = insert_session_and_run_in(&env.store(), 1, RunState::Running, &env.repo);
    env.store()
        .patch_run(
            &parent.id,
            &RunPatch {
                worktree_path: Some(Some(base.to_string_lossy().into_owned())),
                ..Default::default()
            },
        )
        .unwrap();
    let c = SupervisorClient::connect(h.socket_path()).await.unwrap();
    hello_run(&c, &env.store(), &env.paths, &parent.id).await;
    let e = c
        .call::<_, Value>(METHOD_RUN_WORKFLOW, json!({"name":"evil"}))
        .await
        .unwrap_err();
    assert_eq!(rpc_code(e), so_core::proto::NOT_FOUND);
    assert!(env.store().list_workflow_runs(10).unwrap().is_empty());
    let r: RunWorkflowResult = c
        .call(METHOD_RUN_WORKFLOW, json!({"name":"trusted"}))
        .await
        .unwrap();
    assert_eq!(r.state.as_deref(), Some("succeeded"), "{r:?}");
    assert_eq!(r.result, Some(json!("config")));

    // Depth-0 callers retain project workflows and project-over-config precedence.
    let orchestrator = SupervisorClient::connect(h.socket_path()).await.unwrap();
    orchestrator
        .hello("orchestrator", base.to_str().unwrap(), None, None)
        .await
        .unwrap();
    for (name, result) in [("evil", "evil"), ("trusted", "shadow")] {
        let r: RunWorkflowResult = orchestrator
            .call(METHOD_RUN_WORKFLOW, json!({"name":name}))
            .await
            .unwrap();
        assert_eq!(r.state.as_deref(), Some("succeeded"), "{r:?}");
        assert_eq!(r.result, Some(json!(result)));
    }
    h.shutdown().await;
}

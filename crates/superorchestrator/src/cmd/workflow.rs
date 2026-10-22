//! `workflow run|list|status` (SPEC 11, 15) and the `runs watch --workflow` follower.

use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::{Value, json};
use so_core::proto::{
    METHOD_RUN_WORKFLOW, METHOD_WORKFLOW_STATUS, Notification, RunWorkflowParams,
    RunWorkflowResult, WorkflowStatusParams, WorkflowStatusResult, WorkflowStep,
};
use so_workflow::{SavedSource, SavedWorkflows};
use tokio::sync::mpsc::UnboundedReceiver;

use super::runs::{connect, rpc, runtime};
use crate::cli::{WorkflowCmd, WorkflowRun};
use crate::ctx::Ctx;
use crate::exit::FAILURE;
use crate::output::write_table;
use crate::views::open_existing_store;

/// Most runs `workflow list` shows.
const LIST_LIMIT: usize = 50;

pub fn run(ctx: &Ctx, cmd: WorkflowCmd) -> Result<ExitCode> {
    match cmd {
        WorkflowCmd::Run(args) => runtime()?.block_on(run_workflow(ctx, args)),
        WorkflowCmd::List => list(ctx).map(|()| ExitCode::SUCCESS),
        WorkflowCmd::Status { id } => runtime()?
            .block_on(status(ctx, &id))
            .map(|()| ExitCode::SUCCESS),
    }
}

/// "<step>: <status>" for a workflow's progress event; `None` for any other notification.
fn progress_line(n: &Notification) -> Option<String> {
    if n.method != "event" || n.params["type"] != "progress" {
        return None;
    }
    let d = &n.params["data"];
    d["workflow_id"].as_str()?;
    Some(format!(
        "{}: {}",
        d["step"].as_str().unwrap_or("?"),
        d["status"].as_str().unwrap_or("")
    ))
}

/// Progress goes to stderr, so stdout carries only the result.
fn print_progress(n: &Notification) {
    if let Some(line) = progress_line(n) {
        eprintln!("{line}");
    }
}

async fn run_workflow(ctx: &Ctx, a: WorkflowRun) -> Result<ExitCode> {
    let script = match &a.file {
        Some(path) => Some(
            std::fs::read_to_string(path)
                .with_context(|| format!("could not read {}", path.display()))?,
        ),
        None => None,
    };
    let args = match &a.args {
        Some(text) => Some(serde_json::from_str::<Value>(text).context("--args must be JSON")?),
        None => None,
    };
    let params = RunWorkflowParams {
        name: a.name.clone(),
        script,
        args,
        wait: !a.detach,
    };
    let client = connect(ctx).await?;
    let mut notes: Option<UnboundedReceiver<Notification>> = client.notifications();
    let call = client.call::<_, RunWorkflowResult>(METHOD_RUN_WORKFLOW, &params);
    tokio::pin!(call);
    let result = loop {
        tokio::select! {
            Some(n) = async { match notes.as_mut() { Some(r) => r.recv().await, None => None } } => print_progress(&n),
            r = &mut call => break r.map_err(rpc)?,
        }
    };
    // Notifications are queued ahead of the reply.
    while let Some(n) = notes.as_mut().and_then(|r| r.try_recv().ok()) {
        print_progress(&n);
    }
    let failed = !a.detach && result.state.as_deref() != Some("succeeded");
    ctx.emit(&result, |w| {
        if a.detach {
            return writeln!(w, "{}", result.id);
        }
        match &result.result {
            Some(Value::String(s)) => writeln!(w, "{s}"),
            Some(v) => writeln!(w, "{}", serde_json::to_string_pretty(v).unwrap_or_default()),
            None => Ok(()),
        }
    })?;
    if failed {
        eprintln!(
            "error: workflow {} {}: {}",
            result.id,
            result.state.as_deref().unwrap_or("failed"),
            result.error.as_deref().unwrap_or("no reason recorded")
        );
        return Ok(ExitCode::from(FAILURE));
    }
    Ok(ExitCode::SUCCESS)
}

fn list(ctx: &Ctx) -> Result<()> {
    let cwd = std::env::current_dir().context("no working directory")?;
    let saved = SavedWorkflows::new(ctx.paths.workflows_dir(), cwd).list();
    let runs = match open_existing_store(&ctx.paths) {
        Some(store) => store.list_workflow_runs(LIST_LIMIT)?,
        None => Vec::new(),
    };
    let source = |s: SavedSource| match s {
        SavedSource::Project => "project",
        SavedSource::Config => "config",
    };
    let value = json!({
        "saved": saved.iter().map(|w| json!({
            "name": w.name,
            "source": source(w.source),
            "path": w.path,
        })).collect::<Vec<_>>(),
        "runs": runs.iter().map(|r| json!({
            "id": r.id,
            "name": r.name,
            "state": r.state,
            "session_id": r.session_id,
            "started_at": r.started_at,
            "finished_at": r.finished_at,
        })).collect::<Vec<_>>(),
    });
    ctx.emit(&value, |w| {
        writeln!(w, "saved workflows:")?;
        let rows: Vec<Vec<String>> = saved
            .iter()
            .map(|s| {
                vec![
                    s.name.clone(),
                    source(s.source).to_string(),
                    s.path.display().to_string(),
                ]
            })
            .collect();
        write_table(w, &["NAME", "SOURCE", "PATH"], &rows)?;
        writeln!(w, "\nruns:")?;
        let rows: Vec<Vec<String>> = runs
            .iter()
            .map(|r| {
                vec![
                    r.id.clone(),
                    r.name.clone(),
                    r.state.clone(),
                    r.started_at.format("%Y-%m-%d %H:%M:%S").to_string(),
                ]
            })
            .collect();
        write_table(w, &["RUN", "NAME", "STATE", "STARTED"], &rows)
    })
}

async fn get(client: &so_supervisor::SupervisorClient, id: &str) -> Result<WorkflowStatusResult> {
    client
        .call(
            METHOD_WORKFLOW_STATUS,
            WorkflowStatusParams { id: id.into() },
        )
        .await
        .map_err(rpc)
}

fn step_line(s: &WorkflowStep) -> String {
    let run = s
        .run_id
        .as_deref()
        .map(|r| format!("  run {}", r.chars().take(8).collect::<String>()))
        .unwrap_or_default();
    format!("#{} {}  {}{run}", s.index + 1, s.name, s.state)
}

async fn status(ctx: &Ctx, id: &str) -> Result<()> {
    let client = connect(ctx).await?;
    let s = get(&client, id).await?;
    ctx.emit(&s, |w| {
        writeln!(w, "workflow: {}", s.id)?;
        writeln!(w, "name:     {}", s.name)?;
        writeln!(w, "state:    {}", s.state)?;
        if let Some(t) = s.started_at {
            writeln!(w, "started:  {}", t.format("%Y-%m-%d %H:%M:%S"))?;
        }
        if let Some(t) = s.finished_at {
            writeln!(w, "finished: {}", t.format("%Y-%m-%d %H:%M:%S"))?;
        }
        writeln!(w, "steps:")?;
        for step in &s.steps {
            writeln!(w, "  {}", step_line(step))?;
        }
        if let Some(e) = &s.error {
            writeln!(w, "error:    {e}")?;
        }
        if let Some(r) = &s.result {
            writeln!(w, "result:   {r}")?;
        }
        Ok(())
    })
}

/// `runs watch --workflow <id>`: prints each step's state as it changes until the workflow
/// is no longer running. Exits 0 when it succeeded.
pub async fn watch(ctx: &Ctx, id: &str) -> Result<ExitCode> {
    let client = connect(ctx).await?;
    let mut shown: Vec<String> = Vec::new();
    loop {
        let s = get(&client, id).await?;
        for step in &s.steps {
            let line = step_line(step);
            let i = step.index as usize;
            if shown.get(i) != Some(&line) {
                println!("{line}");
                if shown.len() <= i {
                    shown.resize(i + 1, String::new());
                }
                shown[i] = line;
            }
        }
        if s.state != "running" {
            println!("{}", s.state);
            if let Some(e) = &s.error {
                eprintln!("error: {e}");
            }
            return Ok(if s.state == "succeeded" {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(FAILURE)
            });
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

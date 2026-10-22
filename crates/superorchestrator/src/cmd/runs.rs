//! `runs list|show|watch|cancel` and `join`.

use std::os::unix::process::CommandExt;
use std::process::{Command, ExitCode};

use anyhow::{Context, Result};
use serde_json::{Value, json};
use so_core::proto::{
    JoinResult, METHOD_CANCEL_AGENT, METHOD_GET_RUN, METHOD_JOIN_RUN, METHOD_WAIT_AGENT, NOT_FOUND,
    Notification, RunRef, RunView, WaitParams, WaitResult,
};
use so_core::{Event, Paths, RunState};
use so_store::{RunRow, SessionFilter};
use so_supervisor::runs::display_argv;
use so_supervisor::{ClientError, SupervisorClient, spawn_daemon_process};

use crate::cli::RunsCmd;
use crate::ctx::Ctx;
use crate::exit::CliError;
use crate::output::write_table;
use crate::views::open_existing_store;

pub(crate) fn runtime() -> Result<tokio::runtime::Runtime> {
    Ok(tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?)
}

/// Connects to the supervisor, starting it when it is not running.
pub(crate) async fn connect(ctx: &Ctx) -> Result<SupervisorClient> {
    let socket = Paths::socket_path()?;
    let exe = std::env::current_exe().context("could not find this executable")?;
    let dir = ctx.paths.config_dir.clone();
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (client, _) = SupervisorClient::connect_or_spawn_hello(
        &socket,
        || spawn_daemon_process(&exe, &dir),
        "cli",
        &cwd,
        None,
        Some(std::process::id()),
    )
    .await
    .context("could not reach the supervisor")?;
    Ok(client)
}

fn rpc(e: ClientError) -> anyhow::Error {
    match e {
        ClientError::Rpc(r) if r.code == NOT_FOUND => CliError::not_found(r.message).into(),
        ClientError::Rpc(r) => anyhow::anyhow!("{}", r.message),
        other => other.into(),
    }
}

pub fn run(ctx: &Ctx, cmd: RunsCmd) -> Result<ExitCode> {
    match cmd {
        RunsCmd::List { all } => list(ctx, all).map(|()| ExitCode::SUCCESS),
        RunsCmd::Show { run } => runtime()?
            .block_on(show(ctx, &run))
            .map(|()| ExitCode::SUCCESS),
        RunsCmd::Watch { run } => runtime()?.block_on(watch(ctx, &run)),
        RunsCmd::Cancel { run } => runtime()?
            .block_on(cancel(ctx, &run))
            .map(|()| ExitCode::SUCCESS),
    }
}

fn row_json(r: &RunRow) -> Value {
    json!({
        "id": r.id,
        "session_id": r.session_id,
        "parent_run": r.parent_run_id,
        "depth": r.depth,
        "name": r.name,
        "state": r.state,
        "harness": r.harness,
        "account": r.account,
        "model": r.model,
        "cwd": r.cwd,
        "worktree": r.worktree_path,
        "branch": r.branch,
        "error_kind": r.error_kind,
        "created_at": r.created_at,
        "finished_at": r.finished_at,
    })
}

/// Runs straight from the store; no supervisor needed.
pub fn read_runs(ctx: &Ctx, all: bool) -> Result<Vec<RunRow>> {
    let Some(store) = open_existing_store(&ctx.paths) else {
        return Ok(Vec::new());
    };
    let mut rows = if all {
        let mut rows = Vec::new();
        for s in store.list_sessions(SessionFilter::All, 100_000)? {
            rows.extend(store.list_runs_by_session(&s.id, 100_000)?);
        }
        rows
    } else {
        store.list_active_runs(100_000)?
    };
    rows.sort_by(|a, b| (a.created_at, &a.id).cmp(&(b.created_at, &b.id)));
    Ok(rows)
}

fn list(ctx: &Ctx, all: bool) -> Result<()> {
    let rows = read_runs(ctx, all)?;
    let value = Value::Array(rows.iter().map(row_json).collect());
    ctx.emit(&value, |w| {
        let table: Vec<Vec<String>> = rows
            .iter()
            .map(|r| {
                vec![
                    r.id.clone(),
                    r.state.to_string(),
                    r.harness.to_string(),
                    r.account.clone().unwrap_or_default(),
                    r.depth.to_string(),
                    r.name.clone().unwrap_or_default(),
                    r.created_at.format("%Y-%m-%d %H:%M:%S").to_string(),
                ]
            })
            .collect();
        write_table(
            w,
            &[
                "RUN", "STATE", "HARNESS", "ACCOUNT", "DEPTH", "NAME", "CREATED",
            ],
            &table,
        )
    })
}

async fn get(client: &SupervisorClient, run: &str) -> Result<RunView> {
    client
        .call(METHOD_GET_RUN, RunRef { run: run.into() })
        .await
        .map_err(rpc)
}

fn event_line(e: &Event) -> String {
    let detail = ["name", "excerpt", "step", "status", "reason", "to", "path"]
        .iter()
        .find_map(|k| e.data.get(k).and_then(Value::as_str))
        .unwrap_or("");
    format!(
        "{} {} {}",
        e.ts.with_timezone(&chrono::Local).format("%H:%M:%S"),
        e.event_type,
        detail
    )
    .trim_end()
    .to_string()
}

async fn show(ctx: &Ctx, run: &str) -> Result<()> {
    let client = connect(ctx).await?;
    let v = get(&client, run).await?;
    ctx.emit(&v, |w| {
        writeln!(w, "run:      {}", v.run_id)?;
        writeln!(w, "state:    {}", v.state)?;
        if let Some(n) = &v.name {
            writeln!(w, "name:     {n}")?;
        }
        writeln!(
            w,
            "harness:  {} / {} / {}",
            v.harness.map_or(String::new(), |h| h.to_string()),
            v.account.clone().unwrap_or_default(),
            v.model.clone().unwrap_or_default()
        )?;
        if let Some(p) = &v.worktree {
            writeln!(w, "worktree: {p}")?;
        }
        if let Some(b) = &v.branch {
            writeln!(w, "branch:   {b}")?;
        }
        writeln!(
            w,
            "usage:    {} in / {} out tokens, ${:.4}",
            v.usage.input_tokens, v.usage.output_tokens, v.usage.cost
        )?;
        if let Some(j) = &v.join_command {
            writeln!(w, "join:     {j}")?;
        }
        if let Some(r) = &v.result {
            writeln!(w, "result:   {r}")?;
        }
        writeln!(w, "events:")?;
        for e in &v.recent_events {
            writeln!(w, "  {}", event_line(e))?;
        }
        Ok(())
    })
}

async fn watch(ctx: &Ctx, run: &str) -> Result<ExitCode> {
    let client = connect(ctx).await?;
    let mut notes = client.notifications();
    let v = get(&client, run).await?;
    let print = |n: Notification| {
        if n.method == "event"
            && let Ok(e) = serde_json::from_value::<Event>(n.params)
        {
            println!("{}", event_line(&e));
        }
    };
    for e in &v.recent_events {
        println!("{}", event_line(e));
    }
    let mut state = v.state;
    while !state.is_terminal() {
        let params = WaitParams {
            runs: vec![v.run_id.clone()],
            timeout_ms: Some(30_000),
        };
        let call = client.call::<_, WaitResult>(METHOD_WAIT_AGENT, params);
        tokio::pin!(call);
        let result = loop {
            tokio::select! {
                Some(n) = async { match notes.as_mut() { Some(r) => r.recv().await, None => None } } => print(n),
                r = &mut call => break r,
            }
        };
        result.map_err(rpc)?;
        // Notifications are queued ahead of the reply.
        while let Some(n) = notes.as_mut().and_then(|r| r.try_recv().ok()) {
            print(n);
        }
        state = get(&client, run).await?.state;
    }
    println!("{state}");
    Ok(if state == RunState::Succeeded {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(crate::exit::FAILURE)
    })
}

async fn cancel(ctx: &Ctx, run: &str) -> Result<()> {
    let client = connect(ctx).await?;
    let v: RunView = client
        .call(METHOD_CANCEL_AGENT, RunRef { run: run.into() })
        .await
        .map_err(rpc)?;
    ctx.emit_ok(
        json!({"run": v.run_id, "state": v.state}),
        &format!("run {} is {}", v.run_id, v.state),
    )
}

/// `join <run> [--exec]`.
pub fn join(ctx: &Ctx, run: &str, exec: bool) -> Result<ExitCode> {
    let j = runtime()?.block_on(async {
        let client = connect(ctx).await?;
        client
            .call::<_, JoinResult>(METHOD_JOIN_RUN, RunRef { run: run.into() })
            .await
            .map_err(rpc)
    })?;
    if exec {
        let (program, args) = j.argv.split_first().context("empty join command")?;
        // Only returns on failure; success replaces this process.
        let err = Command::new(program)
            .args(args)
            .envs(j.env.iter().map(|(k, v)| (k, v)))
            .current_dir(&j.cwd)
            .exec();
        return Err(err).with_context(|| format!("could not run {program}"));
    }
    let names: Vec<&str> = j.env.iter().map(|(k, _)| k.as_str()).collect();
    ctx.emit(&json!({"argv": j.argv, "env": names, "cwd": j.cwd}), |w| {
        writeln!(
            w,
            "cd {} && {}",
            display_argv(std::slice::from_ref(&j.cwd)),
            display_argv(&j.argv)
        )?;
        if !names.is_empty() {
            writeln!(w, "(with {} set; use --exec to run it)", names.join(", "))?;
        }
        Ok(())
    })?;
    Ok(ExitCode::SUCCESS)
}

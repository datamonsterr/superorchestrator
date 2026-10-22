//! `route "<task>"` (through the supervisor) and `route eval` (local).

use std::io::Write;
use std::process::ExitCode;

use anyhow::{Context, Result};
use so_core::AccountFile;
use so_core::proto::{METHOD_ROUTE, RouteParams, RouteResult};
use so_router::eval::{self, EvalReport};
use so_router::jev::{process_env, resolve_key};
use so_router::{Catalog, RouteCtx, Snapshot};
use so_store::Store;

use crate::cli::{RouteArgs, RouteSub};
use crate::cmd::runs::{connect, rpc, runtime};
use crate::ctx::Ctx;
use crate::exit::{CliError, FAILURE, USAGE};
use crate::output::write_table;
use crate::views::open_existing_store;

pub fn run(ctx: &Ctx, args: RouteArgs) -> Result<ExitCode> {
    match args.sub {
        Some(RouteSub::Eval {
            cases,
            require_live,
            min_accuracy,
        }) => runtime()?.block_on(run_eval(ctx, cases, require_live, min_accuracy)),
        None => {
            let task = args
                .task
                .ok_or_else(|| CliError::new(USAGE, "route needs a task: route \"<task>\""))?;
            runtime()?
                .block_on(route(ctx, task, args.role, args.explain))
                .map(|()| ExitCode::SUCCESS)
        }
    }
}

async fn route(ctx: &Ctx, task: String, role: Option<String>, explain: bool) -> Result<()> {
    let client = connect(ctx).await?;
    let r: RouteResult = client
        .call(
            METHOD_ROUTE,
            RouteParams {
                prompt: task,
                role,
                ..RouteParams::default()
            },
        )
        .await
        .map_err(rpc)?;
    ctx.emit(&r, |w| {
        let effort = r.effort.as_deref().unwrap_or("default");
        writeln!(
            w,
            "{}  ({} / {}, effort {effort})",
            r.choice_id, r.harness, r.model
        )?;
        writeln!(w, "account:  {}", r.account)?;
        writeln!(
            w,
            "source:   {}",
            serde_json::to_value(r.source)
                .unwrap_or_default()
                .as_str()
                .unwrap_or("")
        )?;
        writeln!(w, "why:      {}", r.rationale)?;
        if explain {
            if let Some(c) = r.confidence {
                writeln!(w, "confidence: {c:.2}")?;
            }
            if !r.probabilities.is_empty() {
                writeln!(w, "probabilities:")?;
                for (id, p) in &r.probabilities {
                    writeln!(w, "  {id}  {p:.2}")?;
                }
            }
            if !r.alternatives.is_empty() {
                writeln!(w, "alternatives:")?;
                let rows: Vec<Vec<String>> = r
                    .alternatives
                    .iter()
                    .map(|a| {
                        vec![
                            a.choice_id.clone(),
                            a.account.clone().unwrap_or_default(),
                            a.score.map(|s| format!("{s:.2}")).unwrap_or_default(),
                        ]
                    })
                    .collect();
                write_table(w, &["CHOICE", "ACCOUNT", "SCORE"], &rows)?;
            }
            writeln!(
                w,
                "usage:    {} in, {} out, cost {}",
                r.usage.input_tokens, r.usage.output_tokens, r.usage.cost
            )?;
            writeln!(w, "config:   {}", r.config_fingerprint)?;
        }
        Ok(())
    })
}

async fn run_eval(
    ctx: &Ctx,
    cases: Option<std::path::PathBuf>,
    require_live: bool,
    min_accuracy: Option<f64>,
) -> Result<ExitCode> {
    if let Some(m) = min_accuracy
        && !(0.0..=1.0).contains(&m)
    {
        return Err(CliError::new(USAGE, "--min-accuracy must be between 0 and 1").into());
    }
    let config = ctx.config()?;
    if require_live && resolve_key(&config.router.jev, process_env).is_none() {
        return Err(CliError::new(
            FAILURE,
            format!(
                "--require-live needs an OpenRouter key: set {} or router.jev.key_file",
                config.router.jev.key_env
            ),
        )
        .into());
    }
    let cases = match cases {
        Some(p) => {
            let text = std::fs::read_to_string(&p)
                .with_context(|| format!("could not read {}", p.display()))?;
            eval::parse_cases(&text).map_err(|e| CliError::new(USAGE, e))?
        }
        None => eval::bundled_cases(),
    };
    let accounts = AccountFile::list(&ctx.paths)?;
    let catalog = Catalog::load(&ctx.paths)?;
    let store = match open_existing_store(&ctx.paths) {
        Some(s) => s,
        None => Store::open_in_memory()?,
    };
    let snapshot = Snapshot::collect(
        &store,
        &accounts,
        f64::from(config.usage.low_pct),
        chrono::Utc::now(),
    )?;
    let http = reqwest::Client::new();
    let rc = RouteCtx {
        config: &config,
        catalog: &catalog,
        snapshot: &snapshot,
        http: &http,
        env: process_env,
    };
    let report = eval::evaluate(&rc, &cases).await;
    ctx.emit(&report, |w| human_report(w, &report))?;

    let mut failures = Vec::new();
    if let Some(m) = min_accuracy
        && report.accuracy < m
    {
        failures.push(format!("accuracy {:.3} is below {m}", report.accuracy));
    }
    if report.eligible_compliance < 1.0 {
        failures.push("a case was routed outside the eligible set".to_string());
    }
    if report.errors_count > 0 {
        failures.push(format!(
            "{} case(s) could not be routed",
            report.errors_count
        ));
    }
    if require_live && report.fallback_count > 0 {
        failures.push(format!(
            "{} case(s) fell back from Jev",
            report.fallback_count
        ));
    }
    if failures.is_empty() {
        return Ok(ExitCode::SUCCESS);
    }
    eprintln!("route eval failed: {}", failures.join("; "));
    Ok(ExitCode::from(FAILURE))
}

fn human_report(w: &mut dyn Write, r: &EvalReport) -> std::io::Result<()> {
    writeln!(w, "cases:               {}", r.case_count)?;
    writeln!(w, "accuracy:            {:.1}%", r.accuracy * 100.0)?;
    writeln!(
        w,
        "eligible compliance: {:.1}%",
        r.eligible_compliance * 100.0
    )?;
    writeln!(w, "fallbacks:           {}", r.fallback_count)?;
    writeln!(w, "errors:              {}", r.errors_count)?;
    let sources: Vec<String> = r.sources.iter().map(|(k, v)| format!("{k}={v}")).collect();
    writeln!(w, "sources:             {}", sources.join(" "))?;
    writeln!(
        w,
        "latency p50/p95:     {:.1} / {:.1} ms",
        r.latency_ms.p50, r.latency_ms.p95
    )?;
    writeln!(w, "total cost:          {}", r.total_cost)?;
    let rows: Vec<Vec<String>> = r
        .cases
        .iter()
        .map(|c| {
            vec![
                c.id.clone(),
                c.choice_id.clone().unwrap_or_else(|| "-".into()),
                if c.correct { "ok" } else { "MISS" }.into(),
                c.source.clone(),
            ]
        })
        .collect();
    write_table(w, &["CASE", "CHOICE", "RESULT", "SOURCE"], &rows)
}

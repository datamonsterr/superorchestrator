//! `models refresh|list`.

use anyhow::Result;
use so_router::Catalog;
use so_router::catalog::SourceStatus;
use so_supervisor::services::models::refresh_and_save;

use crate::cli::ModelsCmd;
use crate::cmd::runs::runtime;
use crate::ctx::Ctx;
use crate::output::write_table;

pub fn run(ctx: &Ctx, cmd: ModelsCmd) -> Result<()> {
    match cmd {
        ModelsCmd::List => list(ctx),
        ModelsCmd::Refresh => runtime()?.block_on(refresh_cmd(ctx)),
    }
}

fn list(ctx: &Ctx) -> Result<()> {
    let catalog = Catalog::load(&ctx.paths)?;
    let rows = catalog.entries();
    let value: Vec<_> = rows
        .iter()
        .map(|(h, m)| {
            serde_json::json!({
                "harness": h, "model": m.id, "tier": m.tier, "cost": m.cost,
                "pool": m.pool, "efforts": m.efforts,
            })
        })
        .collect();
    ctx.emit(&value, |w| {
        let table: Vec<Vec<String>> = rows
            .iter()
            .map(|(h, m)| {
                let s = |v: serde_json::Value| v.as_str().unwrap_or_default().to_string();
                vec![
                    h.to_string(),
                    m.id.clone(),
                    s(serde_json::to_value(m.tier).unwrap_or_default()),
                    s(serde_json::to_value(m.cost).unwrap_or_default()),
                    m.pool.clone().unwrap_or_default(),
                    m.efforts.join(","),
                ]
            })
            .collect();
        write_table(
            w,
            &["HARNESS", "MODEL", "TIER", "COST", "POOL", "EFFORTS"],
            &table,
        )
    })
}

async fn refresh_cmd(ctx: &Ctx) -> Result<()> {
    let host = ctx.host()?;
    let http = reqwest::Client::new();
    // Tests point this at a local server; users never need it.
    let url = std::env::var("SUPERORCHESTRATOR_MODELS_DEV_URL").ok();
    let (_, report, _) = refresh_and_save(&ctx.paths, &host, &http, url.as_deref()).await?;
    ctx.emit(&report, |w| {
        for s in &report.sources {
            let st = match s.status {
                SourceStatus::Ok => "ok",
                SourceStatus::Skipped => "skipped",
                SourceStatus::Failed => "failed",
            };
            writeln!(
                w,
                "{:<11} {st:<8} {:>3} models  {}",
                s.source, s.models, s.detail
            )?;
        }
        writeln!(
            w,
            "added {} model(s) to {}",
            report.added.len(),
            ctx.paths.models_toml().display()
        )?;
        if !report.unknown_choice_models.is_empty() {
            writeln!(
                w,
                "choices whose model is not in the catalog (choices are never edited):"
            )?;
            for c in &report.unknown_choice_models {
                writeln!(w, "  {}  {} / {}", c.choice, c.harness, c.model)?;
            }
        }
        if !report.missing_choice_models.is_empty() {
            writeln!(
                w,
                "choices whose model the harness does not list (runs on them will fail):"
            )?;
            for c in &report.missing_choice_models {
                writeln!(w, "  {}  {} / {}", c.choice, c.harness, c.model)?;
            }
        }
        writeln!(
            w,
            "{} catalog model(s) are used by no choice",
            report.unused_models.len()
        )?;
        Ok(())
    })
}

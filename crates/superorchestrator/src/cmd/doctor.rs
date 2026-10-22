//! `doctor`: environment checks as data, rendered as text or JSON.

use std::process::ExitCode;

use anyhow::Result;
use so_supervisor::doctor::{Status, run_checks};

use crate::ctx::Ctx;

pub fn run(ctx: &Ctx) -> Result<ExitCode> {
    let host = ctx.host()?;
    let report = run_checks(&ctx.paths, &host);
    let failures = report.failures();
    let value = serde_json::json!({ "ok": report.ok, "checks": report.checks });
    ctx.emit(&value, |w| {
        for check in &report.checks {
            let mark = match check.status {
                Status::Ok => "✓",
                Status::Warn => "!",
                Status::Fail => "✗",
            };
            writeln!(w, "{mark} {}: {}", check.name, check.detail)?;
        }
        if failures == 0 {
            writeln!(w, "\nall checks passed")
        } else {
            writeln!(w, "\n{failures} check(s) failed")
        }
    })?;
    Ok(if failures == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

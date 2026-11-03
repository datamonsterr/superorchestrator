//! Quiet, best-effort lifecycle notifications for harnesses.

use std::io::{IsTerminal, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};
use so_core::{EventType, Paths, RunState};
use so_supervisor::SupervisorClient;

use crate::cli::HookCmd;
use crate::ctx::Ctx;
use crate::views::open_existing_store;

struct WaitingRun {
    id: String,
    name: String,
    review: bool,
    question: bool,
}

fn relevant(cwd: &Path, root: &Path) -> bool {
    cwd.starts_with(root)
}

fn format_output(event: HookCmd, runs: &[&WaitingRun]) -> Option<Value> {
    let reviews: Vec<_> = runs.iter().filter(|r| r.review).collect();
    match event {
        HookCmd::Stop if !reviews.is_empty() => {
            let names = reviews
                .iter()
                .map(|r| format!("{} ({})", r.id, r.name))
                .collect::<Vec<_>>()
                .join(", ");
            Some(
                json!({"systemMessage": format!("Superorchestrator runs awaiting review: {names}. Use accept_run or reject_run.")}),
            )
        }
        HookCmd::SessionStart => {
            let mut lines = Vec::new();
            for run in runs {
                if run.review {
                    lines.push(format!(
                        "- {} ({}): pending review; use accept_run or reject_run.",
                        run.id, run.name
                    ));
                }
                if run.question {
                    lines.push(format!(
                        "- {} ({}): unanswered question; use send_message kind: answer.",
                        run.id, run.name
                    ));
                }
            }
            if lines.is_empty() {
                None
            } else {
                Some(
                    json!({"hookSpecificOutput": {"hookEventName": "SessionStart", "additionalContext": format!("Superorchestrator runs needing attention:\n{}", lines.join("\n"))}}),
                )
            }
        }
        _ => None,
    }
}

async fn collect(ctx: &Ctx, cwd: &Path, event: HookCmd) -> Option<Value> {
    let socket = Paths::socket_path().ok()?;
    let client = SupervisorClient::connect(&socket).await.ok()?;
    client
        .hello_inspect("hook", &cwd.to_string_lossy(), Some(std::process::id()))
        .await
        .ok()?;
    let root = tokio::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["rev-parse", "--show-toplevel"])
        .kill_on_drop(true)
        .output()
        .await
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| PathBuf::from(s.trim()))
        .unwrap_or_else(|| cwd.to_path_buf());
    let root = root.canonicalize().unwrap_or(root);
    let store = open_existing_store(&ctx.paths)?;
    let rows = store.list_active_runs(100_000).ok()?;
    let mut runs = Vec::new();
    for row in rows {
        let Some(cwd) = row.cwd.map(PathBuf::from) else {
            continue;
        };
        let cwd = cwd.canonicalize().unwrap_or(cwd);
        if !relevant(&cwd, &root) {
            continue;
        }
        let mut question = false;
        if matches!(event, HookCmd::SessionStart) {
            for e in store.list_events_by_run(&row.id, 100_000).ok()? {
                match e.event_type {
                    EventType::QuestionAsked => question = true,
                    EventType::QuestionAnswered => question = false,
                    _ => {}
                }
            }
        }
        runs.push(WaitingRun {
            id: row.id,
            name: row.name.unwrap_or_default(),
            review: row.state == RunState::PendingReview,
            question,
        });
    }
    format_output(event, &runs.iter().collect::<Vec<_>>())
}

pub fn run(config_dir: Option<PathBuf>, event: HookCmd) {
    let Ok(rt) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return;
    };
    let result = rt.block_on(async {
        tokio::time::timeout(Duration::from_secs(2), async {
            let input = if std::io::stdin().is_terminal() {
                String::new()
            } else {
                let (tx, rx) = tokio::sync::oneshot::channel();
                std::thread::spawn(move || {
                    let mut input = String::new();
                    let _ = std::io::stdin().read_to_string(&mut input);
                    let _ = tx.send(input);
                });
                rx.await.unwrap_or_default()
            };
            let cwd = serde_json::from_str::<Value>(&input)
                .ok()
                .and_then(|v| v.get("cwd").and_then(Value::as_str).map(PathBuf::from))
                .or_else(|| std::env::current_dir().ok())?;
            let ctx = Ctx::new(config_dir, false, false).ok()?;
            collect(&ctx, &cwd, event).await
        })
        .await
        .ok()
        .flatten()
    });
    if let Some(value) = result {
        println!("{value}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn run(id: &str, review: bool, question: bool) -> WaitingRun {
        WaitingRun {
            id: id.into(),
            name: format!("name-{id}"),
            review,
            question,
        }
    }
    #[test]
    fn filters_by_path_components_and_formats_both_events() {
        let runs = [
            run("r1", true, false),
            run("r2", false, true),
            run("r3", true, true),
            run("r4", false, false),
        ];
        let selected: Vec<_> = runs
            .iter()
            .zip(["/repo/a", "/repo/b", "/repo-other", "/repo"])
            .filter(|(_, cwd)| relevant(Path::new(cwd), Path::new("/repo")))
            .map(|(run, _)| run)
            .collect();
        assert_eq!(selected.len(), 3);
        let start = format_output(HookCmd::SessionStart, &selected).unwrap();
        let text = start["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert!(
            text.contains("r1 (name-r1)")
                && text.contains("accept_run")
                && text.contains("reject_run")
        );
        assert!(text.contains("r2 (name-r2)") && text.contains("send_message kind: answer"));
        assert!(!text.contains("r3") && !text.contains("r4"));
        let stop = format_output(HookCmd::Stop, &selected).unwrap();
        assert!(stop["systemMessage"].as_str().unwrap().contains("r1"));
        assert!(stop.get("decision").is_none());
        assert!(format_output(HookCmd::Stop, &[&runs[1]]).is_none());
        assert!(format_output(HookCmd::SessionStart, &[&runs[3]]).is_none());
    }
}

//! Quiet, best-effort lifecycle notifications for harnesses.

use std::collections::HashMap;
use std::io::{IsTerminal, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};
use so_core::{Paths, RunState};
use so_supervisor::SupervisorClient;

use crate::cli::HookCmd;
use crate::ctx::Ctx;

struct WaitingRun {
    id: String,
    name: String,
    review: bool,
    question: bool,
}

#[derive(Clone)]
struct RepoIdentity {
    path: PathBuf,
    common_dir: Option<PathBuf>,
}

/// Upper bound for one `git rev-parse`; the hook as a whole has a 2 s budget.
const GIT_BUDGET: Duration = Duration::from_millis(500);

/// Canonical git common dir of `path`, or `None` when it is not a git checkout (or git is
/// missing, slow, or the directory was deleted). The child never outlives `GIT_BUDGET`.
fn git_common_dir(path: &Path) -> Option<PathBuf> {
    use std::process::{Command, Stdio};
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(path)
        .args(["rev-parse", "--git-common-dir"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // Inherited repository-location variables would point git at the wrong repository.
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_") {
            cmd.env_remove(key);
        }
    }
    let mut child = cmd.spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut out = String::new();
        let _ = stdout.read_to_string(&mut out);
        out
    });
    let deadline = std::time::Instant::now() + GIT_BUDGET;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(5));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    let out = reader.join().ok()?;
    status
        .success()
        .then(|| path.join(out.trim()).canonicalize().ok())?
}

fn repo_identity(cwd: &Path) -> RepoIdentity {
    // A deleted directory cannot be canonicalized or asked about git; such a run falls back
    // to path containment on the path as recorded.
    let path = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let common_dir = git_common_dir(&path);
    RepoIdentity { path, common_dir }
}

fn relevant(run: &RepoIdentity, root: &RepoIdentity) -> bool {
    match (&run.common_dir, &root.common_dir) {
        (Some(a), Some(b)) => a == b,
        _ => run.path.starts_with(&root.path),
    }
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

/// Reads the runs of this repository that wait on the user, from a read-only database.
fn read_waiting(db: &Path, cwd: &Path, event: HookCmd) -> Option<Vec<WaitingRun>> {
    let root = repo_identity(cwd);
    // Deliberate: if the database is missing, locked past the short busy timeout, or has an
    // older schema, the hook prints nothing rather than failing the harness.
    let store = so_store::Store::open_read_only(db).ok()?;
    let rows = store.list_active_runs(100_000).ok()?;
    let mut runs = Vec::new();
    let mut identities = HashMap::from([(cwd.to_path_buf(), root.clone())]);
    for row in rows {
        let Some(run_cwd) = row.cwd.map(PathBuf::from) else {
            continue;
        };
        let identity = identities
            .entry(run_cwd.clone())
            .or_insert_with(|| repo_identity(&run_cwd));
        if !relevant(identity, &root) {
            continue;
        }
        let question = matches!(event, HookCmd::SessionStart)
            && row.parent_run_id.is_none()
            // A failed lookup only drops this run's question flag, not the whole report.
            && store.has_open_question(&row.id).unwrap_or(false);
        runs.push(WaitingRun {
            id: row.id,
            name: row.name.unwrap_or_default(),
            review: row.state == RunState::PendingReview,
            question,
        });
    }
    Some(runs)
}

async fn collect(ctx: &Ctx, cwd: &Path, event: HookCmd) -> Option<Value> {
    // Liveness gate: with no supervisor, print nothing.
    let socket = Paths::socket_path().ok()?;
    let client = SupervisorClient::connect(&socket).await.ok()?;
    client
        .hello_inspect("hook", &cwd.to_string_lossy(), Some(std::process::id()))
        .await
        .ok()?;
    let (db, cwd) = (ctx.paths.state_db(), cwd.to_path_buf());
    let runs = tokio::task::spawn_blocking(move || read_waiting(&db, &cwd, event))
        .await
        .ok()??;
    format_output(event, &runs.iter().collect::<Vec<_>>())
}

pub fn run(ctx: &Ctx, event: HookCmd) {
    let ctx = ctx.clone();
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        if let Some(value) = run_worker(&ctx, event) {
            use std::io::Write;
            let _ = writeln!(std::io::stdout().lock(), "{value}");
        }
        let _ = tx.send(());
    });
    let _ = rx.recv_timeout(Duration::from_secs(2));
}

fn run_worker(ctx: &Ctx, event: HookCmd) -> Option<Value> {
    let mut input = String::new();
    if !std::io::stdin().is_terminal() {
        let _ = std::io::stdin().read_to_string(&mut input);
    }
    let Ok(rt) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return None;
    };
    rt.block_on(async {
        let cwd = serde_json::from_str::<Value>(&input)
            .ok()
            .and_then(|v| v.get("cwd").and_then(Value::as_str).map(PathBuf::from))
            .or_else(|| std::env::current_dir().ok())?;
        collect(ctx, &cwd, event).await
    })
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
    fn formats_both_events_for_the_selected_runs() {
        let runs = [
            run("r1", true, false),
            run("r2", false, true),
            run("r3", true, true),
            run("r4", false, false),
        ];
        let selected: Vec<_> = [&runs[0], &runs[1], &runs[3]].into();
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

    #[test]
    fn repository_identity_distinguishes_worktrees_and_nested_repositories() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let sibling = dir.path().join("sibling");
        let nested = repo.join("nested");
        std::fs::create_dir_all(&repo).unwrap();
        let git = |cwd: &Path, args: &[&str]| {
            let status = std::process::Command::new("git")
                .arg("-C")
                .arg(cwd)
                .args(args)
                .env("GIT_AUTHOR_NAME", "Test")
                .env("GIT_AUTHOR_EMAIL", "test@example.com")
                .env("GIT_COMMITTER_NAME", "Test")
                .env("GIT_COMMITTER_EMAIL", "test@example.com")
                .output()
                .unwrap();
            assert!(
                status.status.success(),
                "{}",
                String::from_utf8_lossy(&status.stderr)
            );
        };
        git(&repo, &["init", "-q"]);
        git(&repo, &["commit", "--allow-empty", "-qm", "initial"]);
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "sibling",
                sibling.to_str().unwrap(),
            ],
        );
        std::fs::create_dir_all(&nested).unwrap();
        git(&nested, &["init", "-q"]);
        let root = repo_identity(&repo);
        assert!(relevant(&repo_identity(&sibling), &root));
        assert!(!relevant(&repo_identity(&nested), &root));
        let outside = dir.path().join("outside");
        let child = outside.join("child");
        std::fs::create_dir_all(&child).unwrap();
        assert!(relevant(&repo_identity(&child), &repo_identity(&outside)));
        assert!(!relevant(&repo_identity(&repo), &repo_identity(&outside)));
    }

    #[test]
    fn read_waiting_reports_runs_while_another_connection_holds_a_write_transaction() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("state.db");
        let store = so_store::Store::open(&db).unwrap();
        let now = chrono::Utc::now();
        let session = so_store::SessionRow {
            id: so_core::new_id(),
            kind: so_core::SessionKind::Orchestrator,
            client: "test".into(),
            cwd: None,
            account: None,
            pid: None,
            started_at: now,
            ended_at: None,
            status: "active".into(),
        };
        store.insert_session(&session).unwrap();
        let run = so_store::RunRow {
            id: so_core::new_id(),
            session_id: session.id.clone(),
            parent_run_id: None,
            depth: 0,
            name: Some("w".into()),
            agent: None,
            choice_id: None,
            harness: so_core::Harness::Codex,
            account: None,
            model: None,
            effort: None,
            cwd: Some(dir.path().to_string_lossy().into_owned()),
            worktree_path: None,
            branch: None,
            native_session_id: None,
            state: RunState::PendingReview,
            review: true,
            output_schema: None,
            result_json: None,
            error_kind: None,
            prompt: None,
            isolation: so_core::Isolation::None,
            max_turns: None,
            worktree_base: None,
            worktree_repo: None,
            timeout_ms: None,
            created_at: now,
            started_at: None,
            finished_at: None,
        };
        store.insert_run(&run).unwrap();
        store
            .append_event(&so_core::Event {
                id: so_core::new_id(),
                session_id: session.id,
                run_id: Some(run.id.clone()),
                ts: now,
                event_type: so_core::EventType::QuestionAsked,
                data: json!({}),
            })
            .unwrap();
        let writer = rusqlite::Connection::open(&db).unwrap();
        writer
            .execute_batch("BEGIN IMMEDIATE; CREATE TABLE hold(x);")
            .unwrap();
        let start = std::time::Instant::now();
        let runs = read_waiting(&db, dir.path(), HookCmd::SessionStart).unwrap();
        assert!(start.elapsed() < Duration::from_secs(2));
        writer.execute_batch("ROLLBACK").unwrap();
        assert_eq!(runs.len(), 1);
        assert!(runs[0].id == run.id && runs[0].review && runs[0].question);
    }
}

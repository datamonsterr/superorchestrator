//! What the terminal UI shows: plain data loaded off the draw loop. Everything here reads
//! through the same library functions the CLI commands use.

use chrono::{DateTime, Utc};
use so_accounts::{HostEnv, SignIn};
use so_core::{EventType, Harness, RunState};
use so_store::{RunRow, Store};

use crate::cmd::{install, setup};
use crate::ctx::Ctx;
use crate::views::{collect_views_with, open_existing_store};

/// How many runs the Runs tab keeps.
const RECENT_RUNS: usize = 200;

#[derive(Debug, Clone, PartialEq)]
pub struct SupInfo {
    pub pid: u32,
    pub version: String,
    pub uptime_s: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AccountRow {
    pub name: String,
    pub harness: Harness,
    pub signed_in: SignIn,
    pub enabled: bool,
    pub cap: u32,
    pub active: i64,
    /// `pool: text, pool: text`.
    pub headroom: String,
    /// A cooldown flag is set (the headroom text says until when).
    pub cooling: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunItem {
    pub id: String,
    pub state: RunState,
    pub harness: Harness,
    pub account: Option<String>,
    pub created_at: DateTime<Utc>,
    pub title: String,
    pub has_worktree: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Ok,
    /// Optional, but worth doing.
    Warn,
    Missing,
}

/// One line of the Overview checklist.
#[derive(Debug, Clone, PartialEq)]
pub struct Check {
    pub label: &'static str,
    pub level: Level,
    pub detail: String,
    /// How to fix it, when it is not `Ok`.
    pub fix: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub at: DateTime<Utc>,
    pub config_dir: String,
    pub supervisor: Option<SupInfo>,
    pub accounts: Vec<AccountRow>,
    pub accounts_error: Option<String>,
    pub runs: Vec<RunItem>,
    pub active_runs: usize,
    pub pending_review: usize,
    pub checks: Vec<Check>,
}

impl Snapshot {
    pub fn signed_in(&self) -> usize {
        self.accounts
            .iter()
            .filter(|a| a.signed_in == SignIn::SignedIn)
            .count()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunDetail {
    pub id: String,
    pub state: RunState,
    pub harness: Harness,
    pub account: Option<String>,
    pub model: Option<String>,
    pub worktree: Option<String>,
    pub branch: Option<String>,
    pub error: Option<String>,
    pub brief: String,
    pub route: Vec<String>,
    pub events: Vec<String>,
    /// How many events the run has (the shown ones are the last few).
    pub events_total: usize,
    pub result: Option<String>,
    /// What the run changed, when the reader was asked for it. The app moves it into the
    /// detail view, which keeps it between reads.
    pub diff: Option<Result<so_core::proto::RunDiff, String>>,
}

/// Replaces control characters, so text from a run cannot move the cursor or recolour the
/// screen.
pub fn clean(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

fn title_of(r: &RunRow) -> String {
    let t = r
        .name
        .clone()
        .filter(|n| !n.trim().is_empty())
        .or_else(|| {
            r.prompt
                .as_deref()
                .and_then(|p| p.lines().find(|l| !l.trim().is_empty()))
                .map(|l| l.trim().to_string())
        })
        .unwrap_or_default();
    clean(&t)
}

fn run_item(r: &RunRow) -> RunItem {
    RunItem {
        id: r.id.clone(),
        state: r.state,
        harness: r.harness,
        account: r.account.clone(),
        created_at: r.created_at,
        title: title_of(r),
        has_worktree: r.worktree_path.is_some(),
    }
}

/// Newest first, at most [`RECENT_RUNS`], and the count of active runs.
fn recent_runs(store: &Store) -> so_store::Result<(Vec<RunRow>, usize)> {
    let rows = store.list_recent_runs(RECENT_RUNS)?;
    let active = usize::try_from(store.count_active_runs()?).unwrap_or(0);
    Ok((rows, active))
}

fn checklist(ctx: &Ctx, host: &HostEnv, accounts: &[AccountRow]) -> Vec<Check> {
    let mut out = Vec::new();
    let has_config = ctx.paths.config_toml().exists();
    out.push(Check {
        label: "config",
        level: if has_config {
            Level::Ok
        } else {
            Level::Missing
        },
        detail: if has_config {
            host.tildify(&ctx.paths.config_dir)
        } else {
            "no config.toml yet".into()
        },
        fix: (!has_config).then(|| "Setup tab (4): run `init`".into()),
    });

    let infos = setup::detect_harnesses(ctx, host, &[]).unwrap_or_default();
    let found: Vec<&str> = infos
        .iter()
        .filter(|i| i.binary.is_some())
        .map(|i| i.harness.as_str())
        .collect();
    out.push(Check {
        label: "harnesses",
        level: if found.is_empty() {
            Level::Missing
        } else {
            Level::Ok
        },
        detail: if found.is_empty() {
            "none on PATH".into()
        } else {
            format!("{} ({} of 4)", found.join(", "), found.len())
        },
        fix: found
            .is_empty()
            .then(|| "install codex, opencode, agy or claude".into()),
    });

    out.push(Check {
        label: "accounts",
        level: if accounts.is_empty() {
            Level::Missing
        } else {
            Level::Ok
        },
        detail: match accounts.len() {
            0 => "none yet".into(),
            n => format!("{n} configured"),
        },
        fix: accounts
            .is_empty()
            .then(|| "Accounts tab (2): press a to add, i to import".into()),
    });

    let enabled: Vec<&AccountRow> = accounts.iter().filter(|a| a.enabled).collect();
    let out_count = enabled
        .iter()
        .filter(|a| a.signed_in == SignIn::SignedOut)
        .count();
    let signed = enabled
        .iter()
        .filter(|a| a.signed_in == SignIn::SignedIn)
        .count();
    out.push(Check {
        label: "signed in",
        level: if !enabled.is_empty() && out_count == 0 {
            Level::Ok
        } else {
            Level::Missing
        },
        detail: if enabled.is_empty() {
            "no enabled account".into()
        } else {
            format!("{signed} of {} enabled", enabled.len())
        },
        fix: (out_count > 0).then(|| "Accounts tab (2): select one, press l".into()),
    });

    let installed = install::claude_registered(host);
    out.push(Check {
        label: "Claude Code",
        level: if installed { Level::Ok } else { Level::Missing },
        detail: if installed {
            "MCP server registered".into()
        } else {
            "not installed".into()
        },
        fix: (!installed).then(|| "Setup tab (4): press i, or `superorchestrator install`".into()),
    });

    let jev = ctx.config().map(|c| c.router.jev).unwrap_or_default();
    let has_key = so_router::jev::resolve_key(&jev, so_router::jev::process_env).is_some();
    out.push(Check {
        label: "router key",
        level: if has_key { Level::Ok } else { Level::Warn },
        detail: if has_key {
            format!("{} found", jev.key_env)
        } else {
            "no OpenRouter key; routing uses the heuristic".into()
        },
        fix: (!has_key).then(|| format!("export {} (optional)", jev.key_env)),
    });
    out
}

fn account_rows(
    ctx: &Ctx,
    host: &HostEnv,
    now: DateTime<Utc>,
) -> (Vec<AccountRow>, Option<String>) {
    match collect_views_with(ctx, host) {
        Ok(views) => (
            views
                .into_iter()
                .map(|v| AccountRow {
                    cooling: v.headroom.iter().any(|(_, t)| t.starts_with("cooldown")),
                    headroom: v
                        .headroom
                        .iter()
                        .map(|(p, t)| format!("{p}: {}", short_headroom(t, now)))
                        .collect::<Vec<_>>()
                        .join(", "),
                    name: v.account.name,
                    harness: v.account.harness,
                    signed_in: v.signed_in,
                    enabled: v.account.enabled,
                    cap: v.account.cap,
                    active: v.active_runs.unwrap_or(0),
                })
                .collect(),
            None,
        ),
        Err(e) => (Vec::new(), Some(format!("{e:#}"))),
    }
}

/// "cooldown until 2026-10-23T15:00:00+00:00" becomes "cooldown 3h"; other text is kept.
fn short_headroom(text: &str, now: DateTime<Utc>) -> String {
    let Some(until) = text
        .strip_prefix("cooldown until ")
        .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
    else {
        return text.to_string();
    };
    let secs = (until.with_timezone(&Utc) - now).num_seconds().max(0);
    match secs {
        0..=59 => "cooldown <1m".into(),
        60..=3599 => format!("cooldown {}m", secs / 60),
        3600..=86399 => format!("cooldown {}h", secs / 3600),
        _ => format!("cooldown {}d", secs / 86400),
    }
}

/// Reads everything the tabs show. Never writes. `sup` is the supervisor's status, asked
/// for separately because it needs a socket round trip.
pub fn load_snapshot(
    ctx: &Ctx,
    host: &HostEnv,
    sup: Option<SupInfo>,
    now: DateTime<Utc>,
) -> Snapshot {
    let (accounts, accounts_error) = account_rows(ctx, host, now);
    let (rows, active) = open_existing_store(&ctx.paths)
        .and_then(|s| recent_runs(&s).ok())
        .unwrap_or_default();
    let pending_review = rows
        .iter()
        .filter(|r| r.state == RunState::PendingReview)
        .count();
    Snapshot {
        at: now,
        config_dir: host.tildify(&ctx.paths.config_dir),
        supervisor: sup,
        checks: checklist(ctx, host, &accounts),
        accounts,
        accounts_error,
        runs: rows.iter().map(run_item).collect(),
        active_runs: active,
        pending_review,
    }
}

/// The detail pane of one run, from the store (no supervisor needed).
pub fn load_detail(ctx: &Ctx, id: &str) -> Result<RunDetail, String> {
    let store = open_existing_store(&ctx.paths).ok_or("no run history yet")?;
    let r = store
        .get_run(id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("run {id} not found"))?;
    let events = store
        .list_events_by_run(id, 2000)
        .map_err(|e| e.to_string())?;
    let route = events
        .iter()
        .rev()
        .find(|e| e.event_type == EventType::RouteDecision)
        .map(|e| route_lines(&e.data))
        .unwrap_or_else(|| match store.get_run_route(id) {
            Ok(Some(pins)) => vec![format!("pinned: {}", clean(&pins))],
            _ => vec!["not recorded".to_string()],
        });
    let skip = events.len().saturating_sub(40);
    Ok(RunDetail {
        id: r.id.clone(),
        state: r.state,
        harness: r.harness,
        account: r.account.clone(),
        model: r.model.clone(),
        worktree: r.worktree_path.clone(),
        branch: r.branch.clone(),
        error: r.error_kind.map(|k| k.to_string()),
        brief: r.prompt.as_deref().map(clean_multiline).unwrap_or_default(),
        route,
        events: events.iter().skip(skip).map(event_line).collect(),
        events_total: events.len(),
        result: r.result_json.as_deref().map(clean_multiline),
        diff: None,
    })
}

/// Like [`clean`] but keeps line breaks.
fn clean_multiline(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() && c != '\n' { ' ' } else { c })
        .collect()
}

fn route_lines(data: &serde_json::Value) -> Vec<String> {
    let get = |k: &str| data.get(k).and_then(|v| v.as_str());
    let mut out = Vec::new();
    let who: Vec<&str> = ["source", "choice_id", "account", "model"]
        .iter()
        .filter_map(|k| get(k))
        .collect();
    if !who.is_empty() {
        out.push(clean(&who.join(" / ")));
    }
    let level = get("level");
    if let Some(x) = data.get("complexity").and_then(|v| v.as_f64()) {
        out.push(clean(&match level {
            Some(l) => format!("complexity {x:.1}, level {l}"),
            None => format!("complexity {x:.1}"),
        }));
    }
    if let Some(r) = get("rationale") {
        out.push(clean(r));
    }
    if out.is_empty() {
        out.push(clean(&data.to_string()));
    }
    out
}

fn event_line(e: &so_core::Event) -> String {
    let detail = ["name", "excerpt", "step", "status", "reason", "to", "path"]
        .iter()
        .find_map(|k| e.data.get(k).and_then(|v| v.as_str()))
        .unwrap_or("");
    clean(
        format!(
            "{} {} {}",
            e.ts.with_timezone(&chrono::Local).format("%H:%M:%S"),
            e.event_type,
            detail
        )
        .trim_end(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_characters_never_reach_the_screen() {
        assert_eq!(clean("a\x1b[31mred\x07\n"), "a [31mred  ");
        assert_eq!(clean_multiline("a\x1b[0m\nb"), "a [0m\nb");
    }

    #[test]
    fn cooldowns_are_shown_as_time_left() {
        let now: DateTime<Utc> = "2026-10-23T12:00:00Z".parse().unwrap();
        assert_eq!(
            short_headroom("cooldown until 2026-10-23T15:00:00+00:00", now),
            "cooldown 3h"
        );
        assert_eq!(
            short_headroom("cooldown until 2026-10-23T12:20:00Z", now),
            "cooldown 20m"
        );
        assert_eq!(short_headroom("ok 82%", now), "ok 82%");
    }

    #[test]
    fn route_lines_prefer_who_and_why() {
        let v = serde_json::json!({"source":"jev","choice_id":"codex-fast","account":"codex-1","model":"gpt","rationale":"small edit"});
        assert_eq!(
            route_lines(&v),
            vec!["jev / codex-fast / codex-1 / gpt", "small edit"]
        );
        let v =
            serde_json::json!({"source":"jev","choice_id":"c","complexity":55.04,"level":"mid"});
        assert_eq!(
            route_lines(&v),
            vec!["jev / c", "complexity 55.0, level mid"]
        );
        assert_eq!(route_lines(&serde_json::json!({"x":1})), vec![r#"{"x":1}"#]);
    }
}

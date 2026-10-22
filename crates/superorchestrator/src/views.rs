//! Account views and store helpers shared by `account`, `status` and `doctor`.

use std::io::{self, Write};

use anyhow::Result;
use chrono::Utc;
use serde_json::{Value, json};
use so_accounts::SignIn;
use so_core::{AccountFile, Paths, SessionKind};
use so_store::{SessionFilter, Store};

use crate::ctx::Ctx;
use crate::output::write_table;

/// One account as shown by `account list` and `status`.
pub struct AccountView {
    pub account: AccountFile,
    pub signed_in: SignIn,
    pub active_runs: Option<i64>,
    pub headroom: Vec<(String, String)>,
}

pub fn sign_in_text(s: SignIn) -> &'static str {
    match s {
        SignIn::SignedIn => "yes",
        SignIn::SignedOut => "no",
        SignIn::Unknown => "unknown",
    }
}

/// Opens `state.db` only when it already exists.
pub fn open_existing_store(paths: &Paths) -> Option<Store> {
    paths
        .state_db()
        .exists()
        .then(|| Store::open(paths.state_db()).ok())
        .flatten()
}

pub fn collect_views(ctx: &Ctx) -> Result<Vec<AccountView>> {
    let host = ctx.host()?;
    let store = open_existing_store(&ctx.paths);
    let now = Utc::now();
    let mut out = Vec::new();
    for account in AccountFile::list(&ctx.paths)? {
        let signed_in = so_accounts::sign_in_state(&account, &host)?;
        let active_runs = store
            .as_ref()
            .and_then(|s| s.count_active_runs_by_account(&account.name).ok());
        let headroom = account
            .pools
            .iter()
            .map(|pool| {
                let flag = store
                    .as_ref()
                    .and_then(|s| s.get_flag(&account.name, pool).ok().flatten())
                    .filter(|f| f.until > now);
                let text = if let Some(f) = flag {
                    format!("cooldown until {}", f.until.to_rfc3339())
                } else {
                    match store
                        .as_ref()
                        .and_then(|s| s.get_headroom(&account.name, pool).ok().flatten())
                    {
                        Some(h) => match h.five_hour_remaining_pct {
                            Some(p) => format!("{} {p:.0}%", h.status.as_str()),
                            None => h.status.as_str().to_string(),
                        },
                        None => "unknown".to_string(),
                    }
                };
                (pool.clone(), text)
            })
            .collect();
        out.push(AccountView {
            account,
            signed_in,
            active_runs,
            headroom,
        });
    }
    Ok(out)
}

pub fn views_json(views: &[AccountView]) -> Value {
    Value::Array(
        views
            .iter()
            .map(|v| {
                json!({
                    "name": v.account.name,
                    "harness": v.account.harness.as_str(),
                    "signed_in": sign_in_text(v.signed_in),
                    "enabled": v.account.enabled,
                    "cap": v.account.cap,
                    "active_runs": v.active_runs.unwrap_or(0),
                    "headroom": v.headroom.iter()
                        .map(|(p, t)| (p.clone(), Value::String(t.clone())))
                        .collect::<serde_json::Map<_, _>>(),
                })
            })
            .collect(),
    )
}

pub fn write_views_table(w: &mut dyn Write, views: &[AccountView]) -> io::Result<()> {
    if views.is_empty() {
        return writeln!(
            w,
            "no accounts; add one with `superorchestrator account add`"
        );
    }
    let rows: Vec<Vec<String>> = views
        .iter()
        .map(|v| {
            vec![
                v.account.name.clone(),
                v.account.harness.as_str().to_string(),
                sign_in_text(v.signed_in).to_string(),
                if v.account.enabled { "yes" } else { "no" }.to_string(),
                v.account.cap.to_string(),
                v.active_runs.unwrap_or(0).to_string(),
                v.headroom
                    .iter()
                    .map(|(p, t)| format!("{p}: {t}"))
                    .collect::<Vec<_>>()
                    .join(", "),
            ]
        })
        .collect();
    write_table(
        w,
        &[
            "NAME",
            "HARNESS",
            "SIGNED IN",
            "ENABLED",
            "CAP",
            "ACTIVE",
            "HEADROOM",
        ],
        &rows,
    )
}

fn pid_alive(pid: i64) -> bool {
    let Some(p) = i32::try_from(pid)
        .ok()
        .and_then(rustix::process::Pid::from_raw)
    else {
        return false;
    };
    match rustix::process::test_kill_process(p) {
        Ok(()) => true,
        Err(e) => e != rustix::io::Errno::SRCH,
    }
}

/// Ends active interactive sessions whose process is gone (`exec` replaces the
/// wrapper process, so nothing else records the end). Returns how many ended.
pub fn end_dead_interactive_sessions(store: &Store) -> usize {
    let Ok(active) = store.list_sessions(SessionFilter::Active, 10_000) else {
        return 0;
    };
    let now = Utc::now();
    active
        .iter()
        .filter(|s| s.kind == SessionKind::Interactive)
        .filter(|s| s.pid.is_some_and(|p| !pid_alive(p)))
        .filter(|s| store.end_session(&s.id, "ended", now).is_ok())
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use so_core::new_id;
    use so_store::SessionRow;

    #[test]
    fn dead_interactive_sessions_are_ended_and_live_ones_kept() {
        let store = Store::open_in_memory().unwrap();
        let mk = |pid: i64, kind: SessionKind| SessionRow {
            id: new_id(),
            kind,
            client: "wrapper".into(),
            cwd: None,
            account: Some("a".into()),
            pid: Some(pid),
            started_at: Utc::now(),
            ended_at: None,
            status: "active".into(),
        };
        let live = mk(i64::from(std::process::id()), SessionKind::Interactive);
        let dead = mk(i64::from(i32::MAX), SessionKind::Interactive);
        let orch = mk(i64::from(i32::MAX), SessionKind::Orchestrator);
        for s in [&live, &dead, &orch] {
            store.insert_session(s).unwrap();
        }
        assert_eq!(end_dead_interactive_sessions(&store), 1);
        assert!(
            store
                .get_session(&dead.id)
                .unwrap()
                .unwrap()
                .ended_at
                .is_some()
        );
        assert!(
            store
                .get_session(&live.id)
                .unwrap()
                .unwrap()
                .ended_at
                .is_none()
        );
        assert!(
            store
                .get_session(&orch.id)
                .unwrap()
                .unwrap()
                .ended_at
                .is_none()
        );
    }
}

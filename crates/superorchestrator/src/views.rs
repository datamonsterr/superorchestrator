//! Account views and store helpers shared by `account`, `status` and `doctor`.

use std::io::{self, Write};

use anyhow::Result;
use chrono::Utc;
use serde_json::{Value, json};
use so_accounts::SignIn;
use so_core::{AccountFile, Paths};
use so_store::Store;

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

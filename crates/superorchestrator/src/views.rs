//! Account views and store helpers shared by `account`, `status` and `doctor`.

use std::io::{self, Write};

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use so_accounts::SignIn;
use so_core::{AccountFile, Headroom, Paths};
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
    let refresh = || -> Result<Vec<String>> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let refresher = usage_refresher(ctx, &host)?;
        Ok(rt.block_on(refresher.refresh(&ctx.paths)))
    };
    match refresh() {
        Ok(errors) => {
            for error in errors {
                eprintln!("warning: {error}");
            }
        }
        Err(e) => eprintln!("warning: could not refresh usage: {e}"),
    }
    collect_views_with(ctx, &host)
}

/// Uses the same probes with or without a running supervisor. Store timestamps enforce
/// the TTL across CLI invocations; the long-lived UI also caches failed attempts.
pub fn usage_refresher(
    ctx: &Ctx,
    host: &so_accounts::HostEnv,
) -> Result<std::sync::Arc<so_usage::UsageRefresher>> {
    Ok(so_usage::active_refresher(
        Store::open(ctx.paths.state_db())?,
        ctx.config()?,
        host.clone(),
    ))
}

/// The `HEADROOM` cell for one pool. An active flag wins. Otherwise the stored record is read
/// as it stands at `now` (see `so_usage::effective_with`), so stale rows do not show old numbers.
fn headroom_text(
    flag_until: Option<DateTime<Utc>>,
    stored: Option<&Headroom>,
    low_pct: f64,
    now: DateTime<Utc>,
) -> String {
    if let Some(until) = flag_until.filter(|until| *until > now) {
        return format!("cooldown until {}", until.to_rfc3339());
    }
    match stored {
        Some(h) => {
            let h = so_usage::effective_with(h, low_pct, false, now);
            match (h.five_hour_remaining_pct, h.weekly_remaining_pct) {
                (Some(five), Some(week)) => {
                    format!("{} 5h {five:.0}% wk {week:.0}%", h.status.as_str())
                }
                (Some(p), None) => format!("{} {p:.0}%", h.status.as_str()),
                (None, Some(p)) => format!("{} wk {p:.0}%", h.status.as_str()),
                (None, None) => h.status.as_str().to_string(),
            }
        }
        None => "unknown".to_string(),
    }
}

/// [`collect_views`] against a given host (the terminal UI holds one; tests pass a fake).
pub fn collect_views_with(ctx: &Ctx, host: &so_accounts::HostEnv) -> Result<Vec<AccountView>> {
    let store = open_existing_store(&ctx.paths);
    let now = Utc::now();
    let low_pct = ctx
        .config()
        .map_or(so_usage::headroom::DEFAULT_LOW_PCT, |c| {
            f64::from(c.usage.low_pct)
        });
    let mut out = Vec::new();
    for account in AccountFile::list(&ctx.paths)? {
        let signed_in = so_accounts::sign_in_state(&account, host)?;
        let active_runs = store
            .as_ref()
            .and_then(|s| s.count_active_runs_by_account(&account.name).ok());
        let headroom = account
            .pools
            .iter()
            .map(|pool| {
                let flag_until = store
                    .as_ref()
                    .and_then(|s| s.get_flag(&account.name, pool).ok().flatten())
                    .map(|f| f.until);
                let stored = store
                    .as_ref()
                    .and_then(|s| s.get_headroom(&account.name, pool).ok().flatten());
                let text = headroom_text(flag_until, stored.as_ref(), low_pct, now);
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

#[cfg(test)]
mod tests {
    use chrono::Duration;
    use so_core::{HeadroomSource, HeadroomStatus};

    use super::*;

    fn record(
        status: HeadroomStatus,
        five_hour: Option<(f64, DateTime<Utc>)>,
        observed_at: DateTime<Utc>,
    ) -> Headroom {
        Headroom {
            account: "acct".into(),
            pool: "default".into(),
            five_hour_remaining_pct: five_hour.map(|(p, _)| p),
            five_hour_resets_at: five_hour.map(|(_, r)| r),
            weekly_remaining_pct: None,
            weekly_resets_at: None,
            status,
            source: HeadroomSource::Ledger,
            observed_at,
        }
    }

    #[test]
    fn stale_exhausted_row_without_windows_reads_unknown() {
        let now = Utc::now();
        let stored = record(HeadroomStatus::Exhausted, None, DateTime::UNIX_EPOCH);
        assert_eq!(headroom_text(None, Some(&stored), 15.0, now), "unknown");
    }

    #[test]
    fn future_reset_keeps_stored_percentage() {
        let now = Utc::now();
        let stored = record(
            HeadroomStatus::Ok,
            Some((50.0, now + Duration::hours(1))),
            now,
        );
        assert_eq!(headroom_text(None, Some(&stored), 15.0, now), "ok 50%");
    }

    #[test]
    fn passed_reset_reads_as_fully_reset() {
        let now = Utc::now();
        let stored = record(
            HeadroomStatus::Low,
            Some((10.0, now - Duration::hours(1))),
            now - Duration::hours(2),
        );
        assert_eq!(headroom_text(None, Some(&stored), 15.0, now), "ok 100%");
    }

    #[test]
    fn active_flag_prints_cooldown() {
        let now = Utc::now();
        let until = now + Duration::minutes(5);
        let stored = record(HeadroomStatus::Ok, Some((50.0, now)), now);
        assert_eq!(
            headroom_text(Some(until), Some(&stored), 15.0, now),
            format!("cooldown until {}", until.to_rfc3339())
        );
    }

    #[test]
    fn expired_flag_is_ignored() {
        let now = Utc::now();
        let stored = record(
            HeadroomStatus::Ok,
            Some((50.0, now + Duration::hours(1))),
            now,
        );
        let expired = Some(now - Duration::minutes(1));
        assert_eq!(headroom_text(expired, Some(&stored), 15.0, now), "ok 50%");
    }

    #[test]
    fn missing_row_reads_unknown() {
        assert_eq!(headroom_text(None, None, 15.0, Utc::now()), "unknown");
    }
}

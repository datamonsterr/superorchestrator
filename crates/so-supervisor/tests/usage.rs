#![allow(clippy::unwrap_used)]

mod common;

use common::Env;
use serde_json::json;
use so_core::proto::{HarnessesView, METHOD_LIST_HARNESSES};
use so_core::{AccountFile, Harness, HeadroomSource};
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

#[tokio::test]
async fn supervisor_refreshes_idle_accounts_and_new_accounts_for_the_board() {
    let mut e = Env::new();
    e.config.usage.probe_ttl = so_core::HumanDuration::from_secs(1);
    let binary = e.dir.path().join("usage.sh");
    std::fs::write(&binary, "#!/bin/sh\nprintf 'Gemini Models\\tFive Hour Limit Remaining\\t73%%\\t\\nClaude and GPT models\\tWeekly Limit Remaining\\t29%%\\t\\n'\n").unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
    let add = |name: &str| {
        let home = e.home.join(name);
        std::fs::create_dir_all(home.join(".gemini")).unwrap();
        std::fs::write(home.join(".gemini/oauth_creds.json"), "never print").unwrap();
        AccountFile::new(
            name,
            Harness::Agy,
            home.to_str().unwrap(),
            binary.to_str().unwrap(),
        )
        .unwrap()
        .save(&e.paths)
        .unwrap();
    };
    add("agy-1");
    let h = e.start().await;
    let client = e.inspect_client(&h).await;
    let client = &client;
    let read = |count| async move {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let v: HarnessesView = client.call(METHOD_LIST_HARNESSES, json!({})).await.unwrap();
                if v.accounts.len() == count
                    && v.accounts
                        .iter()
                        .all(|a| a.headroom.iter().all(|h| h.source == HeadroomSource::Probe))
                {
                    return v;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap()
    };
    let v = read(1).await;
    assert_eq!(
        v.accounts[0].headroom[0].five_hour_remaining_pct,
        Some(73.0)
    );
    assert_eq!(v.accounts[0].headroom[1].weekly_remaining_pct, Some(29.0));
    add("agy-2");
    assert_eq!(read(2).await.accounts.len(), 2);
    h.shutdown().await;
}

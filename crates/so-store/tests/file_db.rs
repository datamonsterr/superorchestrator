#![allow(clippy::unwrap_used)]

use std::thread;
use std::time::Instant;

use chrono::{Duration, Utc};
use so_core::*;
use so_store::*;

fn session() -> SessionRow {
    SessionRow {
        id: new_id(),
        kind: SessionKind::Orchestrator,
        client: "c".into(),
        cwd: None,
        account: None,
        pid: Some(7),
        started_at: Utc::now(),
        ended_at: None,
        status: "active".into(),
    }
}

fn run(session_id: &str) -> RunRow {
    RunRow {
        id: new_id(),
        session_id: session_id.into(),
        parent_run_id: None,
        depth: 0,
        name: None,
        agent: None,
        choice_id: None,
        harness: Harness::Claude,
        account: Some("acc".into()),
        model: None,
        effort: None,
        cwd: None,
        worktree_path: None,
        branch: None,
        native_session_id: None,
        state: RunState::Queued,
        review: false,
        output_schema: None,
        result_json: None,
        error_kind: None,
        prompt: None,
        isolation: so_core::Isolation::None,
        max_turns: None,
        worktree_base: None,
        worktree_repo: None,
        timeout_ms: None,
        created_at: Utc::now(),
        started_at: None,
        finished_at: None,
    }
}

fn event(session: &str, i: i64) -> Event {
    Event {
        id: new_id(),
        session_id: session.into(),
        run_id: None,
        ts: Utc::now() + Duration::microseconds(i),
        event_type: EventType::Progress,
        data: serde_json::json!({"i": i}),
    }
}

#[test]
fn file_db_query_groups() {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(dir.path().join("state.db")).unwrap();
    let sess = session();
    s.insert_session(&sess).unwrap();
    assert_eq!(
        s.find_active_session_by_pid(7).unwrap().unwrap().id,
        sess.id
    );
    let r = run(&sess.id);
    s.insert_run(&r).unwrap();
    s.update_run_state(&r.id, RunState::Queued, RunState::Starting)
        .unwrap();
    assert!(matches!(
        s.update_run_state(&r.id, RunState::Queued, RunState::Starting),
        Err(StoreError::StateConflict { .. })
    ));
    assert_eq!(s.count_active_runs_total().unwrap(), 1);
    s.append_event(&event(&sess.id, 0)).unwrap();
    assert_eq!(
        s.list_events_by_session(&sess.id, None, 10).unwrap().len(),
        1
    );
    let m = MessageRow {
        id: new_id(),
        session_id: sess.id.clone(),
        from_run: Some(r.id.clone()),
        to_run: None,
        kind: MessageKind::Progress,
        body: "b".into(),
        created_at: Utc::now(),
        read_at: None,
    };
    s.insert_message(&m).unwrap();
    assert_eq!(s.unread_messages(&sess.id, None, 10).unwrap().len(), 1);
    s.mark_messages_read(&[m.id], Utc::now()).unwrap();
    assert!(s.unread_messages(&sess.id, None, 10).unwrap().is_empty());
    s.insert_ledger(&LedgerEntry {
        account: "acc".into(),
        pool: "p".into(),
        run_id: Some(r.id.clone()),
        ts: Utc::now(),
        input_tokens: 3,
        output_tokens: 4,
        cost: 0.1,
    })
    .unwrap();
    let sums = s
        .ledger_sums("acc", "p", Utc::now() - Duration::hours(1))
        .unwrap();
    assert_eq!(
        (sums.runs, sums.input_tokens, sums.output_tokens),
        (1, 3, 4)
    );
    s.set_flag(&FlagRow {
        account: "acc".into(),
        pool: "p".into(),
        until: Utc::now() + Duration::hours(1),
        reason: "q".into(),
    })
    .unwrap();
    assert_eq!(s.active_flags(Utc::now(), 10).unwrap().len(), 1);
}

#[test]
fn migrations_are_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let a = Store::open(&path).unwrap();
    let sess = session();
    a.insert_session(&sess).unwrap();
    drop(a);
    let b = Store::open(&path).unwrap();
    assert_eq!(b.schema_version().unwrap(), latest_version());
    assert!(b.get_session(&sess.id).unwrap().is_some());
}

#[test]
fn concurrent_writers_do_not_hit_busy() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let a = Store::open(&path).unwrap();
    let b = Store::open(&path).unwrap();
    let sess = session();
    a.insert_session(&sess).unwrap();
    let handles: Vec<_> = [a.clone(), b.clone()]
        .into_iter()
        .enumerate()
        .map(|(n, st)| {
            let sid = sess.id.clone();
            thread::spawn(move || {
                for i in 0..300 {
                    st.append_event(&event(&sid, (n * 1000 + i) as i64))
                        .unwrap();
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    assert_eq!(
        a.list_events_by_session(&sess.id, None, 10_000)
            .unwrap()
            .len(),
        600
    );
}

#[test]
fn ten_thousand_events_list_fast() {
    let s = Store::open_in_memory().unwrap();
    let sid = new_id();
    for i in 0..10_000 {
        s.append_event(&event(&sid, i)).unwrap();
    }
    let t = Instant::now();
    let v = s.list_events_by_session(&sid, None, 20_000).unwrap();
    let el = t.elapsed();
    println!("list_events_by_session 10k: {el:?}");
    assert_eq!(v.len(), 10_000);
    assert!(el.as_secs_f64() < 10.0, "too slow: {el:?}");
}

#[test]
fn schema_version_guards() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    drop(Store::open(&path).unwrap());
    let set = |v: i64| {
        let c = rusqlite::Connection::open(&path).unwrap();
        c.pragma_update(None, "user_version", v).unwrap();
    };
    set(latest_version() + 5);
    assert!(matches!(
        Store::open(&path),
        Err(StoreError::SchemaTooNew { .. })
    ));
    set(-1);
    assert!(matches!(
        Store::open(&path),
        Err(StoreError::InvalidSchemaVersion(-1))
    ));
}

#[cfg(unix)]
#[test]
fn new_db_file_is_private() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    drop(Store::open(&path).unwrap());
    let mode = std::fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[cfg(unix)]
#[test]
fn wide_existing_db_file_is_tightened() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    drop(Store::open(&path).unwrap());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    drop(Store::open(&path).unwrap());
    let mode = std::fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
}

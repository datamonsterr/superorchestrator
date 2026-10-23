//! Embedded schema migrations, versioned with `PRAGMA user_version`.

use rusqlite::{Connection, TransactionBehavior};

use crate::error::{Result, StoreError};

const V1: &str = "
CREATE TABLE schema_versions (table_name TEXT PRIMARY KEY, version INTEGER NOT NULL);

CREATE TABLE sessions (
  id TEXT PRIMARY KEY, kind TEXT NOT NULL, client TEXT NOT NULL, cwd TEXT, account TEXT,
  pid INTEGER, started_at TEXT NOT NULL, ended_at TEXT, status TEXT NOT NULL
);

CREATE TABLE runs (
  id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL REFERENCES sessions(id),
  parent_run_id TEXT, depth INTEGER NOT NULL DEFAULT 0, name TEXT, agent TEXT, choice_id TEXT,
  harness TEXT NOT NULL, account TEXT, model TEXT, effort TEXT, cwd TEXT, worktree_path TEXT,
  branch TEXT, native_session_id TEXT, state TEXT NOT NULL, review INTEGER NOT NULL DEFAULT 0,
  output_schema TEXT, result_json TEXT, error_kind TEXT, prompt TEXT,
  created_at TEXT NOT NULL, started_at TEXT, finished_at TEXT,
  isolation TEXT NOT NULL DEFAULT 'none', max_turns INTEGER, worktree_base TEXT,
  worktree_repo TEXT, timeout_ms INTEGER
);
CREATE INDEX idx_runs_session ON runs(session_id);
CREATE INDEX idx_runs_state ON runs(state);
CREATE INDEX idx_runs_parent ON runs(parent_run_id);
CREATE INDEX idx_runs_account_state ON runs(account, state);
CREATE INDEX idx_runs_state_created ON runs(state, created_at);

CREATE TABLE attempts (
  id TEXT PRIMARY KEY,
  run_id TEXT NOT NULL REFERENCES runs(id),
  account TEXT NOT NULL, pool TEXT, started_at TEXT NOT NULL, ended_at TEXT, outcome TEXT
);
CREATE INDEX idx_attempts_run ON attempts(run_id);

CREATE TABLE events (
  id TEXT PRIMARY KEY, session_id TEXT NOT NULL, run_id TEXT, ts TEXT NOT NULL,
  type TEXT NOT NULL, data_json TEXT NOT NULL
);
CREATE INDEX idx_events_session_ts ON events(session_id, ts, id);
CREATE INDEX idx_events_run_ts ON events(run_id, ts, id);
CREATE INDEX idx_events_ts ON events(ts);

CREATE TABLE messages (
  id TEXT PRIMARY KEY, session_id TEXT NOT NULL, from_run TEXT, to_run TEXT,
  kind TEXT NOT NULL, body TEXT NOT NULL, created_at TEXT NOT NULL, read_at TEXT
);
CREATE INDEX idx_messages_to_run_read ON messages(to_run, read_at);

CREATE TABLE headroom (
  account TEXT NOT NULL, pool TEXT NOT NULL,
  five_hour_remaining_pct REAL, five_hour_resets_at TEXT,
  weekly_remaining_pct REAL, weekly_resets_at TEXT,
  status TEXT NOT NULL, source TEXT NOT NULL, observed_at TEXT NOT NULL,
  PRIMARY KEY (account, pool)
);

CREATE TABLE ledger (
  id INTEGER PRIMARY KEY AUTOINCREMENT, account TEXT NOT NULL, pool TEXT NOT NULL, run_id TEXT,
  ts TEXT NOT NULL, input_tokens INTEGER NOT NULL DEFAULT 0,
  output_tokens INTEGER NOT NULL DEFAULT 0, cost REAL NOT NULL DEFAULT 0
);
CREATE INDEX idx_ledger_account_pool_ts ON ledger(account, pool, ts);

CREATE TABLE flags (
  account TEXT NOT NULL, pool TEXT NOT NULL, until TEXT NOT NULL, reason TEXT NOT NULL,
  PRIMARY KEY (account, pool)
);

CREATE TABLE workflow_cache (
  key_hash TEXT PRIMARY KEY, workflow TEXT NOT NULL, result_json TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE TABLE workflow_runs (
  id TEXT PRIMARY KEY, name TEXT NOT NULL, session_id TEXT, state TEXT NOT NULL,
  args_json TEXT NOT NULL, started_at TEXT NOT NULL, finished_at TEXT
);

CREATE TABLE workflow_steps (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  workflow_run_id TEXT NOT NULL REFERENCES workflow_runs(id) ON DELETE CASCADE,
  step_index INTEGER NOT NULL, phase TEXT, prompt_hash TEXT NOT NULL, run_id TEXT,
  state TEXT NOT NULL, started_at TEXT NOT NULL, finished_at TEXT,
  UNIQUE (workflow_run_id, step_index)
);

INSERT INTO schema_versions (table_name, version) VALUES
  ('sessions',1),('runs',1),('attempts',1),('events',1),('messages',1),('headroom',1),
  ('ledger',1),('flags',1),('workflow_cache',1),('workflow_runs',1),('workflow_steps',1);
";

const V2: &str = "
-- Per-run extras. No foreign key: the row is written before the run is inserted.
CREATE TABLE run_meta (
  run_id TEXT PRIMARY KEY, checks_json TEXT NOT NULL DEFAULT '[]', fork_from TEXT
);
UPDATE schema_versions SET version = 2 WHERE table_name = 'runs';
";

const V3: &str = "
-- A finished run that was sent a message but found no free slot goes back to `queued`
-- with the text it is to be resumed with.
ALTER TABLE run_meta ADD COLUMN resume_prompt TEXT;
";

const V4: &str = "
-- How a run was routed (the caller's pins), kept so a failover can route again within them.
ALTER TABLE run_meta ADD COLUMN route_json TEXT;
";

const V5: &str = "
-- A workflow run keeps its result (or why it failed) so `workflow_status` can report it later.
ALTER TABLE workflow_runs ADD COLUMN result_json TEXT;
ALTER TABLE workflow_runs ADD COLUMN error TEXT;
";

const V6: &str = "
-- The prompt of a run's current turn (start, resume, review feedback), so a failover
-- re-sends that and not the first prompt; and a failover that is waiting for a free slot.
ALTER TABLE run_meta ADD COLUMN turn_prompt TEXT;
ALTER TABLE run_meta ADD COLUMN failover_json TEXT;
";

const V7: &str = "
-- A queued run moved to another account: its route_decision is announced once it starts.
ALTER TABLE run_meta ADD COLUMN queued_route_json TEXT;
";

const V8: &str = "
-- When the process behind a session started, so a reused pid is not mistaken for it.
ALTER TABLE sessions ADD COLUMN pid_start TEXT;
";

const V9: &str = "
-- Per-run RPC identity proof. NULL on legacy runs deliberately refuses run claims.
ALTER TABLE run_meta ADD COLUMN rpc_token_hash BLOB;
ALTER TABLE workflow_runs ADD COLUMN owner_run_id TEXT;
-- schema_versions is deliberately not touched: V3-V8 also only added columns without
-- bumping it, and only PRAGMA user_version tracks the schema.
";

/// Ordered migrations; entry `i` brings the database to version `i + 1`.
const MIGRATIONS: &[&str] = &[V1, V2, V3, V4, V5, V6, V7, V8, V9];

/// Latest schema version known to this build.
pub fn latest_version() -> i64 {
    MIGRATIONS.len() as i64
}

/// Applies pending migrations inside one immediate transaction (safe under concurrent opens).
pub fn migrate(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current: i64 = tx.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if current < 0 {
        return Err(StoreError::InvalidSchemaVersion(current));
    }
    if current > latest_version() {
        return Err(StoreError::SchemaTooNew {
            found: current,
            supported: latest_version(),
        });
    }
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
        tx.execute_batch(sql)?;
        tx.execute_batch(&format!("PRAGMA user_version = {}", i + 1))?;
    }
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn run_identity_migrates_from_v8_without_authenticating_old_runs() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        for sql in super::MIGRATIONS.iter().take(8) {
            conn.execute_batch(sql).unwrap();
        }
        conn.execute_batch(
            "PRAGMA user_version = 8; INSERT INTO run_meta (run_id) VALUES ('legacy');",
        )
        .unwrap();
        assert_eq!(
            conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                .unwrap(),
            8
        );
        super::migrate(&mut conn).unwrap();
        assert_eq!(
            conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                .unwrap(),
            super::latest_version()
        );
        let hash: Option<Vec<u8>> = conn
            .query_row(
                "SELECT rpc_token_hash FROM run_meta WHERE run_id = 'legacy'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(hash.is_none());
    }
}

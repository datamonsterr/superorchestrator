//! The `Store` handle: one connection behind a mutex.

use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use rusqlite::Connection;

use crate::error::Result;
use crate::migrations;

/// Handle to the SQLite database. Cheap to clone; clones share one connection.
///
/// Each process opens its own `Store`; WAL mode lets several processes share the file.
#[derive(Clone)]
pub struct Store {
    conn: Arc<Mutex<Connection>>,
}

impl Store {
    /// Opens (creating if needed) the database at `path` and migrates it.
    pub fn open(path: impl AsRef<Path>) -> Result<Store> {
        let mut conn = Connection::open(path)?;
        // Set first so the WAL switch and migrations wait for other writers.
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA foreign_keys = ON;",
        )?;
        migrations::migrate(&mut conn)?;
        Ok(Store::from_conn(conn))
    }

    /// Opens a private in-memory database (for tests).
    pub fn open_in_memory() -> Result<Store> {
        let mut conn = Connection::open_in_memory()?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.execute_batch("PRAGMA synchronous = NORMAL; PRAGMA foreign_keys = ON;")?;
        migrations::migrate(&mut conn)?;
        Ok(Store::from_conn(conn))
    }

    fn from_conn(conn: Connection) -> Store {
        Store {
            conn: Arc::new(Mutex::new(conn)),
        }
    }

    pub(crate) fn conn(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Current `PRAGMA user_version`.
    pub fn schema_version(&self) -> Result<i64> {
        Ok(self
            .conn()
            .query_row("PRAGMA user_version", [], |r| r.get(0))?)
    }
}

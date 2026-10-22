//! SQLite system of record: schema, migrations and typed queries (ADR-0002).
#![cfg_attr(test, allow(clippy::unwrap_used))]

mod codec;
mod error;
mod events;
mod messages;
mod migrations;
mod rows;
mod runs;
mod sessions;
mod store;
mod usage;
mod workflows;

pub use error::{Result, StoreError};
pub use migrations::latest_version;
pub use rows::*;
pub use store::Store;

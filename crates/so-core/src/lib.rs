//! Config loading, paths, glossary types, protocol types and errors.
#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod account;
pub mod buildinfo;
pub mod child_env;
pub mod config;
pub mod duration;
pub mod error;
pub mod fsutil;
pub mod paths;
pub mod procinfo;
pub mod proto;
pub mod redact;
pub mod types;

pub use account::{AccountFile, validate_account_name};
pub use config::Config;
pub use duration::HumanDuration;
pub use error::{CoreError, Result};
pub use paths::{Paths, Platform, expand_tilde};
pub use types::*;

/// Maximum time a single agent wait may block; the run continues after this limit.
pub const MAX_WAIT: std::time::Duration = std::time::Duration::from_secs(30 * 60);

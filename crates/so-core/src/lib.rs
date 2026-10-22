//! Config loading, paths, glossary types, protocol types and errors.
#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod account;
pub mod config;
pub mod duration;
pub mod error;
mod fsutil;
pub mod paths;
pub mod proto;
pub mod types;

pub use account::{AccountFile, validate_account_name};
pub use config::Config;
pub use duration::HumanDuration;
pub use error::{CoreError, Result};
pub use paths::{Paths, Platform, expand_tilde};
pub use types::*;

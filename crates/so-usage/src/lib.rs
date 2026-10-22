//! Probes, passive capture, ledger and headroom (SPEC 6).
#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod error;
pub mod exhaustion;
pub mod headroom;
pub mod ledger;
pub mod parse;
pub mod probe;

pub use error::{Result, UsageError};
pub use exhaustion::{is_available, mark_exhausted};
pub use headroom::{compute_status, merge_into_record};
pub use ledger::{LedgerWindows, ledger_headroom};
pub use parse::{
    Window, WindowReading, parse_agy_usage, parse_claude_rate_limit_event, parse_codex_rate_limits,
};
pub use probe::{
    BoxFuture, ProbeRequest, ProbeRunner, ResponseMatcher, TokioProbeRunner, UsageService,
};

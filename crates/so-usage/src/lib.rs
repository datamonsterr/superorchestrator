//! Probes, passive capture, ledger and headroom (SPEC 6).
#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod availability;
pub mod error;
pub mod exhaustion;
pub mod headroom;
pub mod ledger;
pub mod parse;
pub mod probe;

pub use availability::{Availability, availability};
pub use error::{Result, UsageError};
pub use exhaustion::{is_available, mark_exhausted};
pub use headroom::{
    compute_status, effective, effective_with, headroom_score, merge_into_record, merge_partial,
};
pub use ledger::{LedgerWindows, ledger_headroom};
pub use parse::{
    Window, WindowReading, parse_agy_usage, parse_claude_rate_limit_event, parse_codex_rate_limits,
};
pub use probe::{
    BoxFuture, ProbeOutput, ProbeRequest, ProbeRunner, ResponseMatcher, TokioProbeRunner,
    UsageService,
};

//! One driver per harness behind a common, harness-neutral API.
#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod claude;
pub mod driver;
pub mod transcript;

pub use claude::ClaudeDriver;
pub use driver::{
    AnyDriver, Driver, DriverError, HarnessEvent, JoinInfo, LaunchSpec, McpServerSpec,
    ProcessGuard, RateLimits, RunControl, RunHandle, classify_common,
};
pub use transcript::{Dir, TranscriptWriter};

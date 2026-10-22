//! One driver per harness behind a common, harness-neutral API.
#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod agy;
pub mod claude;
pub mod driver;
pub mod opencode;
pub mod process;
pub mod transcript;

pub use agy::{AgyControl, AgyDriver};
pub use claude::{ClaudeControl, ClaudeDriver};
pub use driver::{
    AnyDriver, Channel, Driver, DriverConfig, DriverError, Endpoint, HarnessEvent, JoinCommand,
    JoinInfo, LaunchSpec, McpServerSpec, RateLimits, RunControl, RunHandle, ShutdownOutcome,
    classify_common,
};
pub use opencode::{EventMapper, OpencodeControl, OpencodeDriver};
pub use process::{ProcessGuard, ProcessHandle, Supervised, spawn_supervised};
pub use transcript::{Dir, TranscriptWriter};

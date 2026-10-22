//! Custom agents (SPEC 10): `AGENT.md` parsing, discovery, per-harness
//! materialisation and the starter agents.
#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod discover;
mod error;
pub mod materialise;
pub mod parse;
pub mod starter;

pub use discover::{
    Discovery, discover, read_source_of, remove_files, resolve_for_spawn, search_dirs,
};
pub use error::{AgentError, Result};
pub use materialise::{Materialised, McpExtra, gaps, materialise};
pub use parse::MAX_AGENT_FILE;
pub use parse::{
    AgentDef, AgentSource, HarnessPref, McpRef, Memory, parse_agent, validate_agent_name,
};
pub use starter::{install_starters, starter_names};

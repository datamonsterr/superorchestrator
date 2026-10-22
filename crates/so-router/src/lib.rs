//! Catalog, choices, scorer, Jev decider, account selection and failover.
#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod catalog;
pub mod eligible;
pub mod error;
pub mod eval;
pub mod heuristic;
mod http;
pub mod jev;
pub mod route;
pub mod select;

pub use catalog::{Catalog, Cost, Tier};
pub use eligible::{AccountState, Candidate, PoolState, Snapshot};
pub use error::{Result, RouterError};
pub use route::{
    RouteCtx, RouteInputs, RouteOutcome, config_fingerprint, route, route_detailed, route_with,
};

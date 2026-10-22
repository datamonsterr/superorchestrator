//! Method handlers registered on the supervisor's router.

mod harnesses;

use std::sync::Arc;

use crate::server::SupervisorState;

pub use harnesses::{build_harnesses_view, register_harnesses};

/// Registers every handler that ships with the supervisor. The CLI daemon and the tests
/// both start from this; later tasks add their methods here.
pub fn default_handlers(state: &Arc<SupervisorState>) {
    register_harnesses(state);
}

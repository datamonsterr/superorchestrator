//! Method handlers registered on the supervisor's router.

mod harnesses;

pub use harnesses::harnesses_view;

use std::sync::Arc;

use crate::server::SupervisorState;

/// Registers every handler that ships with the supervisor. The CLI daemon and the tests
/// both start from this; later tasks add their methods here.
pub fn default_handlers(state: &Arc<SupervisorState>) {
    harnesses::register_harnesses(state);
    crate::runs::register(state);
}

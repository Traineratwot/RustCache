//! REST API: shared state, error type, handlers, PAC generation, routes.

pub mod error;
pub mod guard;
pub mod handlers;
pub mod pac;
pub mod routes;
pub mod state;

pub use error::ApiError;
pub use routes::{pac_router, router};
pub use state::{ApiState, ListenerStatus};

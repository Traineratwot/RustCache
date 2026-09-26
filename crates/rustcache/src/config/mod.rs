//! Configuration: schema, validation, persistence, and hot-reload.

pub mod persist;
pub mod schema;
pub mod validate;
pub mod watch;

pub use schema::Config;
pub use validate::{FieldIssue, restart_fields_diff, validate_config};

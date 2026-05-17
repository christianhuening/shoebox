//! Shared types and utilities for shoebox.

pub mod error;
pub mod identity;

pub use error::{Error, Result};
pub use identity::{MachineId, UserId};

/// Schema version this build of shoebox understands.
/// Update this when the migration set changes.
pub const SCHEMA_VERSION: i64 = 6;

//! Shared types and utilities for shoebox.

pub mod error;

pub use error::{Error, Result};

/// Schema version this build of shoebox understands.
/// Update this when the migration set changes.
pub const SCHEMA_VERSION: i64 = 6;

//! Library facade for integration tests. The binary entry point lives
//! in `main.rs` and uses these modules directly.
//!
//! Plan 1.4 scaffolding — modules are added in subsequent tasks.

pub mod cert_store;
pub mod config;
pub mod discovery;
pub mod enrollment;
pub mod mtls_http;
pub mod replica;

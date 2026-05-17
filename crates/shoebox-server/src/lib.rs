//! Library facade for integration tests. The binary entry point lives
//! in `main.rs` and uses these modules directly.

pub mod config;
pub mod db;
pub mod http;
pub mod logging;
pub mod mdns;

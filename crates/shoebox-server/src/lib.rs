//! Library facade for integration tests. The binary entry point lives
//! in `main.rs` and uses these modules directly.

pub mod ca;
pub mod cli;
pub mod config;
pub mod db;
pub mod enroll;
pub mod http;
pub mod identity;
pub mod logging;
pub mod mdns;
pub mod mtls;
pub mod revoke;
pub mod secret;
pub mod tls_server;
pub mod whoami;

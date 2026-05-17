//! Library facade for integration tests. The binary entry point lives
//! in `main.rs` and uses these modules directly.

pub mod ca;
pub mod cli;
pub mod config;
pub mod db;
pub mod enroll;
pub mod hashing;
pub mod http;
pub mod identity;
pub mod indexer;
pub mod logging;
pub mod mdns;
pub mod mtls;
pub mod proxy;
pub mod raw_preview;
pub mod revoke;
pub mod secret;
pub mod sqld_embed;
pub mod tls_server;
pub mod whoami;

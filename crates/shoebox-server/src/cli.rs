//! Command-line interface.
//!
//! Default invocation (no subcommand) is `serve`, preserving Plan 1.1's
//! zero-arg behaviour. The `revoke` subcommand revokes a client cert by
//! its serial.

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "shoebox-server", about = "shoebox catalog server")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the server (default if no subcommand is given).
    Serve,
    /// Revoke a client certificate by its hex serial.
    Revoke(RevokeArgs),
}

#[derive(Debug, clap::Args)]
pub struct RevokeArgs {
    /// Hex-encoded serial number of the cert to revoke.
    #[arg(long)]
    pub serial: String,

    /// Optional human-readable reason.
    #[arg(long)]
    pub reason: Option<String>,
}

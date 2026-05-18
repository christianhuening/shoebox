//! `revoke` subcommand implementation.
//!
//! Sub-1-3-5 routed all server-side writes through the embedded sqld
//! subprocess, so this subcommand briefly spawns sqld, applies the
//! INSERT, and shuts sqld down. Must not be run while `shoebox-server
//! serve` is active against the same `data_dir` — both processes would
//! contend on sqld's data files.

use anyhow::Result;

use crate::cli::RevokeArgs;
use crate::config::Config;
use crate::db::Db;
use crate::sqld_embed;

pub async fn run(args: &RevokeArgs, cfg: &Config) -> Result<()> {
    std::fs::create_dir_all(&cfg.data_dir)?;
    let embedded = sqld_embed::start(cfg.data_dir.clone()).await?;
    let db = Db::open(&embedded.local_url).await?;
    db.insert_revoked_cert(&args.serial, args.reason.as_deref(), None)
        .await?;
    embedded.shutdown().await;
    println!(
        "Revoked cert with serial {} (reason: {})",
        args.serial,
        args.reason.as_deref().unwrap_or("<none>")
    );
    Ok(())
}

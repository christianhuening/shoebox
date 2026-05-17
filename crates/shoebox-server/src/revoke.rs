//! `revoke` subcommand implementation.

use anyhow::Result;

use crate::cli::RevokeArgs;
use crate::config::Config;
use crate::db::Db;

pub async fn run(args: &RevokeArgs, cfg: &Config) -> Result<()> {
    let db = Db::open(&cfg.data_dir.join("catalog.db")).await?;
    db.insert_revoked_cert(&args.serial, args.reason.as_deref(), None)
        .await?;
    println!(
        "Revoked cert with serial {} (reason: {})",
        args.serial,
        args.reason.as_deref().unwrap_or("<none>")
    );
    Ok(())
}

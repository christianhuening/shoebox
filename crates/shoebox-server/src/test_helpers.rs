//! Test helpers exposed to integration tests in this crate and the
//! `shoebox-client` crate. Gated behind the `test-helpers` cargo feature
//! so they never compile into the production binary.

use std::sync::Arc;
use tempfile::TempDir;

use crate::db::Db;
use crate::sqld_embed::{self, EmbeddedSqld};

/// A spawned `sqld` subprocess plus a `Db` connected to it via libsql's
/// remote backend. The temp data dir is held for the lifetime of the
/// struct; `shutdown()` consumes self and SIGKILLs the child.
pub struct TestDb {
    pub db: Arc<Db>,
    pub embedded: EmbeddedSqld,
    pub data_dir: TempDir,
}

impl TestDb {
    /// Spawn a fresh sqld in a temp directory and open a Db against it.
    /// Returns once sqld is accepting HTTP connections and all migrations
    /// have been applied.
    pub async fn start() -> Self {
        let data_dir = TempDir::new().expect("creating temp data dir");
        let embedded = sqld_embed::start(data_dir.path().to_path_buf())
            .await
            .expect("spawning sqld");
        let db = Arc::new(
            Db::open(&embedded.local_url)
                .await
                .expect("opening Db against sqld"),
        );
        Self {
            db,
            embedded,
            data_dir,
        }
    }

    /// SIGKILL the sqld child. The `TempDir` is dropped (deleted) by the
    /// struct's normal Drop.
    pub async fn shutdown(self) {
        self.embedded.shutdown().await;
    }
}

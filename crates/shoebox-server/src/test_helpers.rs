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
    ///
    /// Panics if sqld is not available — call `is_sqld_available()` first
    /// from tests that should skip rather than fail on a dev machine
    /// without sqld installed.
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

/// Returns true if a `sqld` binary is reachable via PATH or the
/// `SHOEBOX_SQLD_PATH` env var. Tests that need sqld should early-return
/// when this is false so dev machines without sqld installed don't see
/// confusing panics from the spawn attempt.
#[must_use]
pub fn is_sqld_available() -> bool {
    let sqld_binary_name =
        std::env::var("SHOEBOX_SQLD_PATH").unwrap_or_else(|_| "sqld".to_string());
    which::which(&sqld_binary_name).is_ok()
}

/// Convenience macro: at the top of a `#[tokio::test]`, skip with a
/// printed message when sqld is missing. Usage:
/// `if shoebox_server::test_helpers::skip_unless_sqld!() { return; }`.
#[macro_export]
macro_rules! skip_unless_sqld {
    () => {{
        if !$crate::test_helpers::is_sqld_available() {
            eprintln!(
                "skipping: sqld not on PATH (set SHOEBOX_SQLD_PATH to override). \
                 Install: see crates/shoebox-server/README or the project Dockerfile."
            );
            true
        } else {
            false
        }
    }};
}

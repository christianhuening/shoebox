//! Spawns and manages a `sqld` child process bound to a loopback port.
//!
//! The path to the `sqld` binary defaults to `sqld` (assumed on PATH);
//! `SHOEBOX_SQLD_PATH` env var overrides.
//!
//! ## Architecture note — two writers to `catalog.db` (v1 known risk)
//!
//! For Plan 1.3 v1 we keep `<data_dir>/catalog.db` as the single source of
//! truth. The migration runner (`Db`) opens the file on startup, runs
//! migrations, then keeps its handle alive for server-side bookkeeping writes
//! (revoked_certs, sessions, etc.). At the same time, `sqld` opens that same
//! file and serves it over the libSQL wire protocol.
//!
//! **This means two processes can write to the same SQLite file concurrently.**
//! SQLite's WAL mode handles concurrent *readers* fine but is not safe for
//! concurrent *writers* from separate processes unless carefully coordinated.
//! In practice the server-side writes are infrequent and the risk of a write
//! collision is low, but this is a known architectural debt accepted for v1.
//!
//! **Resolution target:** Plan 1.3 Task 22 (Dockerfile) should be accompanied
//! by a follow-up task that re-routes all server-side catalog access through
//! the loopback sqld connection and eliminates the direct `Db` file handle.
//!
//! ## Runtime requirement
//!
//! `sqld` must be on `PATH` (or `SHOEBOX_SQLD_PATH` must point at the binary).
//! Plan 1.3 Task 22 will update the Dockerfile to install sqld alongside
//! shoebox-server.

use anyhow::{anyhow, bail, Context, Result};
use std::net::{SocketAddr, TcpListener};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::{Child, Command};

/// A running `sqld` child process bound to an ephemeral loopback port.
pub struct EmbeddedSqld {
    /// Loopback URL the proxy targets, e.g. `http://127.0.0.1:53421`.
    pub local_url: String,
    /// Child process handle. Drop will SIGKILL.
    pub child: Child,
}

impl EmbeddedSqld {
    /// Gracefully shut down the child process.
    pub async fn shutdown(mut self) {
        if let Some(process_id) = self.child.id() {
            tracing::info!(event = "sqld.shutdown", pid = process_id);
        }
        let _ = self.child.start_kill();
        let _ = self.child.wait().await;
    }
}

/// Locate the sqld binary. Honors `SHOEBOX_SQLD_PATH` env var; else uses `sqld` on PATH.
fn sqld_binary() -> String {
    std::env::var("SHOEBOX_SQLD_PATH").unwrap_or_else(|_| "sqld".to_string())
}

/// Pick a free loopback port.
///
/// There is a TOCTOU window between us picking the port and sqld binding it,
/// but on a loopback interface with no other shoebox-server running this is
/// acceptable.
fn pick_loopback_port() -> Result<u16> {
    let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .map_err(|e| anyhow!("binding ephemeral port: {e}"))?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}

/// Wait for sqld to become reachable.
///
/// Polls a set of common health endpoints every 100ms for up to 10 seconds.
/// A 404 response is accepted as "server is up, just not that path".
async fn wait_until_ready(local_url: &str) -> Result<()> {
    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_millis(500))
        .build()?;
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        for health_path in ["/v1/health", "/health", "/"] {
            let health_url = format!("{local_url}{health_path}");
            if let Ok(response) = http_client.get(&health_url).send().await {
                let status_code = response.status().as_u16();
                if response.status().is_success() || status_code == 404 {
                    // Reachable. 404 is fine — server is up, just not that path.
                    return Ok(());
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    bail!("sqld did not become reachable within 10s on {}", local_url)
}

/// Spawn a `sqld` child process serving `<data_dir>/sqld/` on an ephemeral loopback port.
///
/// The sqld subprocess manages its own data directory at `<data_dir>/sqld/`.
/// The shoebox migration runner continues to use `<data_dir>/catalog.db`
/// directly.
///
/// ## v1 known risk — concurrent writers
///
/// See the module-level documentation for the "two writers" risk accepted for
/// Plan 1.3 v1. This will be resolved in a follow-up task.
///
/// ## Flags used for spawning
///
/// - `--http-listen-addr 127.0.0.1:<port>` — bind HTTP (v0.24-era flag name)
/// - `--db-path <dir>` — database directory
///
/// If your sqld version uses different flag names, override via `SHOEBOX_SQLD_PATH`
/// pointing at a wrapper script that translates flags.
pub async fn start(data_dir: PathBuf) -> Result<EmbeddedSqld> {
    let ephemeral_port = pick_loopback_port()?;
    let local_url = format!("http://127.0.0.1:{ephemeral_port}");
    let binary_path = sqld_binary();

    let sqld_data_subdir = data_dir.join("sqld");
    std::fs::create_dir_all(&sqld_data_subdir).context("creating sqld subdir")?;

    // sqld arg conventions vary across versions; the common ones (v0.24-era):
    //   --http-listen-addr 127.0.0.1:PORT
    //   --db-path <dir-or-file>
    let mut spawn_cmd = Command::new(&binary_path);
    spawn_cmd
        .arg("--http-listen-addr")
        .arg(format!("127.0.0.1:{ephemeral_port}"))
        .arg("--db-path")
        .arg(&sqld_data_subdir)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    tracing::info!(
        event = "sqld.spawn",
        bin = %binary_path,
        local_url = %local_url,
        db_path = ?sqld_data_subdir,
        "spawning sqld subprocess"
    );

    let mut child_process = spawn_cmd.spawn().with_context(|| {
        format!("spawning {binary_path} (set SHOEBOX_SQLD_PATH if sqld is not on PATH)")
    })?;

    // Pipe sqld's stderr into our tracing output so its logs are visible.
    if let Some(stderr_stream) = child_process.stderr.take() {
        tokio::spawn(async move {
            use tokio::io::{AsyncBufReadExt, BufReader};
            let mut line_reader = BufReader::new(stderr_stream).lines();
            while let Ok(Some(log_line)) = line_reader.next_line().await {
                tracing::debug!(event = "sqld.stderr", line = %log_line);
            }
        });
    }

    // Block until sqld is accepting HTTP connections.
    wait_until_ready(&local_url).await.context(
        "sqld failed to start; check that SHOEBOX_SQLD_PATH points at a working sqld binary",
    )?;

    Ok(EmbeddedSqld {
        local_url,
        child: child_process,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Skipped if `sqld` is not on PATH.
    ///
    /// To run this test: install sqld via
    /// `cargo install --git https://github.com/tursodatabase/libsql sqld`
    /// or set `SHOEBOX_SQLD_PATH` to an existing sqld binary.
    #[tokio::test]
    async fn starts_subprocess_if_sqld_present() {
        let binary_name = sqld_binary();
        if which::which(&binary_name).is_err() {
            eprintln!("skipping: sqld not on PATH (set SHOEBOX_SQLD_PATH to override)");
            return;
        }
        let temporary_dir = tempfile::TempDir::new().unwrap();
        let embedded_instance = start(temporary_dir.path().to_path_buf()).await.unwrap();
        assert!(embedded_instance.local_url.starts_with("http://127.0.0.1:"));
        embedded_instance.shutdown().await;
    }
}

//! Shared client state: cert, mTLS client, replica, config, current
//! connection status, current user, current screen. Owned by `main.rs`
//! behind `Arc<RwLock<…>>`; screens borrow read-only via `view()` and
//! mutate via messages dispatched by `update()`.

use std::path::PathBuf;
use std::sync::Arc;

use crate::config::ClientConfig;
use crate::replica::Replica;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionStatus {
    #[default]
    Disconnected,
    Connecting,
    Online,
    Offline,
}

/// True iff the user explicitly chose file storage over keychain during
/// this session's enrollment. Surfaced as a persistent warning banner.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct FileStorageWarning(pub bool);

pub struct AppState {
    pub config: ClientConfig,
    pub config_path: PathBuf,
    pub replica: Option<Arc<Replica>>,
    pub client: Option<reqwest::Client>,
    /// CA PEM pinned during the wizard (or loaded from disk on steady-
    /// state launches). Needed by `mtls_http::build_mtls_client` after
    /// cert rotation.
    pub ca_pem: Option<String>,
    pub connection_status: ConnectionStatus,
    pub file_storage_warning: FileStorageWarning,
    /// In-flight error to display in the current screen's inline area.
    /// Set by `update()` handlers; cleared on next user action.
    pub last_error: Option<String>,
}

impl AppState {
    /// Build an `AppState` with no resources yet — the wizard or
    /// steady-state init in `main.rs` fills the optional fields.
    #[must_use]
    pub fn new(config: ClientConfig, config_path: PathBuf) -> Self {
        Self {
            config,
            config_path,
            replica: None,
            client: None,
            ca_pem: None,
            connection_status: ConnectionStatus::Disconnected,
            file_storage_warning: FileStorageWarning(false),
            last_error: None,
        }
    }

    /// True iff `config.is_first_run()` AND no in-memory cert/client
    /// has been populated yet.
    #[must_use]
    pub fn needs_wizard(&self) -> bool {
        self.config.is_first_run() && self.client.is_none()
    }
}

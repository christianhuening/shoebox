//! Top-level Screen enum + Message enum. Each screen module exposes a
//! `view(&AppState) -> Element<Message>` function.

pub mod discovery;
pub mod enroll_progress;
pub mod enter_secret;
pub mod library;
pub mod profile_picker;

use crate::discovery::DiscoveredServer;
use crate::enrollment::EnrollResult;

#[derive(Debug, Clone, Default)]
#[allow(clippy::large_enum_variant)]
pub enum Screen {
    #[default]
    Discovery,
    EnterSecret {
        chosen_server: DiscoveredServer,
        /// Populated after `fetch_ca_cert` succeeds.
        ca_pem: Option<String>,
    },
    EnrollProgress {
        chosen_server: DiscoveredServer,
        ca_pem: String,
    },
    /// Shown when keychain write failed during `EnrollProgress` and the
    /// user is being asked whether to retry or use file storage.
    KeychainFailure {
        enroll_result: EnrollResult,
        chosen_server: DiscoveredServer,
        ca_pem: String,
        last_keychain_error: String,
    },
    ProfilePicker {
        /// Loaded once from `users` after the replica opens; refreshed on
        /// "Create new" success.
        users: Vec<UserRow>,
    },
    Library,
}

#[derive(Debug, Clone)]
pub struct UserRow {
    pub id: String,
    pub display_name: String,
}

/// Every Iced Message in the app. Categorised by which screen emits or
/// consumes it. Screen handlers in the screen modules pattern-match on
/// this enum.
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum Message {
    // Discovery
    ServerDiscovered(DiscoveredServer),
    DiscoveryError(String),
    DiscoveryRetry,
    ManualUrlSubmitted {
        display_name: String,
        url: String,
    },
    ServerPicked(DiscoveredServer),

    // EnterSecret + ca-cert bootstrap
    CaCertFetched(Result<String, String>),
    SecretSubmitted {
        secret: String,
        display_name: String,
    },

    // EnrollProgress
    EnrollFinished(Result<EnrollResult, String>),
    CertStored(Result<(), String>),
    /// User accepted the explicit consent fallback.
    UseFileStorageInstead,
    RetryKeychainStore,

    // ProfilePicker
    UsersLoaded(Result<Vec<UserRow>, String>),
    UserPicked(String),
    CreateUserSubmitted {
        display_name: String,
    },
    UserCreated(Result<UserRow, String>),

    // Library + background tickers
    ReplicaSyncTick,
    ReplicaSyncFinished(Result<u64, String>),
    CertRenewalTick,

    // Generic
    ClearError,
    Shutdown,
}

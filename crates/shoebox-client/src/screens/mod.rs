//! Top-level Screen enum + Message enum. Each screen module exposes a
//! `view(&AppState) -> Element<Message>` function.

pub mod discovery;
pub mod enroll_progress;
pub mod enter_secret;
pub mod library;
pub mod library_view;
pub mod profile_picker;

use std::sync::Arc;

use crate::discovery::DiscoveredServer;
use crate::enrollment::EnrollResult;
use crate::library_state::{DetailLoaded, FolderRow, GridCell, LockStatus, NavigationDirection};
use crate::replica::Replica;
use crate::thumb_cache::CachedResult;

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
    /// Fresh `LibraryStats` produced by `library::load_stats` after a
    /// successful sync (or after the user finishes the profile picker).
    LibraryStatsLoaded(Result<library::LibraryStats, String>),
    /// Result of the steady-state "open replica + load stats" task that
    /// runs once at startup when we're past first-run.
    ReplicaOpenedAndStatsLoaded(Result<OpenedReplicaBundle, String>),
    /// Result of post-enrollment "open replica + load users" task — hands
    /// the new `Arc<Replica>` plus user list back to `main.rs::update()`.
    EnrollmentFinalized {
        replica: Arc<Replica>,
        users: Vec<UserRow>,
    },
    CertRenewalTick,

    // Library view
    LibraryFolderTreeLoaded(Result<Vec<FolderRow>, String>),
    LibraryFolderSelected(String),
    LibraryGridLoaded {
        folder_id: String,
        cells: Result<Vec<GridCell>, String>,
    },
    LibraryThumbReady {
        hash: String,
        result: CachedResult,
    },
    LibraryGridCellSelected(usize),
    LibraryDetailLoaded(Result<DetailLoaded, String>),
    LibraryRatingChanged {
        variant_id: String,
        rating: u8,
    },
    LibraryRatingPersisted(Result<(), String>),
    LibraryKeywordInputChanged(String),
    LibraryKeywordSubmitted,
    LibraryKeywordAddPersisted(Result<(), String>),
    LibraryKeywordRemoveClicked {
        keyword_id: String,
    },
    LibraryKeywordRemovePersisted(Result<(), String>),
    LibraryNewVirtualCopyClicked,
    LibraryVirtualCopyPersisted(Result<String, String>),
    LibraryLockStatusTick,
    LibraryLockStatusLoaded(Result<LockStatus, String>),
    LibraryAcquireLockClicked,
    LibraryRequestTakeoverClicked,
    LibraryReleaseLockClicked,
    LibraryLockActionPersisted(Result<(), String>),
    LibraryLockHeartbeatTick,
    LibraryKeyboardNavigation(NavigationDirection),
    LibraryKeyboardRating(u8),
    LibraryClearError,

    // Generic
    ClearError,
    Shutdown,
}

/// Snapshot returned by the steady-state replica-open task. Carries
/// everything `update` needs to populate `AppState` in one shot.
#[derive(Debug, Clone)]
pub struct OpenedReplicaBundle {
    pub ca_pem: String,
    pub client: reqwest::Client,
    pub replica: Arc<Replica>,
    pub stats: library::LibraryStats,
}

//! shoebox-client binary — one Iced Application driving the
//! Plan 1.4 state machine.
//!
//! The Iced 0.13 builder pattern is `iced::application(title, update, view)
//! .subscription(sub).run_with(init)`. `App::update` / `App::view` /
//! `App::subscription` / `App::new` are method-references that satisfy
//! the corresponding `Fn` traits (`Update`, `View`, etc.).
//!
//! ## State model
//!
//! `AppState` is owned directly by `App` — *not* behind `Arc<RwLock<…>>` —
//! so that `view()` can borrow its `String`/`Option<String>` fields and
//! return an `Element<'_, Message>` tied to `&self`. (Holding a lock
//! guard across the lifetime of a returned Element doesn't compile.)
//!
//! Background tasks (replica sync, cert renewal, profile loads) get the
//! resources they need handed in by-clone: `Arc<Replica>` for the
//! catalog, `reqwest::Client` for HTTP (internally `Arc`-shared), owned
//! `String`s for URLs / PEMs. They return their results as
//! `Message` variants; `update()` writes those results back into the
//! owned `AppState`.

use std::sync::Arc;
use std::time::Duration;

use shoebox_client::app_state::{AppState, ConnectionStatus, FileStorageWarning};
use shoebox_client::cert_renewal::RenewalContext;
use shoebox_client::cert_store;
use shoebox_client::config::{default_config_path, ClientConfig};
use shoebox_client::discovery::{Browser, DiscoveredServer};
use shoebox_client::enrollment::{enroll, fetch_ca_cert, EnrollResult};
use shoebox_client::mtls_http::build_mtls_client;
use shoebox_client::replica::Replica;
use shoebox_client::screens::{
    discovery as discovery_screen, enroll_progress as enroll_progress_screen,
    enter_secret as enter_secret_screen, library as library_screen, library_view,
    profile_picker as profile_picker_screen, Message, OpenedReplicaBundle, Screen,
};

const REPLICA_SYNC_INTERVAL: Duration = Duration::from_secs(30);
const CERT_RENEWAL_INTERVAL: Duration = Duration::from_secs(12 * 60 * 60);
/// Cadence at which we drain the mDNS Browser's receiver while the
/// Discovery screen is showing.
const DISCOVERY_POLL_INTERVAL: Duration = Duration::from_millis(250);

fn main() -> iced::Result {
    tracing_subscriber::fmt::init();
    // Install the default rustls crypto provider once — the per-call
    // `ClientConfig::builder()` paths in `mtls_http` + `replica` both
    // require a provider to be installed for the process.
    let _ = rustls::crypto::ring::default_provider().install_default();
    iced::application("shoebox", App::update, App::view)
        .subscription(App::subscription)
        .run_with(App::new)
}

struct App {
    /// Owned source of truth. The fields the view borrows (`last_error`,
    /// `connection_status`, `file_storage_warning`, `config.server_url`)
    /// live here directly so view returns an `Element<'_, Message>` tied
    /// to `&self`.
    state: AppState,
    /// Current screen + its UI-only draft state.
    screen: Screen,
    discovered_servers: Vec<DiscoveredServer>,
    manual_url_draft: String,
    manual_name_draft: String,
    secret_draft: String,
    display_name_draft: String,
    new_user_draft: String,
    library_stats: library_screen::LibraryStats,
    /// mDNS browser. `None` once we've paired with a server.
    discovery_browser: Option<Browser>,
    /// Cert renewal task context. `None` until enrollment completes.
    renewal_context: Option<Arc<parking_lot::Mutex<RenewalContext>>>,
}

impl App {
    fn new() -> (Self, iced::Task<Message>) {
        let config_path = default_config_path().expect("config dir resolvable");
        let config = ClientConfig::read_from(&config_path).unwrap_or_default();
        let app_state = AppState::new(config, config_path);
        let initial_screen = if app_state.needs_wizard() {
            Screen::default()
        } else {
            Screen::Library
        };
        let discovery_browser = if matches!(initial_screen, Screen::Discovery) {
            Browser::start().ok()
        } else {
            None
        };
        // Steady-state launch: spawn the open-replica-and-load-stats
        // task BEFORE we move `app_state` into `App`, since we need
        // pieces of it (config) to drive the task.
        let initial_task = if matches!(initial_screen, Screen::Library) {
            let server_url = app_state.config.server_url.clone();
            let last_user = app_state.config.last_active_user_id.clone();
            iced::Task::perform(
                open_replica_and_load_stats(server_url, last_user),
                Message::ReplicaOpenedAndStatsLoaded,
            )
        } else {
            iced::Task::none()
        };
        let app = Self {
            state: app_state,
            screen: initial_screen,
            discovered_servers: Vec::new(),
            manual_url_draft: String::new(),
            manual_name_draft: String::new(),
            secret_draft: String::new(),
            display_name_draft: String::new(),
            new_user_draft: String::new(),
            library_stats: library_screen::LibraryStats::default(),
            discovery_browser,
            renewal_context: None,
        };
        (app, initial_task)
    }

    fn view(&self) -> iced::Element<'_, Message> {
        match &self.screen {
            Screen::Discovery => discovery_screen::view(
                self.state.last_error.as_deref(),
                &self.discovered_servers,
                &self.manual_url_draft,
                &self.manual_name_draft,
            ),
            Screen::EnterSecret {
                chosen_server,
                ca_pem,
            } => enter_secret_screen::view(
                self.state.last_error.as_deref(),
                chosen_server,
                &self.secret_draft,
                &self.display_name_draft,
                ca_pem.is_some(),
            ),
            Screen::EnrollProgress { .. } | Screen::KeychainFailure { .. } => {
                enroll_progress_screen::view(&self.screen)
            }
            Screen::ProfilePicker { users } => profile_picker_screen::view(
                self.state.last_error.as_deref(),
                users,
                &self.new_user_draft,
            ),
            Screen::Library => library_view::view(&self.state),
        }
    }

    fn subscription(&self) -> iced::Subscription<Message> {
        let mut subscriptions = Vec::new();
        if matches!(self.screen, Screen::Library) {
            subscriptions
                .push(iced::time::every(REPLICA_SYNC_INTERVAL).map(|_| Message::ReplicaSyncTick));
            subscriptions
                .push(iced::time::every(CERT_RENEWAL_INTERVAL).map(|_| Message::CertRenewalTick));
            subscriptions.push(library_view::keyboard_subscription());
            subscriptions.push(
                iced::time::every(std::time::Duration::from_secs(5))
                    .map(|_| Message::LibraryLockStatusTick),
            );
            subscriptions.push(
                iced::time::every(std::time::Duration::from_secs(300))
                    .map(|_| Message::LibraryLockHeartbeatTick),
            );
        }
        // mDNS events stream in via a polling subscription that drains
        // the Browser's receiver. For v1 simplicity: poll every 250 ms
        // while on the Discovery screen. `update()` handles each tick by
        // draining the browser's receiver — the sentinel `DiscoveredServer`
        // (empty url) is the signal to drain.
        if matches!(self.screen, Screen::Discovery) {
            subscriptions.push(iced::time::every(DISCOVERY_POLL_INTERVAL).map(|_| {
                Message::ServerDiscovered(DiscoveredServer {
                    display_name: String::new(),
                    url: String::new(),
                    manual: false,
                })
            }));
        }
        iced::Subscription::batch(subscriptions)
    }

    #[allow(clippy::too_many_lines, clippy::match_same_arms)]
    fn update(&mut self, message: Message) -> iced::Task<Message> {
        match message {
            Message::ServerDiscovered(sentinel_or_real) => {
                // Drain the browser's queue, then merge whatever's there.
                if let Some(browser) = self.discovery_browser.as_mut() {
                    while let Ok(real) = browser.rx.try_recv() {
                        discovery_screen::merge_discovered(&mut self.discovered_servers, real);
                    }
                }
                // If the message itself was a real entry (not a poll
                // sentinel — distinguished by non-empty url), merge it too.
                if !sentinel_or_real.url.is_empty() {
                    discovery_screen::merge_discovered(
                        &mut self.discovered_servers,
                        sentinel_or_real,
                    );
                }
                iced::Task::none()
            }
            Message::DiscoveryError(error_message) => {
                self.state.last_error = Some(error_message);
                iced::Task::none()
            }
            Message::DiscoveryRetry => {
                if let Some(browser) = self.discovery_browser.as_mut() {
                    if let Err(rebrowse_err) = browser.rebrowse() {
                        self.state.last_error = Some(format!("rebrowse failed: {rebrowse_err}"));
                    } else {
                        self.state.last_error = None;
                    }
                }
                iced::Task::none()
            }
            Message::ManualUrlSubmitted { display_name, url } => {
                self.manual_name_draft.clone_from(&display_name);
                self.manual_url_draft.clone_from(&url);
                if !url.is_empty() {
                    if let Some(browser) = self.discovery_browser.as_ref() {
                        browser.add_manual(&display_name, &url);
                    }
                }
                iced::Task::none()
            }
            Message::ServerPicked(server) => {
                self.screen = Screen::EnterSecret {
                    chosen_server: server.clone(),
                    ca_pem: None,
                };
                let target_url = server.url.clone();
                iced::Task::perform(
                    async move {
                        fetch_ca_cert(&target_url)
                            .await
                            .map_err(|fetch_err| fetch_err.to_string())
                    },
                    Message::CaCertFetched,
                )
            }
            Message::CaCertFetched(Ok(ca_pem)) => {
                if let Screen::EnterSecret { ca_pem: slot, .. } = &mut self.screen {
                    *slot = Some(ca_pem);
                }
                self.state.last_error = None;
                iced::Task::none()
            }
            Message::CaCertFetched(Err(ca_err)) => {
                self.state.last_error = Some(format!("fetching server CA: {ca_err}"));
                iced::Task::none()
            }
            Message::SecretSubmitted {
                secret,
                display_name,
            } => {
                self.secret_draft.clone_from(&secret);
                self.display_name_draft.clone_from(&display_name);
                let Screen::EnterSecret {
                    chosen_server,
                    ca_pem: Some(ca_pem),
                } = self.screen.clone()
                else {
                    return iced::Task::none();
                };
                self.screen = Screen::EnrollProgress {
                    chosen_server: chosen_server.clone(),
                    ca_pem: ca_pem.clone(),
                };
                let server_url = chosen_server.url.clone();
                let ca_for_enroll = ca_pem;
                iced::Task::perform(
                    async move {
                        enroll(&server_url, &ca_for_enroll, &secret, &display_name)
                            .await
                            .map_err(|enroll_err| enroll_err.to_string())
                    },
                    Message::EnrollFinished,
                )
            }
            Message::EnrollFinished(Ok(enroll_result)) => {
                let Screen::EnrollProgress {
                    chosen_server,
                    ca_pem,
                } = self.screen.clone()
                else {
                    return iced::Task::none();
                };
                // Try keychain first; on failure transition to KeychainFailure.
                let store_outcome = enroll_progress_screen::store_via_keychain_or_signal_failure(
                    &chosen_server.url,
                    &enroll_result.client_cert_pem,
                    &enroll_result.client_key_pem,
                );
                match store_outcome {
                    Ok(()) => self.finalize_enrollment(
                        chosen_server.url.clone(),
                        ca_pem,
                        enroll_result,
                        FileStorageWarning(false),
                    ),
                    Err(keyring_err) => {
                        self.screen = Screen::KeychainFailure {
                            enroll_result,
                            chosen_server,
                            ca_pem,
                            last_keychain_error: keyring_err,
                        };
                        iced::Task::none()
                    }
                }
            }
            Message::EnrollFinished(Err(enroll_err)) => {
                self.state.last_error = Some(enroll_err);
                // Drop back to EnterSecret so the user can retry.
                if let Screen::EnrollProgress {
                    chosen_server,
                    ca_pem,
                } = self.screen.clone()
                {
                    self.screen = Screen::EnterSecret {
                        chosen_server,
                        ca_pem: Some(ca_pem),
                    };
                }
                iced::Task::none()
            }
            Message::RetryKeychainStore => {
                let Screen::KeychainFailure {
                    enroll_result,
                    chosen_server,
                    ca_pem,
                    ..
                } = self.screen.clone()
                else {
                    return iced::Task::none();
                };
                let retry_outcome = enroll_progress_screen::store_via_keychain_or_signal_failure(
                    &chosen_server.url,
                    &enroll_result.client_cert_pem,
                    &enroll_result.client_key_pem,
                );
                match retry_outcome {
                    Ok(()) => self.finalize_enrollment(
                        chosen_server.url.clone(),
                        ca_pem,
                        enroll_result,
                        FileStorageWarning(false),
                    ),
                    Err(keyring_err) => {
                        if let Screen::KeychainFailure {
                            last_keychain_error,
                            ..
                        } = &mut self.screen
                        {
                            *last_keychain_error = keyring_err;
                        }
                        iced::Task::none()
                    }
                }
            }
            Message::UseFileStorageInstead => {
                let Screen::KeychainFailure {
                    enroll_result,
                    chosen_server,
                    ca_pem,
                    ..
                } = self.screen.clone()
                else {
                    return iced::Task::none();
                };
                match enroll_progress_screen::store_via_file(
                    &chosen_server.url,
                    &enroll_result.client_cert_pem,
                    &enroll_result.client_key_pem,
                ) {
                    Ok(()) => self.finalize_enrollment(
                        chosen_server.url.clone(),
                        ca_pem,
                        enroll_result,
                        FileStorageWarning(true),
                    ),
                    Err(file_err) => {
                        self.state.last_error =
                            Some(format!("file storage also failed: {file_err}"));
                        iced::Task::none()
                    }
                }
            }
            Message::CertStored(Ok(())) => iced::Task::none(),
            Message::CertStored(Err(store_err)) => {
                self.state.last_error = Some(store_err);
                iced::Task::none()
            }
            Message::UsersLoaded(Ok(users)) => {
                self.screen = Screen::ProfilePicker { users };
                iced::Task::none()
            }
            Message::UsersLoaded(Err(load_err)) => {
                self.state.last_error = Some(load_err);
                iced::Task::none()
            }
            Message::UserPicked(user_id) => {
                self.state.config.last_active_user_id = Some(user_id);
                let config_snapshot = self.state.config.clone();
                if let Err(write_err) = config_snapshot.write_to(&self.state.config_path) {
                    self.state.last_error = Some(format!("writing client.toml: {write_err}"));
                }
                self.screen = Screen::Library;
                let Some(replica) = self.state.replica.clone() else {
                    return iced::Task::none();
                };
                let last_user = self.state.config.last_active_user_id.clone();
                let stats_task = iced::Task::perform(
                    load_library_stats(replica.clone(), last_user),
                    Message::LibraryStatsLoaded,
                );
                iced::Task::batch([stats_task, command_for_folder_tree(replica)])
            }
            Message::CreateUserSubmitted { display_name } => {
                self.new_user_draft.clone_from(&display_name);
                let Some(replica) = self.state.replica.clone() else {
                    self.state.last_error = Some("no replica".to_string());
                    return iced::Task::none();
                };
                iced::Task::perform(
                    async move {
                        let conn = replica.conn().map_err(|conn_err| conn_err.to_string())?;
                        profile_picker_screen::create_user(&conn, &display_name)
                            .await
                            .map_err(|create_err| create_err.to_string())
                    },
                    Message::UserCreated,
                )
            }
            Message::UserCreated(Ok(new_user)) => {
                if let Screen::ProfilePicker { users } = &mut self.screen {
                    users.push(new_user);
                }
                iced::Task::none()
            }
            Message::UserCreated(Err(create_err)) => {
                self.state.last_error = Some(create_err);
                iced::Task::none()
            }
            Message::ReplicaSyncTick => {
                let Some(replica) = self.state.replica.clone() else {
                    return iced::Task::none();
                };
                iced::Task::perform(
                    async move {
                        replica
                            .sync()
                            .await
                            .map_err(|sync_err| sync_err.to_string())
                    },
                    Message::ReplicaSyncFinished,
                )
            }
            Message::ReplicaSyncFinished(Ok(frame_no)) => {
                self.library_stats.frame_no = frame_no;
                self.state.connection_status = ConnectionStatus::Online;
                let Some(replica) = self.state.replica.clone() else {
                    return iced::Task::none();
                };
                let last_user = self.state.config.last_active_user_id.clone();
                iced::Task::perform(
                    load_library_stats(replica, last_user),
                    Message::LibraryStatsLoaded,
                )
            }
            Message::ReplicaSyncFinished(Err(sync_err)) => {
                self.state.connection_status = ConnectionStatus::Offline;
                self.state.last_error = Some(sync_err);
                iced::Task::none()
            }
            Message::LibraryStatsLoaded(Ok(stats)) => {
                let frame_no = self.library_stats.frame_no;
                self.library_stats = stats;
                // Preserve frame_no — load_stats doesn't populate it (it
                // comes from Replica::sync()'s return value).
                self.library_stats.frame_no = frame_no;
                iced::Task::none()
            }
            Message::LibraryStatsLoaded(Err(load_err)) => {
                self.state.last_error = Some(load_err);
                iced::Task::none()
            }
            Message::ReplicaOpenedAndStatsLoaded(Ok(opened)) => {
                let OpenedReplicaBundle {
                    ca_pem,
                    client,
                    replica,
                    stats,
                    thumb_cache,
                } = opened;
                self.state.ca_pem = Some(ca_pem);
                self.state.client = Some(client);
                self.state.replica = Some(replica);
                self.state.connection_status = ConnectionStatus::Online;
                self.library_stats = stats;
                self.state.thumb_cache = Some(thumb_cache);
                let Some(replica) = self.state.replica.clone() else {
                    return iced::Task::none();
                };
                command_for_folder_tree(replica)
            }
            Message::ReplicaOpenedAndStatsLoaded(Err(open_err)) => {
                self.state.connection_status = ConnectionStatus::Offline;
                self.state.last_error = Some(open_err);
                iced::Task::none()
            }
            Message::CertRenewalTick => {
                if let Some(context) = self.renewal_context.clone() {
                    iced::Task::perform(
                        async move {
                            shoebox_client::cert_renewal::run_one(&context)
                                .await
                                .map_err(|renewal_err| renewal_err.to_string())
                        },
                        |result| match result {
                            Ok(()) => Message::ClearError,
                            Err(renewal_err) => Message::DiscoveryError(renewal_err),
                        },
                    )
                } else {
                    iced::Task::none()
                }
            }
            Message::ClearError => {
                self.state.last_error = None;
                iced::Task::none()
            }
            Message::EnrollmentFinalized {
                replica,
                users,
                thumb_cache,
            } => {
                self.state.replica = Some(replica);
                self.state.thumb_cache = Some(thumb_cache);
                self.state.connection_status = ConnectionStatus::Online;
                self.screen = Screen::ProfilePicker { users };
                iced::Task::none()
            }
            Message::Shutdown => iced::Task::none(),
            Message::LibraryFolderTreeLoaded(Ok(rows)) => {
                self.state.library_view.folder_tree = rows;
                self.state.library_view.error = None;
                if let Some(first) = self.state.library_view.folder_tree.first().cloned() {
                    self.state.library_view.selected_folder_id = Some(first.id.clone());
                    return command_for_grid(&self.state, first.id);
                }
                iced::Task::none()
            }
            Message::LibraryFolderTreeLoaded(Err(folder_tree_err)) => {
                self.state.library_view.error =
                    Some(format!("Folder tree failed: {folder_tree_err}"));
                iced::Task::none()
            }
            Message::LibraryFolderSelected(folder_id) => {
                self.state.library_view.selected_folder_id = Some(folder_id.clone());
                self.state.library_view.selected_grid_index = None;
                self.state.library_view.detail = None;
                self.state.library_view.lock_status =
                    shoebox_client::library_state::LockStatus::Free;
                command_for_grid(&self.state, folder_id)
            }
            Message::LibraryGridLoaded { folder_id, cells } => {
                if self.state.library_view.selected_folder_id.as_deref() != Some(&folder_id)
                    && !folder_id.is_empty()
                {
                    return iced::Task::none();
                }
                match cells {
                    Ok(loaded_cells) => {
                        self.state.library_view.grid = loaded_cells;
                        self.state.library_view.error = None;
                        let tasks = thumb_fetch_commands(&self.state);
                        iced::Task::batch(tasks)
                    }
                    Err(grid_err) => {
                        self.state.library_view.error =
                            Some(format!("Grid load failed: {grid_err}"));
                        iced::Task::none()
                    }
                }
            }
            Message::LibraryThumbReady { hash, result } => {
                if let Ok(image) = result {
                    for cell in &mut self.state.library_view.grid {
                        if cell.photo_id == hash {
                            cell.thumbnail = Some(image.clone());
                        }
                    }
                }
                iced::Task::none()
            }
            Message::LibraryGridCellSelected(index) => {
                self.state.library_view.selected_grid_index = Some(index);
                command_for_detail(&self.state)
            }
            Message::LibraryDetailLoaded(Ok(detail)) => {
                self.state.library_view.detail = Some(detail);
                self.state.library_view.error = None;
                command_for_lock_status(&self.state)
            }
            Message::LibraryDetailLoaded(Err(e)) => {
                self.state.library_view.error = Some(format!("Detail load failed: {e}"));
                iced::Task::none()
            }

            Message::LibraryRatingChanged { variant_id, rating } => {
                persist_rating(&self.state, variant_id, rating)
            }
            Message::LibraryKeyboardRating(rating) => {
                let Some(index) = self.state.library_view.selected_grid_index else {
                    return iced::Task::none();
                };
                let Some(cell) = self.state.library_view.grid.get(index).cloned() else {
                    return iced::Task::none();
                };
                persist_rating(&self.state, cell.variant_id, rating)
            }
            Message::LibraryRatingPersisted(Ok(())) => command_for_detail_and_grid(&self.state),
            Message::LibraryRatingPersisted(Err(e)) => {
                self.state.library_view.error = Some(format!("Save rating failed: {e}"));
                iced::Task::none()
            }

            Message::LibraryKeywordInputChanged(value) => {
                self.state.library_view.keyword_input = value;
                iced::Task::none()
            }
            Message::LibraryKeywordSubmitted => {
                let name = std::mem::take(&mut self.state.library_view.keyword_input)
                    .trim()
                    .to_string();
                if name.is_empty() {
                    return iced::Task::none();
                }
                let Some(replica) = self.state.replica.clone() else {
                    return iced::Task::none();
                };
                let Some(user_id) = self.state.config.last_active_user_id.clone() else {
                    return iced::Task::none();
                };
                let Some(detail) = self.state.library_view.detail.clone() else {
                    return iced::Task::none();
                };
                iced::Task::perform(
                    async move {
                        let conn = replica.conn().map_err(|error| error.to_string())?;
                        shoebox_client::library_state::add_keyword(
                            &conn,
                            &detail.photo_id,
                            &user_id,
                            &name,
                        )
                        .await
                        .map(|_| ())
                        .map_err(|error| error.to_string())
                    },
                    Message::LibraryKeywordAddPersisted,
                )
            }
            Message::LibraryKeywordAddPersisted(Ok(())) => command_for_detail(&self.state),
            Message::LibraryKeywordAddPersisted(Err(e)) => {
                self.state.library_view.error = Some(format!("Add keyword failed: {e}"));
                iced::Task::none()
            }

            Message::LibraryKeywordRemoveClicked { keyword_id } => {
                let Some(replica) = self.state.replica.clone() else {
                    return iced::Task::none();
                };
                let Some(detail) = self.state.library_view.detail.clone() else {
                    return iced::Task::none();
                };
                iced::Task::perform(
                    async move {
                        let conn = replica.conn().map_err(|error| error.to_string())?;
                        shoebox_client::library_state::remove_keyword(
                            &conn,
                            &detail.photo_id,
                            &keyword_id,
                        )
                        .await
                        .map_err(|error| error.to_string())
                    },
                    Message::LibraryKeywordRemovePersisted,
                )
            }
            Message::LibraryKeywordRemovePersisted(Ok(())) => command_for_detail(&self.state),
            Message::LibraryKeywordRemovePersisted(Err(e)) => {
                self.state.library_view.error = Some(format!("Remove keyword failed: {e}"));
                iced::Task::none()
            }

            Message::LibraryNewVirtualCopyClicked => {
                let Some(replica) = self.state.replica.clone() else {
                    return iced::Task::none();
                };
                let Some(user_id) = self.state.config.last_active_user_id.clone() else {
                    return iced::Task::none();
                };
                let Some(detail) = self.state.library_view.detail.clone() else {
                    return iced::Task::none();
                };
                iced::Task::perform(
                    async move {
                        let conn = replica.conn().map_err(|error| error.to_string())?;
                        shoebox_client::library_state::create_virtual_copy(
                            &conn,
                            &detail.photo_id,
                            &user_id,
                        )
                        .await
                        .map_err(|error| error.to_string())
                    },
                    Message::LibraryVirtualCopyPersisted,
                )
            }
            Message::LibraryVirtualCopyPersisted(Ok(_)) => {
                let Some(folder_id) = self.state.library_view.selected_folder_id.clone() else {
                    return iced::Task::none();
                };
                command_for_grid(&self.state, folder_id)
            }
            Message::LibraryVirtualCopyPersisted(Err(e)) => {
                self.state.library_view.error = Some(format!("Create virtual copy failed: {e}"));
                iced::Task::none()
            }

            Message::LibraryLockStatusTick => command_for_lock_status(&self.state),
            Message::LibraryLockStatusLoaded(Ok(status)) => {
                self.state.library_view.lock_status = status;
                iced::Task::none()
            }
            Message::LibraryLockStatusLoaded(Err(e)) => {
                self.state.library_view.error = Some(format!("Lock status load failed: {e}"));
                iced::Task::none()
            }
            Message::LibraryAcquireLockClicked => {
                http_lock_command(&self.state, LockAction::Acquire)
            }
            Message::LibraryReleaseLockClicked => {
                http_lock_command(&self.state, LockAction::Release)
            }
            Message::LibraryRequestTakeoverClicked => {
                http_lock_command(&self.state, LockAction::Takeover)
            }
            Message::LibraryLockActionPersisted(Ok(())) => command_for_lock_status(&self.state),
            Message::LibraryLockActionPersisted(Err(e)) => {
                self.state.library_view.error = Some(format!("Lock action failed: {e}"));
                iced::Task::none()
            }
            Message::LibraryLockHeartbeatTick => {
                if !matches!(
                    self.state.library_view.lock_status,
                    shoebox_client::library_state::LockStatus::HeldByYou
                        | shoebox_client::library_state::LockStatus::HeldByYouTakeoverPending { .. }
                ) {
                    return iced::Task::none();
                }
                let Some(client) = self.state.client.clone() else {
                    return iced::Task::none();
                };
                let Some(detail) = self.state.library_view.detail.clone() else {
                    return iced::Task::none();
                };
                let server_url = self.state.config.server_url.clone();
                iced::Task::perform(
                    async move {
                        shoebox_client::library_state::http_heartbeat_lock(
                            &client,
                            &server_url,
                            &detail.variant_id,
                        )
                        .await
                        .map_err(|error| error.to_string())
                    },
                    Message::LibraryLockActionPersisted,
                )
            }

            Message::LibraryKeyboardNavigation(direction) => {
                let total = self.state.library_view.grid.len();
                let cells_per_row = self.state.library_view.cells_per_row.max(4);
                let next = shoebox_client::library_state::advance_selection(
                    self.state.library_view.selected_grid_index,
                    total,
                    cells_per_row,
                    direction,
                );
                self.state.library_view.selected_grid_index = next;
                command_for_detail(&self.state)
            }
            Message::LibraryClearError => {
                self.state.library_view.error = None;
                iced::Task::none()
            }
        }
    }

    /// After successful keychain or file storage, write `client.toml`,
    /// open the replica, build the mTLS client, load users, transition
    /// to `ProfilePicker`.
    #[allow(clippy::needless_pass_by_value)]
    fn finalize_enrollment(
        &mut self,
        server_url: String,
        ca_pem: String,
        enroll_result: EnrollResult,
        file_storage_warning: FileStorageWarning,
    ) -> iced::Task<Message> {
        self.state.config.server_url.clone_from(&server_url);
        self.state
            .config
            .cert_serial_hex
            .clone_from(&enroll_result.cert_serial_hex);
        self.state.file_storage_warning = file_storage_warning;
        self.state.ca_pem = Some(ca_pem.clone());
        let config_snapshot = self.state.config.clone();
        if let Err(write_err) = config_snapshot.write_to(&self.state.config_path) {
            self.state.last_error = Some(format!("writing client.toml: {write_err}"));
        }

        // Build the mTLS client.
        let cert_pem = enroll_result.client_cert_pem.clone();
        let key_pem = enroll_result.client_key_pem.clone();
        let mtls_client = match build_mtls_client(&ca_pem, &cert_pem, &key_pem) {
            Ok(client) => client,
            Err(build_err) => {
                self.state.last_error = Some(format!("could not build mTLS client: {build_err}"));
                return iced::Task::none();
            }
        };

        // Set up the cert renewal context (12 h ticker uses it).
        let renewal = Arc::new(parking_lot::Mutex::new(RenewalContext {
            server_url: server_url.clone(),
            client: mtls_client.clone(),
            config_path: self.state.config_path.clone(),
            not_after_unix: enroll_result.not_after_unix,
        }));
        self.renewal_context = Some(renewal);

        // Build the thumb cache up front so it can be handed to update()
        // along with the post-enrollment replica.
        let thumb_dir = directories::ProjectDirs::from("io", "shoebox", "shoebox-client")
            .map_or_else(
                || std::path::PathBuf::from("./shoebox-thumbs"),
                |project_dirs| project_dirs.cache_dir().join("thumbs"),
            );
        let thumb_cache = match shoebox_client::thumb_cache::ThumbCache::new(
            mtls_client.clone(),
            server_url.clone(),
            thumb_dir,
        ) {
            Ok(cache) => cache,
            Err(cache_err) => {
                self.state.last_error =
                    Some(format!("could not build thumbnail cache: {cache_err}"));
                return iced::Task::none();
            }
        };

        self.state.client = Some(mtls_client);
        self.discovery_browser = None; // we're paired

        let server_url_for_task = server_url;
        let ca_for_task = ca_pem;
        let cert_for_task = cert_pem;
        let key_for_task = key_pem;
        let thumb_cache_for_task = thumb_cache;
        iced::Task::perform(
            async move {
                let local_path = replica_local_path(&server_url_for_task)?;
                let replica = Replica::open(
                    &local_path,
                    &server_url_for_task,
                    &ca_for_task,
                    &cert_for_task,
                    &key_for_task,
                )
                .await
                .map_err(|open_err| open_err.to_string())?;
                replica
                    .sync()
                    .await
                    .map_err(|sync_err| sync_err.to_string())?;
                let conn = replica.conn().map_err(|conn_err| conn_err.to_string())?;
                let users = profile_picker_screen::load_users(&conn)
                    .await
                    .map_err(|load_err| load_err.to_string())?;
                Ok::<
                    (
                        Arc<Replica>,
                        Vec<shoebox_client::screens::UserRow>,
                        shoebox_client::thumb_cache::ThumbCache,
                    ),
                    String,
                >((Arc::new(replica), users, thumb_cache_for_task))
            },
            |result| match result {
                Ok((replica, users, thumb_cache)) => Message::EnrollmentFinalized {
                    replica,
                    users,
                    thumb_cache,
                },
                Err(open_err) => Message::UsersLoaded(Err(open_err)),
            },
        )
    }
}

/// Build the on-disk path where the libSQL replica for `server_url` lives.
///
/// # Errors
/// Returns an error string if the platform's app-data dir can't be
/// resolved (extremely rare; headless build with no `$HOME`).
fn replica_local_path(server_url: &str) -> Result<std::path::PathBuf, String> {
    let project_dirs = directories::ProjectDirs::from("io", "shoebox", "shoebox-client")
        .ok_or_else(|| "could not determine data dir".to_string())?;
    let server_slug = hex::encode(blake3::hash(server_url.as_bytes()).as_bytes());
    Ok(project_dirs
        .data_local_dir()
        .join("replicas")
        .join(server_slug)
        .join("catalog.db"))
}

/// Steady-state launch path: re-load cert from keyring/file, build the
/// mTLS client, open the local replica, sync it, then load stats.
///
/// # Errors
/// Returns an error string on any I/O, network, or libsql failure.
async fn open_replica_and_load_stats(
    server_url: String,
    last_user_id: Option<String>,
) -> Result<OpenedReplicaBundle, String> {
    let cert_and_key_pair = cert_store::load_from_keyring(&server_url)
        .unwrap_or_default()
        .or_else(|| cert_store::load_from_file(&server_url).unwrap_or_default());
    let (cert_pem, key_pem) =
        cert_and_key_pair.ok_or_else(|| "no client cert stored".to_string())?;
    let ca_pem = fetch_ca_cert(&server_url)
        .await
        .map_err(|fetch_err| fetch_err.to_string())?;
    let mtls_client = build_mtls_client(&ca_pem, &cert_pem, &key_pem)
        .map_err(|build_err| build_err.to_string())?;
    let thumb_dir = directories::ProjectDirs::from("io", "shoebox", "shoebox-client").map_or_else(
        || std::path::PathBuf::from("./shoebox-thumbs"),
        |project_dirs| project_dirs.cache_dir().join("thumbs"),
    );
    let thumb_cache = shoebox_client::thumb_cache::ThumbCache::new(
        mtls_client.clone(),
        server_url.clone(),
        thumb_dir,
    )
    .map_err(|cache_err| cache_err.to_string())?;
    let local_path = replica_local_path(&server_url)?;
    let replica = Replica::open(&local_path, &server_url, &ca_pem, &cert_pem, &key_pem)
        .await
        .map_err(|open_err| open_err.to_string())?;
    let frame_no = replica
        .sync()
        .await
        .map_err(|sync_err| sync_err.to_string())?;
    let conn = replica.conn().map_err(|conn_err| conn_err.to_string())?;
    let mut stats = library_screen::load_stats(&conn, last_user_id.as_deref())
        .await
        .map_err(|stats_err| stats_err.to_string())?;
    stats.frame_no = frame_no;
    Ok(OpenedReplicaBundle {
        ca_pem,
        client: mtls_client,
        replica: Arc::new(replica),
        stats,
        thumb_cache,
    })
}

/// Read fresh `LibraryStats` from an open replica using the currently
/// active user id.
///
/// # Errors
/// Returns a string error if connection setup or one of the underlying
/// queries fails.
async fn load_library_stats(
    replica: Arc<Replica>,
    last_user_id: Option<String>,
) -> Result<library_screen::LibraryStats, String> {
    let conn = replica.conn().map_err(|conn_err| conn_err.to_string())?;
    library_screen::load_stats(&conn, last_user_id.as_deref())
        .await
        .map_err(|stats_err| stats_err.to_string())
}

/// Spawn a task that loads the library's folder tree from the replica and
/// dispatches `LibraryFolderTreeLoaded`.
fn command_for_folder_tree(replica: Arc<Replica>) -> iced::Task<Message> {
    iced::Task::perform(
        async move {
            let conn = replica.conn().map_err(|conn_err| conn_err.to_string())?;
            shoebox_client::library_state::load_folder_tree(&conn)
                .await
                .map_err(|tree_err| tree_err.to_string())
        },
        Message::LibraryFolderTreeLoaded,
    )
}

/// Spawn a task that loads grid cells for a given folder and dispatches
/// `LibraryGridLoaded` with the folder id preserved on both success and
/// failure paths.
fn command_for_grid(state: &AppState, folder_id: String) -> iced::Task<Message> {
    let Some(replica) = state.replica.clone() else {
        return iced::Task::none();
    };
    let Some(user_id) = state.config.last_active_user_id.clone() else {
        return iced::Task::none();
    };
    iced::Task::perform(
        async move {
            let conn = replica.conn().map_err(|conn_err| conn_err.to_string())?;
            shoebox_client::library_state::load_grid_for_folder(&conn, &folder_id, &user_id)
                .await
                .map_err(|grid_err| grid_err.to_string())
                .map(|cells| (folder_id, cells))
        },
        |result| match result {
            Ok((folder_id, cells)) => Message::LibraryGridLoaded {
                folder_id,
                cells: Ok(cells),
            },
            Err(grid_err) => Message::LibraryGridLoaded {
                folder_id: String::new(),
                cells: Err(grid_err),
            },
        },
    )
}

/// Spawn one `LibraryThumbReady`-dispatching task per grid cell that does
/// not yet have a thumbnail loaded. Empty when the cache is missing.
fn thumb_fetch_commands(state: &AppState) -> Vec<iced::Task<Message>> {
    let Some(thumb_cache) = state.thumb_cache.clone() else {
        return Vec::new();
    };
    state
        .library_view
        .grid
        .iter()
        .filter(|cell| cell.thumbnail.is_none())
        .map(|cell| {
            let hash = cell.photo_id.clone();
            let thumb_cache = thumb_cache.clone();
            iced::Task::perform(
                async move {
                    let result = thumb_cache.get(&hash).await;
                    (hash, result)
                },
                |(hash, result)| Message::LibraryThumbReady { hash, result },
            )
        })
        .collect()
}

/// Spawn a task that loads detail metadata for the currently selected grid
/// cell, dispatching `LibraryDetailLoaded`.
fn command_for_detail(state: &AppState) -> iced::Task<Message> {
    let Some(replica) = state.replica.clone() else {
        return iced::Task::none();
    };
    let Some(user_id) = state.config.last_active_user_id.clone() else {
        return iced::Task::none();
    };
    let Some(selected_index) = state.library_view.selected_grid_index else {
        return iced::Task::none();
    };
    let Some(selected_cell) = state.library_view.grid.get(selected_index).cloned() else {
        return iced::Task::none();
    };
    iced::Task::perform(
        async move {
            let conn = replica.conn().map_err(|conn_err| conn_err.to_string())?;
            shoebox_client::library_state::load_detail(&conn, &selected_cell.variant_id, &user_id)
                .await
                .map_err(|detail_err| detail_err.to_string())
        },
        Message::LibraryDetailLoaded,
    )
}

fn persist_rating(state: &AppState, variant_id: String, rating: u8) -> iced::Task<Message> {
    let Some(replica) = state.replica.clone() else {
        return iced::Task::none();
    };
    let Some(user_id) = state.config.last_active_user_id.clone() else {
        return iced::Task::none();
    };
    iced::Task::perform(
        async move {
            let conn = replica.conn().map_err(|error| error.to_string())?;
            shoebox_client::library_state::upsert_rating(&conn, &variant_id, &user_id, rating)
                .await
                .map_err(|error| error.to_string())
        },
        Message::LibraryRatingPersisted,
    )
}

fn command_for_detail_and_grid(state: &AppState) -> iced::Task<Message> {
    let mut tasks = vec![command_for_detail(state)];
    if let Some(folder_id) = state.library_view.selected_folder_id.clone() {
        tasks.push(command_for_grid(state, folder_id));
    }
    iced::Task::batch(tasks)
}

fn command_for_lock_status(state: &AppState) -> iced::Task<Message> {
    let Some(replica) = state.replica.clone() else {
        return iced::Task::none();
    };
    let Some(user_id) = state.config.last_active_user_id.clone() else {
        return iced::Task::none();
    };
    let Some(detail) = state.library_view.detail.clone() else {
        return iced::Task::none();
    };
    iced::Task::perform(
        async move {
            let conn = replica.conn().map_err(|error| error.to_string())?;
            shoebox_client::library_state::load_lock_status(&conn, &detail.variant_id, &user_id)
                .await
                .map_err(|error| error.to_string())
        },
        Message::LibraryLockStatusLoaded,
    )
}

#[derive(Clone, Copy)]
enum LockAction {
    Acquire,
    Release,
    Takeover,
}

fn http_lock_command(state: &AppState, action: LockAction) -> iced::Task<Message> {
    let Some(client) = state.client.clone() else {
        return iced::Task::none();
    };
    let Some(detail) = state.library_view.detail.clone() else {
        return iced::Task::none();
    };
    let server_url = state.config.server_url.clone();
    match action {
        LockAction::Acquire => iced::Task::perform(
            async move {
                shoebox_client::library_state::http_acquire_lock(
                    &client,
                    &server_url,
                    &detail.variant_id,
                )
                .await
                .map(|_| ())
                .map_err(|error| error.to_string())
            },
            Message::LibraryLockActionPersisted,
        ),
        LockAction::Release => iced::Task::perform(
            async move {
                shoebox_client::library_state::http_release_lock(
                    &client,
                    &server_url,
                    &detail.variant_id,
                )
                .await
                .map_err(|error| error.to_string())
            },
            Message::LibraryLockActionPersisted,
        ),
        LockAction::Takeover => iced::Task::perform(
            async move {
                shoebox_client::library_state::http_request_takeover(
                    &client,
                    &server_url,
                    &detail.variant_id,
                )
                .await
                .map_err(|error| error.to_string())
            },
            Message::LibraryLockActionPersisted,
        ),
    }
}

//! Library screen — debug "catalog state" view.

use iced::widget::{column, container, row, text};
use iced::Element;

use crate::app_state::ConnectionStatus;
use crate::screens::Message;

/// View state owned by `main.rs`: the latest stats loaded from the
/// replica. Refreshed on `Message::ReplicaSyncFinished`.
#[derive(Debug, Default, Clone)]
pub struct LibraryStats {
    pub schema_version: i64,
    pub photo_count: i64,
    pub folder_count: i64,
    pub active_user_display_name: String,
    /// Latest sync frame number; updated by `main.rs` after each
    /// `Replica::sync()` call. Used for log signaling only.
    pub frame_no: u64,
}

/// The subset of `AppState` this screen actually reads, passed by the
/// caller so the screen module doesn't need to hold a read guard across
/// the Element's lifetime.
#[must_use]
#[allow(clippy::similar_names)]
pub fn view<'a>(
    connection_status: ConnectionStatus,
    server_url: &'a str,
    file_storage_warning: bool,
    stats: &'a LibraryStats,
) -> Element<'a, Message> {
    let connection_line =
        text(format!("Connection: {connection_status:?} ({server_url})")).size(16);

    let offline_banner: Element<Message> = if connection_status == ConnectionStatus::Offline {
        text("⚠ Offline — reading from local replica; writes disabled").into()
    } else {
        row![].into()
    };

    let file_storage_banner: Element<Message> = if file_storage_warning {
        text(
            "⚠ Cert is stored in a file (you chose this when the keychain failed). \
             Re-enroll on a working keychain to upgrade.",
        )
        .into()
    } else {
        row![].into()
    };

    let stats_block = column![
        text(format!("Schema version: {}", stats.schema_version)),
        text(format!("Photos: {}", stats.photo_count)),
        text(format!("Folders: {}", stats.folder_count)),
        text(format!("Active user: {}", stats.active_user_display_name)),
    ]
    .spacing(4);

    container(
        column![
            text("shoebox").size(28),
            connection_line,
            offline_banner,
            file_storage_banner,
            stats_block,
        ]
        .spacing(12)
        .padding(20),
    )
    .into()
}

/// Helper for `main.rs::update()` — populates a fresh `LibraryStats`
/// from a libsql `Connection`. Reads `schema_version` from
/// `_schema_migrations` (max version), `photo_count` and `folder_count`
/// from their respective tables, and `active_user_display_name` from
/// the `users` row matching `active_user_id`.
///
/// # Errors
/// Returns an error on query failure.
pub async fn load_stats(
    conn: &libsql::Connection,
    active_user_id: Option<&str>,
) -> Result<LibraryStats, anyhow::Error> {
    let mut stats = LibraryStats::default();

    let mut rows = conn
        .query(
            "SELECT COALESCE(MAX(version), 0) FROM _schema_migrations",
            (),
        )
        .await?;
    if let Some(r) = rows.next().await? {
        stats.schema_version = r.get(0)?;
    }

    let mut rows = conn.query("SELECT COUNT(*) FROM photos", ()).await?;
    if let Some(r) = rows.next().await? {
        stats.photo_count = r.get(0)?;
    }

    let mut rows = conn.query("SELECT COUNT(*) FROM folders", ()).await?;
    if let Some(r) = rows.next().await? {
        stats.folder_count = r.get(0)?;
    }

    if let Some(user_id) = active_user_id {
        let mut rows = conn
            .query("SELECT display_name FROM users WHERE id = ?1", [user_id])
            .await?;
        if let Some(r) = rows.next().await? {
            stats.active_user_display_name = r.get(0)?;
        }
    }
    Ok(stats)
}

//! Helper used during startup to surface initial catalog stats. The
//! library screen itself is rendered by `screens::library_view`.

/// Stats loaded once at startup so logs / future telemetry can surface
/// "what we synced". Not displayed in the UI as of Plan 1.4b.
#[derive(Debug, Default, Clone)]
pub struct LibraryStats {
    pub schema_version: i64,
    pub photo_count: i64,
    pub folder_count: i64,
    pub active_user_display_name: String,
    pub frame_no: u64,
}

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

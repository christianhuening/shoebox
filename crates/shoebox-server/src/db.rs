//! libSQL database lifecycle: open, run migrations.

use anyhow::{anyhow, Context, Result};
use include_dir::{include_dir, Dir};
use libsql::{Builder, Connection, Database};
use std::path::Path;

static MIGRATIONS_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/migrations");

pub struct Db {
    pub database: Database,
}

impl Db {
    /// Open (creating if absent) the libSQL database at the given path
    /// and apply all pending migrations.
    pub async fn open(path: &Path) -> Result<Self> {
        let database = Builder::new_local(path)
            .build()
            .await
            .map_err(|e| anyhow!("failed to open libSQL database at {path:?}: {e}"))?;

        let conn = database
            .connect()
            .map_err(|e| anyhow!("failed to connect to libSQL: {e}"))?;
        apply_migrations(&conn).await?;
        Ok(Self { database })
    }

    pub fn connect(&self) -> Result<Connection> {
        self.database
            .connect()
            .map_err(|e| anyhow!("failed to connect to libSQL: {e}"))
    }
}

async fn apply_migrations(conn: &Connection) -> Result<()> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS _schema_migrations (
            version    INTEGER PRIMARY KEY,
            applied_at INTEGER NOT NULL
        )",
        (),
    )
    .await
    .context("creating _schema_migrations")?;

    let mut entries: Vec<_> = MIGRATIONS_DIR
        .files()
        .filter(|f| {
            f.path()
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e == "sql")
                .unwrap_or(false)
        })
        .collect();
    entries.sort_by_key(|f| f.path().file_name().map(|n| n.to_os_string()));

    for file in entries {
        let name = file
            .path()
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| anyhow!("bad migration filename"))?;
        let version: i64 = name
            .split('_')
            .next()
            .ok_or_else(|| anyhow!("migration {name} missing numeric prefix"))?
            .parse()
            .map_err(|_| anyhow!("migration {name} has non-numeric prefix"))?;

        let mut rows = conn
            .query(
                "SELECT 1 FROM _schema_migrations WHERE version = ?1",
                [version],
            )
            .await?;
        if rows.next().await?.is_some() {
            continue;
        }

        let sql = file
            .contents_utf8()
            .ok_or_else(|| anyhow!("migration {name} not UTF-8"))?;
        tracing::info!(event = "migration.apply", version, name, "applying migration");
        conn.execute_batch(sql).await.with_context(|| {
            format!("applying migration {name} (version {version})")
        })?;
        let now_ms = chrono_now_ms();
        conn.execute(
            "INSERT INTO _schema_migrations (version, applied_at) VALUES (?1, ?2)",
            (version, now_ms),
        )
        .await?;
    }

    Ok(())
}

fn chrono_now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn opens_and_applies_migrations() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("catalog.db");
        let db = Db::open(&path).await.unwrap();
        let conn = db.connect().unwrap();

        // _schema_migrations exists and contains version 1.
        let mut rows = conn
            .query("SELECT version FROM _schema_migrations ORDER BY version", ())
            .await
            .unwrap();
        let row = rows.next().await.unwrap().expect("at least one migration");
        let version: i64 = row.get(0).unwrap();
        assert_eq!(version, 1);

        // Reopening is a no-op: migrations are idempotent.
        let db2 = Db::open(&path).await.unwrap();
        drop(db2);
    }

    #[tokio::test]
    async fn migration_0005_creates_keyword_tables() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("catalog.db");
        let db = Db::open(&path).await.unwrap();
        let conn = db.connect().unwrap();

        for table in ["keywords", "photo_keywords"] {
            let mut rows = conn
                .query(
                    "SELECT name FROM sqlite_master WHERE type='table' AND name = ?1",
                    [table],
                )
                .await
                .unwrap();
            assert!(
                rows.next().await.unwrap().is_some(),
                "table {table} should exist after migration 0005"
            );
        }
    }

    #[tokio::test]
    async fn migration_0004_creates_variant_user_state() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("catalog.db");
        let db = Db::open(&path).await.unwrap();
        let conn = db.connect().unwrap();

        let mut rows = conn
            .query(
                "SELECT name FROM sqlite_master WHERE type='table' AND name='variant_user_state'",
                (),
            )
            .await
            .unwrap();
        assert!(rows.next().await.unwrap().is_some());
    }

    #[tokio::test]
    async fn migration_0003_creates_variant_tables() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("catalog.db");
        let db = Db::open(&path).await.unwrap();
        let conn = db.connect().unwrap();

        for table in ["variants", "develop_locks"] {
            let mut rows = conn
                .query(
                    "SELECT name FROM sqlite_master WHERE type='table' AND name = ?1",
                    [table],
                )
                .await
                .unwrap();
            assert!(
                rows.next().await.unwrap().is_some(),
                "table {table} should exist after migration 0003"
            );
        }
    }

    #[tokio::test]
    async fn migration_0002_creates_file_tables() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("catalog.db");
        let db = Db::open(&path).await.unwrap();
        let conn = db.connect().unwrap();

        for table in ["folders", "photos", "photo_files"] {
            let mut rows = conn
                .query(
                    "SELECT name FROM sqlite_master WHERE type='table' AND name = ?1",
                    [table],
                )
                .await
                .unwrap();
            assert!(
                rows.next().await.unwrap().is_some(),
                "table {table} should exist after migration 0002"
            );
        }
    }

    #[tokio::test]
    async fn migration_0001_creates_identity_tables() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("catalog.db");
        let db = Db::open(&path).await.unwrap();
        let conn = db.connect().unwrap();

        for table in ["config", "users", "sessions", "revoked_certs"] {
            let mut rows = conn
                .query(
                    "SELECT name FROM sqlite_master WHERE type='table' AND name = ?1",
                    [table],
                )
                .await
                .unwrap();
            assert!(
                rows.next().await.unwrap().is_some(),
                "table {table} should exist after migration 0001"
            );
        }
    }
}

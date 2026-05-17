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
            .map_err(|e| anyhow!("failed to open libSQL database at {}: {e}", path.display()))?;

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

    /// Insert a row into `revoked_certs`. `serial_hex` is the lowercase-hex
    /// serial number of the leaf cert being revoked.
    pub async fn insert_revoked_cert(
        &self,
        serial_hex: &str,
        reason: Option<&str>,
        revoked_by: Option<&str>,
    ) -> anyhow::Result<()> {
        let conn = self.connect()?;
        let now_ms = now_ms();
        conn.execute(
            "INSERT INTO revoked_certs (serial_number, revoked_at, reason, revoked_by) \
             VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT(serial_number) DO NOTHING",
            (
                serial_hex.to_string(),
                now_ms,
                reason.map(str::to_string),
                revoked_by.map(str::to_string),
            ),
        )
        .await?;
        Ok(())
    }

    /// Return true if the given hex serial appears in `revoked_certs`.
    pub async fn is_serial_revoked(&self, serial_hex: &str) -> anyhow::Result<bool> {
        let conn = self.connect()?;
        let mut rows = conn
            .query(
                "SELECT 1 FROM revoked_certs WHERE serial_number = ?1",
                [serial_hex],
            )
            .await?;
        Ok(rows.next().await?.is_some())
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
                .is_some_and(|e| e == "sql")
        })
        .collect();
    entries.sort_by_key(|f| f.path().file_name().map(std::ffi::OsStr::to_os_string));

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
        tracing::info!(
            event = "migration.apply",
            version,
            name,
            "applying migration"
        );
        conn.execute_batch(sql)
            .await
            .with_context(|| format!("applying migration {name} (version {version})"))?;
        let now_ms = now_ms();
        conn.execute(
            "INSERT INTO _schema_migrations (version, applied_at) VALUES (?1, ?2)",
            (version, now_ms),
        )
        .await?;
    }

    Ok(())
}

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
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
            .query(
                "SELECT version FROM _schema_migrations ORDER BY version",
                (),
            )
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
    async fn migration_0006_creates_collection_tables() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("catalog.db");
        let db = Db::open(&path).await.unwrap();
        let conn = db.connect().unwrap();

        for table in ["collections", "collection_members"] {
            let mut rows = conn
                .query(
                    "SELECT name FROM sqlite_master WHERE type='table' AND name = ?1",
                    [table],
                )
                .await
                .unwrap();
            assert!(
                rows.next().await.unwrap().is_some(),
                "table {table} should exist after migration 0006"
            );
        }
    }

    #[tokio::test]
    async fn all_six_migrations_applied_in_order() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("catalog.db");
        let db = Db::open(&path).await.unwrap();
        let conn = db.connect().unwrap();

        let mut rows = conn
            .query(
                "SELECT version FROM _schema_migrations ORDER BY version",
                (),
            )
            .await
            .unwrap();
        let mut versions = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            versions.push(row.get::<i64>(0).unwrap());
        }
        assert_eq!(versions, vec![1, 2, 3, 4, 5, 6]);
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

    #[tokio::test]
    async fn revoked_serial_round_trips() {
        let tmp = TempDir::new().unwrap();
        let db = Db::open(&tmp.path().join("catalog.db")).await.unwrap();
        assert!(!db.is_serial_revoked("abc123").await.unwrap());
        db.insert_revoked_cert("abc123", Some("test"), None).await.unwrap();
        assert!(db.is_serial_revoked("abc123").await.unwrap());
        // Idempotent: inserting again does not error.
        db.insert_revoked_cert("abc123", Some("test"), None).await.unwrap();
    }
}

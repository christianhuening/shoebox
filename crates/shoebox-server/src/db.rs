//! libSQL database lifecycle: open, run migrations.
//!
//! As of sub-1-3-5, `Db::open` connects to the embedded `sqld` subprocess
//! via libsql's remote backend (Hrana HTTP over loopback) rather than
//! opening a local `SQLite` file directly. `sqld` is the single backing
//! store for both server-side writes (this Db) and client-side replicas
//! (which sync from sqld's gRPC port through the mTLS proxy).

use anyhow::{anyhow, Context, Result};
use include_dir::{include_dir, Dir};
use libsql::{Builder, Connection, Database};

static MIGRATIONS_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/migrations");

pub struct Db {
    pub database: Database,
}

impl Db {
    /// Open a libsql `Database` connected over HTTP to the loopback
    /// `sqld` subprocess at `sqld_http_url`, and apply all pending
    /// migrations through that connection. `sqld` is the single backing
    /// store for both server-side writes and client-side replicas.
    pub async fn open(sqld_http_url: &str) -> Result<Self> {
        let database = Builder::new_remote(sqld_http_url.to_string(), String::new())
            .build()
            .await
            .map_err(|e| {
                anyhow!("failed to open libSQL remote database at {sqld_http_url}: {e}")
            })?;

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

    /// Try to acquire the develop lock on `variant_id` for `session_id` /
    /// `user_id` with a TTL of `ttl_ms` milliseconds.
    ///
    /// Returns `true` if the lock was newly acquired, `false` if another
    /// session already holds it (insert is a no-op via `ON CONFLICT DO
    /// NOTHING`).
    ///
    /// # Errors
    /// Returns an error if the database connection or insert fails.
    pub async fn lock_acquire(
        &self,
        variant_id: &str,
        session_id: &str,
        user_id: &str,
        ttl_ms: i64,
    ) -> anyhow::Result<bool> {
        let conn = self.connect()?;
        let acquired_at = now_ms();
        let expires_at = acquired_at + ttl_ms;
        let rows_affected = conn
            .execute(
                "INSERT INTO develop_locks \
                 (variant_id, session_id, user_id, acquired_at, expires_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5) \
                 ON CONFLICT(variant_id) DO NOTHING",
                (
                    variant_id.to_string(),
                    session_id.to_string(),
                    user_id.to_string(),
                    acquired_at,
                    expires_at,
                ),
            )
            .await?;
        Ok(rows_affected > 0)
    }

    /// Extend the develop lock on `variant_id` held by `session_id` by
    /// `ttl_ms` milliseconds from now. Returns `true` if a matching lock row
    /// was found and updated, `false` otherwise (lock expired, never held,
    /// or held by a different session).
    ///
    /// # Errors
    /// Returns an error if the database connection or update fails.
    pub async fn lock_heartbeat(
        &self,
        variant_id: &str,
        session_id: &str,
        ttl_ms: i64,
    ) -> anyhow::Result<bool> {
        let conn = self.connect()?;
        let expires_at = now_ms() + ttl_ms;
        let rows_affected = conn
            .execute(
                "UPDATE develop_locks SET expires_at = ?1 \
                 WHERE variant_id = ?2 AND session_id = ?3",
                (expires_at, variant_id.to_string(), session_id.to_string()),
            )
            .await?;
        Ok(rows_affected > 0)
    }

    /// Release the develop lock on `variant_id` if it is held by
    /// `session_id`. Returns `true` if a row was deleted, `false` if no
    /// matching lock existed.
    ///
    /// # Errors
    /// Returns an error if the database connection or delete fails.
    pub async fn lock_release(&self, variant_id: &str, session_id: &str) -> anyhow::Result<bool> {
        let conn = self.connect()?;
        let rows_affected = conn
            .execute(
                "DELETE FROM develop_locks WHERE variant_id = ?1 AND session_id = ?2",
                (variant_id.to_string(), session_id.to_string()),
            )
            .await?;
        Ok(rows_affected > 0)
    }

    /// Record a takeover request on the develop lock for `variant_id` by
    /// `requesting_user_id`. The update is conditional on no takeover
    /// already being pending. Returns `true` if a takeover was newly
    /// recorded, `false` if no lock exists or a takeover was already
    /// pending.
    ///
    /// # Errors
    /// Returns an error if the database connection or update fails.
    pub async fn lock_request_takeover(
        &self,
        variant_id: &str,
        requesting_user_id: &str,
    ) -> anyhow::Result<bool> {
        let conn = self.connect()?;
        let requested_at = now_ms();
        let rows_affected = conn
            .execute(
                "UPDATE develop_locks \
                 SET takeover_requested_by = ?1, takeover_requested_at = ?2 \
                 WHERE variant_id = ?3 AND takeover_requested_by IS NULL",
                (
                    requesting_user_id.to_string(),
                    requested_at,
                    variant_id.to_string(),
                ),
            )
            .await?;
        Ok(rows_affected > 0)
    }

    /// Delete all develop-lock rows whose `expires_at` is in the past.
    /// Returns the number of rows removed.
    ///
    /// # Errors
    /// Returns an error if the database connection or delete fails.
    pub async fn lock_release_expired(&self) -> anyhow::Result<usize> {
        let conn = self.connect()?;
        let now = now_ms();
        let rows_affected = conn
            .execute("DELETE FROM develop_locks WHERE expires_at < ?1", [now])
            .await?;
        Ok(usize::try_from(rows_affected).unwrap_or(usize::MAX))
    }

    /// Return the `user_id` of the current develop-lock holder for
    /// `variant_id`, or `None` if no lock exists.
    ///
    /// # Errors
    /// Returns an error if the database connection, query, or column
    /// extraction fails.
    pub async fn lock_holder(&self, variant_id: &str) -> anyhow::Result<Option<String>> {
        let conn = self.connect()?;
        let mut rows = conn
            .query(
                "SELECT user_id FROM develop_locks WHERE variant_id = ?1",
                [variant_id],
            )
            .await?;
        let holder = match rows.next().await? {
            Some(row) => Some(row.get::<String>(0)?),
            None => None,
        };
        Ok(holder)
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

    #[tokio::test]
    async fn opens_and_applies_migrations() {
        let test_db = crate::test_helpers::TestDb::start().await;
        let conn = test_db.db.connect().unwrap();

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

        // Reopening against the same sqld is a no-op: migrations are idempotent.
        let db2 = Db::open(&test_db.embedded.local_url).await.unwrap();
        drop(db2);
    }

    #[tokio::test]
    async fn migration_0006_creates_collection_tables() {
        let test_db = crate::test_helpers::TestDb::start().await;
        let conn = test_db.db.connect().unwrap();

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
    async fn all_migrations_applied_in_order() {
        let test_db = crate::test_helpers::TestDb::start().await;
        let conn = test_db.db.connect().unwrap();

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
        assert_eq!(versions, vec![1, 2, 3, 4, 5, 6, 7]);
    }

    #[tokio::test]
    async fn migration_0007_partial_index_rejects_duplicate_root_keywords() {
        let test_db = crate::test_helpers::TestDb::start().await;
        let conn = test_db.db.connect().unwrap();

        conn.execute(
            "INSERT INTO keywords(id, parent_id, name, created_at) VALUES('k1', NULL, 'trees', 0)",
            (),
        )
        .await
        .unwrap();

        // Same name at root level must now be rejected.
        let duplicate = conn
            .execute(
                "INSERT INTO keywords(id, parent_id, name, created_at) \
                 VALUES('k2', NULL, 'trees', 0)",
                (),
            )
            .await;
        assert!(
            duplicate.is_err(),
            "second root-level 'trees' should violate the partial unique index"
        );

        // Same name under a non-null parent still works — the partial
        // index only applies to root keywords.
        conn.execute(
            "INSERT INTO keywords(id, parent_id, name, created_at) VALUES('p1', NULL, 'nature', 0)",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO keywords(id, parent_id, name, created_at) VALUES('c1', 'p1', 'trees', 0)",
            (),
        )
        .await
        .expect("nested 'trees' under a parent keyword is fine");
    }

    #[tokio::test]
    async fn migration_0005_creates_keyword_tables() {
        let test_db = crate::test_helpers::TestDb::start().await;
        let conn = test_db.db.connect().unwrap();

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
        let test_db = crate::test_helpers::TestDb::start().await;
        let conn = test_db.db.connect().unwrap();

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
        let test_db = crate::test_helpers::TestDb::start().await;
        let conn = test_db.db.connect().unwrap();

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
        let test_db = crate::test_helpers::TestDb::start().await;
        let conn = test_db.db.connect().unwrap();

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
        let test_db = crate::test_helpers::TestDb::start().await;
        let conn = test_db.db.connect().unwrap();

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
        let test_db = crate::test_helpers::TestDb::start().await;
        let db = test_db.db.clone();
        assert!(!db.is_serial_revoked("abc123").await.unwrap());
        db.insert_revoked_cert("abc123", Some("test"), None)
            .await
            .unwrap();
        assert!(db.is_serial_revoked("abc123").await.unwrap());
        // Idempotent: inserting again does not error.
        db.insert_revoked_cert("abc123", Some("test"), None)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn lock_lifecycle_roundtrips() {
        let test_db = crate::test_helpers::TestDb::start().await;
        let db = test_db.db.clone();
        let conn = db.connect().unwrap();

        // Set up FK chain: users, session, photo, variant.
        let setup_timestamp = 1_000_000_i64;
        conn.execute(
            "INSERT INTO users (id, display_name, created_at) VALUES ('u1', 'Alice', ?1)",
            [setup_timestamp],
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO users (id, display_name, created_at) VALUES ('u2', 'Bob', ?1)",
            [setup_timestamp],
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO sessions (id, user_id, client_machine_id, established_at, last_active_at) \
             VALUES ('s1', 'u1', 'm1', ?1, ?1)",
            [setup_timestamp],
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO photos (id, file_size, file_format, imported_at) \
             VALUES ('h1', 100, 'PEF', ?1)",
            [setup_timestamp],
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO variants (id, photo_id, variant_index, created_by, created_at, \
             develop_settings_json, develop_settings_version, develop_updated_at, develop_updated_by) \
             VALUES ('v1', 'h1', 0, 'u1', ?1, '{}', 1, ?1, 'u1')",
            [setup_timestamp],
        )
        .await
        .unwrap();

        assert!(db.lock_acquire("v1", "s1", "u1", 60_000).await.unwrap());
        // Re-acquire by same session: false (already held).
        assert!(!db.lock_acquire("v1", "s1", "u1", 60_000).await.unwrap());
        assert!(db.lock_heartbeat("v1", "s1", 120_000).await.unwrap());
        assert!(db.lock_request_takeover("v1", "u2").await.unwrap());
        // Second takeover by same user: false (already set).
        assert!(!db.lock_request_takeover("v1", "u2").await.unwrap());
        assert!(db.lock_release("v1", "s1").await.unwrap());
        // After release, holder is None.
        assert_eq!(db.lock_holder("v1").await.unwrap(), None);
    }
}

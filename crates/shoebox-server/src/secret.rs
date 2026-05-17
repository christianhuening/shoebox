//! Shared catalog secret used during enrollment.
//!
//! On first launch, a random 24-byte secret is generated, argon2id-hashed,
//! and the hash is persisted to the catalog `config` table under the key
//! `enrollment_secret_hash`. The plaintext is printed once to the log and
//! never stored on disk. Operators can override via the `SHOEBOX_SECRET`
//! env var at startup.

use anyhow::{anyhow, Context, Result};
use argon2::password_hash::{
    rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString,
};
use argon2::Argon2;
use libsql::Connection;

const CONFIG_KEY: &str = "enrollment_secret_hash";

/// Verify a presented plaintext against the stored argon2id hash.
pub async fn verify(conn: &Connection, presented: &str) -> Result<bool> {
    let mut rows = conn
        .query("SELECT value FROM config WHERE key = ?1", [CONFIG_KEY])
        .await
        .context("reading enrollment_secret_hash")?;
    let Some(row) = rows.next().await? else {
        return Ok(false);
    };
    let hash_str: String = row.get(0)?;
    let parsed =
        PasswordHash::new(&hash_str).map_err(|e| anyhow!("malformed stored secret hash: {e}"))?;
    Ok(Argon2::default()
        .verify_password(presented.as_bytes(), &parsed)
        .is_ok())
}

/// Ensure a secret hash is present in the catalog. If one isn't, either
/// use the `SHOEBOX_SECRET` env var (if set), or generate a random secret.
/// The plaintext (whether supplied or generated) is returned so the caller
/// can log it exactly once at bootstrap.
pub async fn ensure_present(conn: &Connection) -> Result<EnsureOutcome> {
    let mut rows = conn
        .query("SELECT 1 FROM config WHERE key = ?1", [CONFIG_KEY])
        .await?;
    if rows.next().await?.is_some() {
        return Ok(EnsureOutcome::AlreadySet);
    }

    let plaintext = match std::env::var("SHOEBOX_SECRET") {
        Ok(v) if !v.is_empty() => v,
        _ => generate_random_secret(),
    };

    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(plaintext.as_bytes(), &salt)
        .map_err(|e| anyhow!("argon2 hash: {e}"))?
        .to_string();

    conn.execute(
        "INSERT INTO config (key, value) VALUES (?1, ?2)",
        (CONFIG_KEY, hash),
    )
    .await
    .context("inserting enrollment_secret_hash")?;

    Ok(EnsureOutcome::Generated { plaintext })
}

fn generate_random_secret() -> String {
    use rand::{distributions::Alphanumeric, Rng};
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(24)
        .map(char::from)
        .collect()
}

pub enum EnsureOutcome {
    AlreadySet,
    Generated { plaintext: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use tempfile::TempDir;

    #[tokio::test]
    async fn first_call_generates_and_persists() {
        let tmp = TempDir::new().unwrap();
        let db = Db::open(&tmp.path().join("catalog.db")).await.unwrap();
        let conn = db.connect().unwrap();

        match ensure_present(&conn).await.unwrap() {
            EnsureOutcome::Generated { plaintext } => assert_eq!(plaintext.len(), 24),
            EnsureOutcome::AlreadySet => panic!("should generate on fresh DB"),
        }

        match ensure_present(&conn).await.unwrap() {
            EnsureOutcome::AlreadySet => {}
            EnsureOutcome::Generated { .. } => panic!("should be idempotent"),
        }
    }

    #[tokio::test]
    async fn verify_accepts_correct_secret() {
        let tmp = TempDir::new().unwrap();
        let db = Db::open(&tmp.path().join("catalog.db")).await.unwrap();
        let conn = db.connect().unwrap();
        let plaintext = match ensure_present(&conn).await.unwrap() {
            EnsureOutcome::Generated { plaintext } => plaintext,
            EnsureOutcome::AlreadySet => panic!(),
        };

        assert!(verify(&conn, &plaintext).await.unwrap());
        assert!(!verify(&conn, "wrong-secret").await.unwrap());
    }
}

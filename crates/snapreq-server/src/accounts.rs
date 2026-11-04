//! Local accounts, browser sessions and API tokens (PLAN P8.1, ADR-023).
//!
//! Passwords are stored as Argon2id PHC strings. Session and API token secrets are 256 random
//! bits; only their SHA-256 digests are stored, so a copy of the database cannot be used to log
//! in. Secrets are returned once, when they are created, and never logged.

use std::sync::OnceLock;
use std::time::Duration;

use argon2::{Argon2, PasswordHasher, PasswordVerifier};
use base64::Engine as _;
use rusqlite::{OptionalExtension, params};
use serde::Serialize;

use crate::store::{Store, db_err, now_ms};
use crate::{Error, Result};

/// Prefix of API tokens, so they are recognisable (and findable by secret scanners).
pub const TOKEN_PREFIX: &str = "snapreq_pat_";

/// Shortest accepted password, in characters.
pub const MIN_PASSWORD_CHARS: usize = 8;
/// Longest accepted password, in bytes (hashing cost grows with the length).
const MAX_PASSWORD_BYTES: usize = 1024;

/// An account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct User {
    pub id: i64,
    pub username: String,
    /// Unix milliseconds.
    pub created_at: i64,
}

/// An API token as listed (the secret itself is shown only when it is created).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct ApiToken {
    pub id: i64,
    pub name: String,
    /// The last four characters of the token, to tell tokens apart.
    pub hint: String,
    pub created_at: i64,
    /// Unix milliseconds; `null`: never expires.
    pub expires_at: Option<i64>,
    pub last_used_at: Option<i64>,
}

/// A user name: 1–64 letters, digits, `_`, `-`, `.` or `@`.
pub fn check_username(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.chars().count() <= 64
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '@'));
    if ok {
        Ok(())
    } else {
        Err(Error::Account(
            "a user name has 1 to 64 letters, digits, `_`, `-`, `.` or `@`".into(),
        ))
    }
}

/// A password: at least [`MIN_PASSWORD_CHARS`] characters, at most 1024 bytes.
pub fn check_password(password: &str) -> Result<()> {
    if password.chars().count() < MIN_PASSWORD_CHARS {
        return Err(Error::Account(format!(
            "a password needs at least {MIN_PASSWORD_CHARS} characters"
        )));
    }
    if password.len() > MAX_PASSWORD_BYTES {
        return Err(Error::Account(format!(
            "a password has at most {MAX_PASSWORD_BYTES} bytes"
        )));
    }
    Ok(())
}

/// 256 random bits, base64url without padding.
fn random_secret() -> Result<String> {
    use ring::rand::SecureRandom as _;
    let mut bytes = [0u8; 32];
    ring::rand::SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| Error::Account("the system random generator failed".into()))?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}

/// The stored form of a session or token secret.
fn digest(secret: &str) -> Vec<u8> {
    ring::digest::digest(&ring::digest::SHA256, secret.as_bytes())
        .as_ref()
        .to_vec()
}

fn hash_password(password: &str) -> Result<String> {
    Argon2::default()
        .hash_password(password.as_bytes())
        .map(|h| h.to_string())
        .map_err(|e| Error::Account(format!("cannot hash the password: {e}")))
}

fn verify_password(password: &str, hash: &str) -> bool {
    Argon2::default()
        .verify_password(password.as_bytes(), hash)
        .is_ok()
}

/// A hash to verify against when the user does not exist, so that a login takes about as
/// long for unknown users as for wrong passwords.
fn dummy_hash() -> &'static str {
    static HASH: OnceLock<String> = OnceLock::new();
    HASH.get_or_init(|| hash_password("snapreq-no-such-user").unwrap_or_default())
}

/// Runs CPU-heavy hashing off the async threads (and outside the database lock).
async fn cpu<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| Error::Account(format!("hashing task failed: {e}")))
}

fn user_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<User> {
    Ok(User {
        id: r.get(0)?,
        username: r.get(1)?,
        created_at: r.get(2)?,
    })
}

fn token_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ApiToken> {
    Ok(ApiToken {
        id: r.get(0)?,
        name: r.get(1)?,
        hint: r.get(2)?,
        created_at: r.get(3)?,
        expires_at: r.get(4)?,
        last_used_at: r.get(5)?,
    })
}

fn millis(d: Duration) -> i64 {
    i64::try_from(d.as_millis()).unwrap_or(i64::MAX)
}

impl Store {
    /// Creates an account.
    pub async fn create_user(&self, username: &str, password: &str) -> Result<User> {
        check_username(username)?;
        check_password(password)?;
        let password = password.to_owned();
        let hash = cpu(move || hash_password(&password)).await??;
        let username = username.to_owned();
        self.with(move |c| {
            let exists: bool = c
                .query_row(
                    "SELECT EXISTS (SELECT 1 FROM users WHERE username = ?1)",
                    [&username],
                    |r| r.get(0),
                )
                .map_err(db_err)?;
            if exists {
                return Err(Error::Account(format!("user `{username}` already exists")));
            }
            let created_at = now_ms();
            c.execute(
                "INSERT INTO users (username, password_hash, created_at) VALUES (?1, ?2, ?3)",
                params![username, hash, created_at],
            )
            .map_err(db_err)?;
            Ok(User {
                id: c.last_insert_rowid(),
                username,
                created_at,
            })
        })
        .await
    }

    /// Every account, by name.
    pub async fn users(&self) -> Result<Vec<User>> {
        self.with(|c| {
            let mut stmt = c
                .prepare("SELECT id, username, created_at FROM users ORDER BY username")
                .map_err(db_err)?;
            let rows = stmt.query_map([], user_row).map_err(db_err)?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(db_err)
        })
        .await
    }

    /// The number of accounts.
    pub async fn user_count(&self) -> Result<usize> {
        self.with(|c| {
            c.query_row("SELECT count(*) FROM users", [], |r| r.get::<_, i64>(0))
                .map(|n| usize::try_from(n).unwrap_or(0))
                .map_err(db_err)
        })
        .await
    }

    /// Deletes an account with its sessions, tokens, history and runs. `false`: no such user.
    pub async fn delete_user(&self, username: &str) -> Result<bool> {
        let username = username.to_owned();
        self.with(move |c| {
            c.execute("DELETE FROM users WHERE username = ?1", [username])
                .map(|n| n > 0)
                .map_err(db_err)
        })
        .await
    }

    /// Sets a user's password and ends their sessions, except `keep` (the session that
    /// changed it). API tokens stay valid. `false`: no such user.
    pub async fn set_password(
        &self,
        username: &str,
        password: &str,
        keep: Option<&str>,
    ) -> Result<bool> {
        check_password(password)?;
        let password = password.to_owned();
        let hash = cpu(move || hash_password(&password)).await??;
        let username = username.to_owned();
        let keep = keep.map(digest).unwrap_or_default();
        self.with(move |c| {
            let tx = c.transaction().map_err(db_err)?;
            let id: Option<i64> = tx
                .query_row(
                    "SELECT id FROM users WHERE username = ?1",
                    [&username],
                    |r| r.get(0),
                )
                .optional()
                .map_err(db_err)?;
            let Some(id) = id else { return Ok(false) };
            tx.execute(
                "UPDATE users SET password_hash = ?1 WHERE id = ?2",
                params![hash, id],
            )
            .map_err(db_err)?;
            tx.execute(
                "DELETE FROM sessions WHERE user_id = ?1 AND token_hash != ?2",
                params![id, keep],
            )
            .map_err(db_err)?;
            tx.commit().map_err(db_err)?;
            Ok(true)
        })
        .await
    }

    /// Checks a user's password. Takes about as long whether or not the user exists.
    pub async fn check_login(&self, username: &str, password: &str) -> Result<Option<User>> {
        let name = username.to_owned();
        let found: Option<(User, String)> = self
            .with(move |c| {
                c.query_row(
                    "SELECT id, username, created_at, password_hash FROM users WHERE username = ?1",
                    [name],
                    |r| Ok((user_row(r)?, r.get(3)?)),
                )
                .optional()
                .map_err(db_err)
            })
            .await?;
        let password = password.to_owned();
        let hash = found
            .as_ref()
            .map_or_else(|| dummy_hash().to_owned(), |(_, h)| h.clone());
        let ok = cpu(move || verify_password(&password, &hash)).await?;
        Ok(found.filter(|_| ok).map(|(u, _)| u))
    }

    /// Starts a session for a user; returns the secret for the session cookie.
    pub async fn create_session(&self, user_id: i64, ttl: Duration) -> Result<String> {
        let secret = random_secret()?;
        let hash = digest(&secret);
        self.with(move |c| {
            let now = now_ms();
            c.execute("DELETE FROM sessions WHERE expires_at <= ?1", [now])
                .map_err(db_err)?;
            c.execute(
                "INSERT INTO sessions (token_hash, user_id, created_at, expires_at) VALUES (?1, ?2, ?3, ?4)",
                params![hash, user_id, now, now.saturating_add(millis(ttl))],
            )
            .map_err(db_err)?;
            Ok(())
        })
        .await?;
        Ok(secret)
    }

    /// The user of an unexpired session.
    pub async fn session_user(&self, secret: &str) -> Result<Option<User>> {
        let hash = digest(secret);
        self.with(move |c| {
            c.query_row(
                "SELECT u.id, u.username, u.created_at FROM sessions s JOIN users u ON u.id = s.user_id
                 WHERE s.token_hash = ?1 AND s.expires_at > ?2",
                params![hash, now_ms()],
                user_row,
            )
            .optional()
            .map_err(db_err)
        })
        .await
    }

    /// Ends a session (logout).
    pub async fn delete_session(&self, secret: &str) -> Result<()> {
        let hash = digest(secret);
        self.with(move |c| {
            c.execute("DELETE FROM sessions WHERE token_hash = ?1", [hash])
                .map(drop)
                .map_err(db_err)
        })
        .await
    }

    /// Creates an API token; returns it with its secret (shown once).
    pub async fn create_token(
        &self,
        user_id: i64,
        name: &str,
        ttl: Option<Duration>,
    ) -> Result<(ApiToken, String)> {
        let name = name.trim().to_owned();
        if name.is_empty() || name.chars().count() > 100 {
            return Err(Error::Account(
                "a token name has 1 to 100 characters".into(),
            ));
        }
        let secret = format!("{TOKEN_PREFIX}{}", random_secret()?);
        let hash = digest(&secret);
        let hint: String = secret[secret.len() - 4..].to_owned();
        let token = self
            .with(move |c| {
                let created_at = now_ms();
                let expires_at = ttl.map(|t| created_at.saturating_add(millis(t)));
                c.execute(
                    "INSERT INTO api_tokens (user_id, name, token_hash, hint, created_at, expires_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![user_id, name, hash, hint, created_at, expires_at],
                )
                .map_err(db_err)?;
                Ok(ApiToken {
                    id: c.last_insert_rowid(),
                    name,
                    hint,
                    created_at,
                    expires_at,
                    last_used_at: None,
                })
            })
            .await?;
        Ok((token, secret))
    }

    /// A user's API tokens, newest first.
    pub async fn tokens(&self, user_id: i64) -> Result<Vec<ApiToken>> {
        self.with(move |c| {
            let mut stmt = c
                .prepare(
                    "SELECT id, name, hint, created_at, expires_at, last_used_at FROM api_tokens
                     WHERE user_id = ?1 ORDER BY id DESC",
                )
                .map_err(db_err)?;
            let rows = stmt.query_map([user_id], token_row).map_err(db_err)?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(db_err)
        })
        .await
    }

    /// Revokes one of a user's tokens. `false`: the user has no such token.
    pub async fn delete_token(&self, user_id: i64, id: i64) -> Result<bool> {
        self.with(move |c| {
            c.execute(
                "DELETE FROM api_tokens WHERE id = ?1 AND user_id = ?2",
                params![id, user_id],
            )
            .map(|n| n > 0)
            .map_err(db_err)
        })
        .await
    }

    /// The user of an unexpired API token; records when it was used.
    pub async fn token_user(&self, secret: &str) -> Result<Option<User>> {
        if !secret.starts_with(TOKEN_PREFIX) {
            return Ok(None);
        }
        let hash = digest(secret);
        self.with(move |c| {
            let now = now_ms();
            let found: Option<(i64, User)> = c
                .query_row(
                    "SELECT t.id, u.id, u.username, u.created_at FROM api_tokens t JOIN users u ON u.id = t.user_id
                     WHERE t.token_hash = ?1 AND (t.expires_at IS NULL OR t.expires_at > ?2)",
                    params![hash, now],
                    |r| {
                        Ok((
                            r.get(0)?,
                            User {
                                id: r.get(1)?,
                                username: r.get(2)?,
                                created_at: r.get(3)?,
                            },
                        ))
                    },
                )
                .optional()
                .map_err(db_err)?;
            if let Some((id, _)) = &found {
                c.execute(
                    "UPDATE api_tokens SET last_used_at = ?1 WHERE id = ?2",
                    params![now, id],
                )
                .map_err(db_err)?;
            }
            Ok(found.map(|(_, u)| u))
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_passwords() {
        assert!(check_username("ana").is_ok());
        assert!(check_username("ana.lopez@example.com").is_ok());
        assert!(check_username("").is_err());
        assert!(check_username("a b").is_err());
        assert!(check_username(&"x".repeat(65)).is_err());
        assert!(check_password("short").is_err());
        assert!(check_password("long enough").is_ok());
        assert!(check_password(&"x".repeat(2000)).is_err());
    }

    #[test]
    fn secrets_are_random_and_hashed() {
        let a = random_secret().unwrap();
        let b = random_secret().unwrap();
        assert_ne!(a, b);
        assert_eq!(a.len(), 43);
        assert_eq!(digest(&a).len(), 32);
        assert_ne!(digest(&a), digest(&b));
        let h = hash_password("correct horse").unwrap();
        assert!(h.starts_with("$argon2id$"));
        assert!(verify_password("correct horse", &h));
        assert!(!verify_password("wrong horse", &h));
    }
}

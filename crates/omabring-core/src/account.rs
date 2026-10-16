use rusqlite::{params, OptionalExtension};

use crate::crypto::{hash_password, random_token, sha256_hex, verify_password};
use crate::error::{Error, Result};
use crate::model::Session;
use crate::{now_rfc3339, parse_time, plus_seconds, App};

const SESSION_SECONDS: i64 = 60 * 60 * 24 * 30;

impl App {
    pub fn register(&self, email: &str, password: &str) -> Result<Session> {
        let email = normalize_email(email)?;
        validate_password(password)?;
        let hash = hash_password(password)?;
        let created = now_rfc3339();
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO users (email, password_hash, created_at) VALUES (?1, ?2, ?3)",
                params![email, hash, created],
            )
            .map_err(map_email_constraint)?;
            let user_id = conn.last_insert_rowid();
            issue_session(conn, user_id, &email)
        })
    }

    pub fn login(&self, email: &str, password: &str) -> Result<Session> {
        let email = normalize_email(email)?;
        self.with_conn(|conn| {
            let row = conn
                .query_row(
                    "SELECT id, password_hash FROM users WHERE email = ?1",
                    params![email],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()?;
            let Some((user_id, hash)) = row else {
                return Err(Error::BadCredentials);
            };
            if !verify_password(password, &hash) {
                return Err(Error::BadCredentials);
            }
            issue_session(conn, user_id, &email)
        })
    }

    pub fn resume(&self, token: &str) -> Result<Session> {
        let token_hash = sha256_hex(token);
        self.with_conn(|conn| {
            let row = conn
                .query_row(
                    "SELECT s.user_id, s.expires_at, u.email FROM sessions s JOIN users u ON u.id = s.user_id WHERE s.token_hash = ?1",
                    params![token_hash],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?)),
                )
                .optional()?;
            let Some((user_id, expires_at, email)) = row else {
                return Err(Error::Unauthorized);
            };
            if parse_time(&expires_at)? < chrono::Utc::now() {
                conn.execute("DELETE FROM sessions WHERE token_hash = ?1", params![token_hash])?;
                return Err(Error::Unauthorized);
            }
            Ok(Session {
                token: token.to_string(),
                user_id,
                email,
                expires_at,
            })
        })
    }

    pub fn logout(&self, token: &str) -> Result<()> {
        let token_hash = sha256_hex(token);
        self.with_conn(|conn| {
            conn.execute(
                "DELETE FROM sessions WHERE token_hash = ?1",
                params![token_hash],
            )?;
            Ok(())
        })
    }

    pub fn email_for(&self, user_id: i64) -> Result<String> {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT email FROM users WHERE id = ?1",
                params![user_id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or(Error::Unauthorized)
        })
    }
}

fn issue_session(conn: &rusqlite::Connection, user_id: i64, email: &str) -> Result<Session> {
    let token = random_token(32);
    let token_hash = sha256_hex(&token);
    let expires_at = plus_seconds(SESSION_SECONDS);
    conn.execute(
        "INSERT INTO sessions (token_hash, user_id, expires_at) VALUES (?1, ?2, ?3)",
        params![token_hash, user_id, expires_at],
    )?;
    Ok(Session {
        token,
        user_id,
        email: email.to_string(),
        expires_at,
    })
}

pub(crate) fn normalize_email(email: &str) -> Result<String> {
    let email = email.trim().to_lowercase();
    let Some((local, domain)) = email.split_once('@') else {
        return Err(Error::InvalidEmail);
    };
    if local.is_empty() || !domain.contains('.') || email.contains(char::is_whitespace) {
        return Err(Error::InvalidEmail);
    }
    Ok(email)
}

fn validate_password(password: &str) -> Result<()> {
    if password.chars().count() < 8 {
        Err(Error::WeakPassword)
    } else {
        Ok(())
    }
}

fn map_email_constraint(err: rusqlite::Error) -> Error {
    if let rusqlite::Error::SqliteFailure(code, _) = &err {
        if code.code == rusqlite::ErrorCode::ConstraintViolation {
            return Error::EmailTaken;
        }
    }
    Error::Db(err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::App;

    fn app() -> (tempfile::TempDir, App) {
        let dir = tempfile::tempdir().unwrap();
        let app = App::open(dir.path()).unwrap();
        (dir, app)
    }

    #[test]
    fn register_login_and_reject_bad_password() {
        let (_dir, app) = app();
        let session = app.register("Ada@Example.com", "password1").unwrap();
        assert_eq!(session.email, "ada@example.com");
        assert!(app.resume(&session.token).is_ok());
        assert!(matches!(
            app.login("ada@example.com", "nope-nope").unwrap_err(),
            Error::BadCredentials
        ));
        let again = app.login("ada@example.com", "password1").unwrap();
        assert_eq!(again.user_id, session.user_id);
        app.logout(&again.token).unwrap();
        assert!(app.resume(&again.token).is_err());
    }

    #[test]
    fn duplicate_email_and_weak_password() {
        let (_dir, app) = app();
        app.register("a@b.co", "password1").unwrap();
        assert!(matches!(
            app.register("a@b.co", "password1").unwrap_err(),
            Error::EmailTaken
        ));
        assert!(matches!(
            app.register("c@d.co", "short").unwrap_err(),
            Error::WeakPassword
        ));
        assert!(app.register("not-an-email", "password1").is_err());
    }
}

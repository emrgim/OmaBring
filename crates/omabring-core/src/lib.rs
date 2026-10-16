mod account;
mod crypto;
mod error;
mod model;
mod oauth;
mod shopping;
mod theme;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::Connection;

pub use crypto::pkce_s256;
pub use error::{Error, Result};
pub use model::*;
pub use theme::{
    load_active_palette, load_palette_path, load_palette_str, resolve_theme_dir, Palette,
};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const DEFAULT_BIND: &str = "127.0.0.1:8787";

pub struct App {
    conn: Mutex<Connection>,
    machine_key: [u8; 32],
    data_dir: PathBuf,
}

impl App {
    pub fn open(data_dir: impl AsRef<Path>) -> Result<Self> {
        let data_dir = data_dir.as_ref().to_path_buf();
        fs::create_dir_all(&data_dir)?;
        set_mode(&data_dir, 0o700);
        let key_path = data_dir.join("machine.key");
        let machine_key = load_or_create_key(&key_path)?;
        let db_path = data_dir.join("omabring.db");
        let conn = Connection::open(&db_path)?;
        conn.execute_batch(
            r#"
            PRAGMA foreign_keys = ON;
            PRAGMA journal_mode = WAL;
            PRAGMA busy_timeout = 5000;

            CREATE TABLE IF NOT EXISTS users (
                id INTEGER PRIMARY KEY,
                email TEXT NOT NULL UNIQUE COLLATE NOCASE,
                password_hash TEXT NOT NULL,
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS sessions (
                token_hash TEXT PRIMARY KEY,
                user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                expires_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS store_accounts (
                user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                store TEXT NOT NULL,
                ciphertext BLOB NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (user_id, store)
            );

            CREATE TABLE IF NOT EXISTS invoices (
                id INTEGER PRIMARY KEY,
                user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                store TEXT NOT NULL,
                invoice_no TEXT NOT NULL,
                purchased_on TEXT NOT NULL,
                currency TEXT NOT NULL,
                total REAL NOT NULL,
                UNIQUE (user_id, store, invoice_no)
            );

            CREATE TABLE IF NOT EXISTS invoice_lines (
                id INTEGER PRIMARY KEY,
                invoice_id INTEGER NOT NULL REFERENCES invoices(id) ON DELETE CASCADE,
                name TEXT NOT NULL,
                name_key TEXT NOT NULL,
                qty REAL NOT NULL,
                unit TEXT NOT NULL,
                unit_price REAL NOT NULL,
                line_total REAL NOT NULL
            );

            CREATE TABLE IF NOT EXISTS cart_lines (
                user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                store TEXT NOT NULL,
                line_id TEXT NOT NULL,
                product_id TEXT NOT NULL,
                name TEXT NOT NULL,
                qty REAL NOT NULL,
                unit TEXT NOT NULL,
                unit_price REAL,
                currency TEXT NOT NULL,
                PRIMARY KEY (user_id, store, line_id)
            );

            CREATE TABLE IF NOT EXISTS checkouts (
                id TEXT PRIMARY KEY,
                user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                store TEXT NOT NULL,
                status TEXT NOT NULL,
                url TEXT,
                note TEXT NOT NULL,
                detail TEXT NOT NULL,
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS oauth_clients (
                client_id TEXT PRIMARY KEY,
                client_name TEXT NOT NULL,
                secret_hash TEXT,
                redirect_uris TEXT NOT NULL,
                token_endpoint_auth_method TEXT NOT NULL,
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS oauth_pending (
                id TEXT PRIMARY KEY,
                client_id TEXT NOT NULL,
                redirect_uri TEXT NOT NULL,
                code_challenge TEXT NOT NULL,
                state TEXT,
                scope TEXT NOT NULL,
                status TEXT NOT NULL,
                auth_code TEXT,
                created_at TEXT NOT NULL,
                expires_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS oauth_codes (
                code_hash TEXT PRIMARY KEY,
                client_id TEXT NOT NULL,
                user_id INTEGER NOT NULL,
                redirect_uri TEXT NOT NULL,
                code_challenge TEXT NOT NULL,
                scope TEXT NOT NULL,
                expires_at TEXT NOT NULL,
                used INTEGER NOT NULL DEFAULT 0
            );

            CREATE TABLE IF NOT EXISTS oauth_tokens (
                id INTEGER PRIMARY KEY,
                user_id INTEGER NOT NULL,
                client_id TEXT NOT NULL,
                access_hash TEXT NOT NULL UNIQUE,
                refresh_hash TEXT NOT NULL UNIQUE,
                scope TEXT NOT NULL,
                access_expires TEXT NOT NULL,
                refresh_expires TEXT NOT NULL,
                revoked INTEGER NOT NULL DEFAULT 0
            );
            "#,
        )?;
        set_mode(&db_path, 0o600);
        Ok(Self {
            conn: Mutex::new(conn),
            machine_key,
            data_dir,
        })
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub(crate) fn with_conn<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let conn = self.conn.lock().unwrap_or_else(|err| err.into_inner());
        f(&conn)
    }
}

pub fn default_data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("OMABRING_DATA_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    home_dir().join(".local/share/omabring")
}

pub fn default_bind_addr() -> String {
    std::env::var("OMABRING_BIND").unwrap_or_else(|_| DEFAULT_BIND.to_string())
}

pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

pub(crate) fn now_rfc3339() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

pub(crate) fn parse_time(value: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|stamp| stamp.with_timezone(&Utc))
        .map_err(|_| Error::Message("bad timestamp".into()))
}

pub(crate) fn plus_seconds(seconds: i64) -> String {
    (Utc::now() + chrono::Duration::seconds(seconds)).to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn load_or_create_key(path: &Path) -> Result<[u8; 32]> {
    if path.is_file() {
        let bytes = fs::read(path)?;
        if bytes.len() != 32 {
            return Err(Error::Crypto);
        }
        let mut key = [0u8; 32];
        key.copy_from_slice(&bytes);
        set_mode(path, 0o600);
        return Ok(key);
    }
    let bytes = crypto::random_bytes(32);
    let mut key = [0u8; 32];
    key.copy_from_slice(&bytes);
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(&key)?;
    }
    Ok(key)
}

fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_directory_is_private() {
        let dir = tempfile::tempdir().unwrap();
        let app = App::open(dir.path()).unwrap();
        assert!(app.data_dir().join("machine.key").is_file());
        assert!(app.data_dir().join("omabring.db").is_file());
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(app.data_dir().join("machine.key"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }
}

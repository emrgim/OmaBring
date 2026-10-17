mod bringfast;
mod crypto;
mod error;
mod household;
mod model;
mod oauth;
mod receipts;
mod shopping;
mod theme;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{params, Connection};

pub use bringfast::{bringfast_image_index, BringfastImport, ImportReport};
pub use crypto::pkce_s256;
pub use error::{Error, Result};
pub use household::{
    bringfast_name_key, default_backup_dir, product_image_dirs, MissingImage, ProductHistory,
    PurchasePoint, StorePrice,
};
pub use model::*;
pub use receipts::receipt_page_images;
pub use theme::{
    active_stylesheet, load_active_palette, load_palette_path, load_palette_str, load_window_theme,
    resolve_theme_dir, theme_watch_paths, Palette,
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
        conn.execute_batch(SCHEMA)?;
        migrate_single_owner(&conn)?;
        household::ensure_household(&conn)?;
        let _ = fs::remove_file(data_dir.join("session.json"));
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

const SCHEMA: &str = r#"
PRAGMA foreign_keys = ON;
PRAGMA journal_mode = WAL;
PRAGMA busy_timeout = 5000;

CREATE TABLE IF NOT EXISTS store_accounts (
    store TEXT PRIMARY KEY,
    ciphertext BLOB NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS invoices (
    id INTEGER PRIMARY KEY,
    store TEXT NOT NULL,
    invoice_no TEXT NOT NULL,
    purchased_on TEXT NOT NULL,
    currency TEXT NOT NULL,
    total REAL NOT NULL,
    UNIQUE (store, invoice_no)
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
    store TEXT NOT NULL,
    line_id TEXT NOT NULL,
    product_id TEXT NOT NULL,
    name TEXT NOT NULL,
    qty REAL NOT NULL,
    unit TEXT NOT NULL,
    unit_price REAL,
    currency TEXT NOT NULL,
    PRIMARY KEY (store, line_id)
);

CREATE TABLE IF NOT EXISTS checkouts (
    id TEXT PRIMARY KEY,
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
    redirect_uri TEXT NOT NULL,
    code_challenge TEXT NOT NULL,
    scope TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    used INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS oauth_tokens (
    id INTEGER PRIMARY KEY,
    client_id TEXT NOT NULL,
    access_hash TEXT NOT NULL UNIQUE,
    refresh_hash TEXT NOT NULL UNIQUE,
    scope TEXT NOT NULL,
    access_expires TEXT NOT NULL,
    refresh_expires TEXT NOT NULL,
    revoked INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS products (
    product_key TEXT PRIMARY KEY,
    sku TEXT,
    name TEXT NOT NULL,
    category TEXT,
    image_url TEXT,
    source TEXT
);

CREATE TABLE IF NOT EXISTS shopping_list (
    product_key TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    vote TEXT NOT NULL DEFAULT '',
    excluded INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS product_images (
    name_key TEXT PRIMARY KEY,
    path TEXT NOT NULL,
    source_url TEXT NOT NULL DEFAULT ''
);

CREATE TABLE IF NOT EXISTS image_searches (
    name_key TEXT PRIMARY KEY,
    searched_at TEXT NOT NULL,
    found INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS product_aliases (
    alias_key TEXT PRIMARY KEY,
    canonical_key TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS app_settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
"#;

/// Older databases keyed every row by a local account and stored a password
/// hash. This machine has one owner. Rows move onto that owner. A store keeps
/// the latest saved sign-in. Receipts, cart lines, and checkouts are all kept;
/// a repeated receipt number or cart line id gets a stable suffix.
fn migrate_single_owner(conn: &Connection) -> Result<()> {
    if !legacy_account_schema(conn)? {
        return Ok(());
    }
    conn.pragma_update(None, "foreign_keys", false)?;
    conn.pragma_update(None, "secure_delete", true)?;
    let tx = conn.unchecked_transaction()?;
    disambiguate_invoice_numbers(&tx)?;
    disambiguate_cart_line_ids(&tx)?;
    tx.execute_batch(
        r#"
        DROP TABLE IF EXISTS store_accounts_v2;
        DROP TABLE IF EXISTS invoices_v2;
        DROP TABLE IF EXISTS cart_lines_v2;
        DROP TABLE IF EXISTS checkouts_v2;
        DROP TABLE IF EXISTS oauth_codes_v2;
        DROP TABLE IF EXISTS oauth_tokens_v2;

        CREATE TABLE store_accounts_v2 (
            store TEXT PRIMARY KEY,
            ciphertext BLOB NOT NULL,
            updated_at TEXT NOT NULL
        );
        INSERT INTO store_accounts_v2 (store, ciphertext, updated_at)
        SELECT store, ciphertext, updated_at FROM store_accounts AS chosen
        WHERE chosen.rowid = (
            SELECT rowid FROM store_accounts AS candidate
            WHERE candidate.store = chosen.store
            ORDER BY candidate.updated_at DESC, candidate.user_id DESC
            LIMIT 1
        );

        CREATE TABLE invoices_v2 (
            id INTEGER PRIMARY KEY,
            store TEXT NOT NULL,
            invoice_no TEXT NOT NULL,
            purchased_on TEXT NOT NULL,
            currency TEXT NOT NULL,
            total REAL NOT NULL,
            UNIQUE (store, invoice_no)
        );
        INSERT INTO invoices_v2 (id, store, invoice_no, purchased_on, currency, total)
        SELECT id, store, invoice_no, purchased_on, currency, total FROM invoices;

        CREATE TABLE cart_lines_v2 (
            store TEXT NOT NULL,
            line_id TEXT NOT NULL,
            product_id TEXT NOT NULL,
            name TEXT NOT NULL,
            qty REAL NOT NULL,
            unit TEXT NOT NULL,
            unit_price REAL,
            currency TEXT NOT NULL,
            PRIMARY KEY (store, line_id)
        );
        INSERT INTO cart_lines_v2 (store, line_id, product_id, name, qty, unit, unit_price, currency)
        SELECT store, line_id, product_id, name, qty, unit, unit_price, currency FROM cart_lines;

        CREATE TABLE checkouts_v2 (
            id TEXT PRIMARY KEY,
            store TEXT NOT NULL,
            status TEXT NOT NULL,
            url TEXT,
            note TEXT NOT NULL,
            detail TEXT NOT NULL,
            created_at TEXT NOT NULL
        );
        INSERT INTO checkouts_v2 (id, store, status, url, note, detail, created_at)
        SELECT id, store, status, url, note, detail, created_at FROM checkouts;

        CREATE TABLE oauth_codes_v2 (
            code_hash TEXT PRIMARY KEY,
            client_id TEXT NOT NULL,
            redirect_uri TEXT NOT NULL,
            code_challenge TEXT NOT NULL,
            scope TEXT NOT NULL,
            expires_at TEXT NOT NULL,
            used INTEGER NOT NULL DEFAULT 0
        );
        INSERT INTO oauth_codes_v2 (code_hash, client_id, redirect_uri, code_challenge, scope, expires_at, used)
        SELECT code_hash, client_id, redirect_uri, code_challenge, scope, expires_at, used FROM oauth_codes;

        CREATE TABLE oauth_tokens_v2 (
            id INTEGER PRIMARY KEY,
            client_id TEXT NOT NULL,
            access_hash TEXT NOT NULL UNIQUE,
            refresh_hash TEXT NOT NULL UNIQUE,
            scope TEXT NOT NULL,
            access_expires TEXT NOT NULL,
            refresh_expires TEXT NOT NULL,
            revoked INTEGER NOT NULL DEFAULT 0
        );
        INSERT INTO oauth_tokens_v2 (id, client_id, access_hash, refresh_hash, scope, access_expires, refresh_expires, revoked)
        SELECT id, client_id, access_hash, refresh_hash, scope, access_expires, refresh_expires, revoked FROM oauth_tokens;

        DROP TABLE store_accounts;
        DROP TABLE invoices;
        DROP TABLE cart_lines;
        DROP TABLE checkouts;
        DROP TABLE oauth_codes;
        DROP TABLE oauth_tokens;
        DROP TABLE IF EXISTS sessions;
        DROP TABLE IF EXISTS users;

        ALTER TABLE store_accounts_v2 RENAME TO store_accounts;
        ALTER TABLE invoices_v2 RENAME TO invoices;
        ALTER TABLE cart_lines_v2 RENAME TO cart_lines;
        ALTER TABLE checkouts_v2 RENAME TO checkouts;
        ALTER TABLE oauth_codes_v2 RENAME TO oauth_codes;
        ALTER TABLE oauth_tokens_v2 RENAME TO oauth_tokens;
        "#,
    )?;
    tx.commit()?;
    conn.pragma_update(None, "foreign_keys", true)?;
    conn.execute_batch(
        "PRAGMA wal_checkpoint(TRUNCATE); VACUUM; PRAGMA wal_checkpoint(TRUNCATE);",
    )?;
    conn.pragma_update(None, "secure_delete", false)?;
    Ok(())
}

fn legacy_account_schema(conn: &Connection) -> Result<bool> {
    Ok(table_exists(conn, "users")?
        || column_exists(conn, "store_accounts", "user_id")?
        || column_exists(conn, "oauth_tokens", "user_id")?)
}

fn table_exists(conn: &Connection, table: &str) -> Result<bool> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
        params![table],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

pub(crate) fn column_exists(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    if !table_exists(conn, table)? {
        return Ok(false);
    }
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let names = stmt.query_map([], |row| row.get::<_, String>(1))?;
    for name in names {
        if name? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn disambiguate_invoice_numbers(conn: &Connection) -> Result<()> {
    if !table_exists(conn, "invoices")? {
        return Ok(());
    }
    let mut stmt = conn.prepare("SELECT id, store, invoice_no FROM invoices ORDER BY id")?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    let rows = rows.collect::<std::result::Result<Vec<_>, _>>()?;
    drop(stmt);
    let mut seen = std::collections::HashSet::new();
    for (id, store, invoice_no) in rows {
        let mut candidate = invoice_no.clone();
        if seen.insert((store.clone(), candidate.clone())) {
            continue;
        }
        let mut n = 2u32;
        candidate = format!("{invoice_no} #{id}");
        while !seen.insert((store.clone(), candidate.clone())) {
            candidate = format!("{invoice_no} #{id}-{n}");
            n += 1;
        }
        conn.execute(
            "UPDATE invoices SET invoice_no = ?1 WHERE id = ?2",
            params![candidate, id],
        )?;
    }
    Ok(())
}

fn disambiguate_cart_line_ids(conn: &Connection) -> Result<()> {
    if !table_exists(conn, "cart_lines")? {
        return Ok(());
    }
    let mut stmt =
        conn.prepare("SELECT rowid, user_id, store, line_id FROM cart_lines ORDER BY rowid")?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    let rows = rows.collect::<std::result::Result<Vec<_>, _>>()?;
    drop(stmt);
    let mut seen = std::collections::HashSet::new();
    for (rowid, user_id, store, line_id) in rows {
        let mut candidate = line_id.clone();
        if seen.insert((store.clone(), candidate.clone())) {
            continue;
        }
        let mut n = 2u32;
        candidate = format!("{line_id}#{user_id}");
        while !seen.insert((store.clone(), candidate.clone())) {
            candidate = format!("{line_id}#{user_id}-{n}");
            n += 1;
        }
        conn.execute(
            "UPDATE cart_lines SET line_id = ?1 WHERE rowid = ?2",
            params![candidate, rowid],
        )?;
    }
    Ok(())
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

    #[test]
    fn legacy_accounts_collapse_onto_one_owner() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("omabring.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE users (
                id INTEGER PRIMARY KEY,
                email TEXT NOT NULL UNIQUE,
                password_hash TEXT NOT NULL,
                created_at TEXT NOT NULL
            );
            CREATE TABLE sessions (
                token_hash TEXT PRIMARY KEY,
                user_id INTEGER NOT NULL,
                expires_at TEXT NOT NULL
            );
            CREATE TABLE store_accounts (
                user_id INTEGER NOT NULL,
                store TEXT NOT NULL,
                ciphertext BLOB NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (user_id, store)
            );
            CREATE TABLE invoices (
                id INTEGER PRIMARY KEY,
                user_id INTEGER NOT NULL,
                store TEXT NOT NULL,
                invoice_no TEXT NOT NULL,
                purchased_on TEXT NOT NULL,
                currency TEXT NOT NULL,
                total REAL NOT NULL,
                UNIQUE (user_id, store, invoice_no)
            );
            CREATE TABLE invoice_lines (
                id INTEGER PRIMARY KEY,
                invoice_id INTEGER NOT NULL,
                name TEXT NOT NULL,
                name_key TEXT NOT NULL,
                qty REAL NOT NULL,
                unit TEXT NOT NULL,
                unit_price REAL NOT NULL,
                line_total REAL NOT NULL
            );
            CREATE TABLE cart_lines (
                user_id INTEGER NOT NULL,
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
            CREATE TABLE checkouts (
                id TEXT PRIMARY KEY,
                user_id INTEGER NOT NULL,
                store TEXT NOT NULL,
                status TEXT NOT NULL,
                url TEXT,
                note TEXT NOT NULL,
                detail TEXT NOT NULL,
                created_at TEXT NOT NULL
            );
            CREATE TABLE oauth_clients (
                client_id TEXT PRIMARY KEY,
                client_name TEXT NOT NULL,
                secret_hash TEXT,
                redirect_uris TEXT NOT NULL,
                token_endpoint_auth_method TEXT NOT NULL,
                created_at TEXT NOT NULL
            );
            CREATE TABLE oauth_pending (
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
            CREATE TABLE oauth_codes (
                code_hash TEXT PRIMARY KEY,
                client_id TEXT NOT NULL,
                user_id INTEGER NOT NULL,
                redirect_uri TEXT NOT NULL,
                code_challenge TEXT NOT NULL,
                scope TEXT NOT NULL,
                expires_at TEXT NOT NULL,
                used INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE oauth_tokens (
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
        )
        .unwrap();
        conn.execute(
            "INSERT INTO users (id, email, password_hash, created_at) VALUES (1, 'a@b.co', 'legacy-password-hash-9f3a', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO users (id, email, password_hash, created_at) VALUES (2, 'c@d.co', 'legacy-password-hash-9f3a', '2026-02-01T00:00:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions (token_hash, user_id, expires_at) VALUES ('sess', 1, '2026-12-01T00:00:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO store_accounts (user_id, store, ciphertext, updated_at) VALUES (1, 'carrefour', X'01', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO store_accounts (user_id, store, ciphertext, updated_at) VALUES (2, 'carrefour', X'02', '2026-03-01T00:00:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO store_accounts (user_id, store, ciphertext, updated_at) VALUES (1, 'spinneys', X'03', '2026-01-02T00:00:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO invoices (id, user_id, store, invoice_no, purchased_on, currency, total) VALUES (1, 1, 'carrefour', '1001', '2026-01-01', 'AED', 5)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO invoices (id, user_id, store, invoice_no, purchased_on, currency, total) VALUES (2, 2, 'carrefour', '1001', '2026-02-01', 'AED', 7)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO invoice_lines (invoice_id, name, name_key, qty, unit, unit_price, line_total) VALUES (1, 'Kept Milk', 'kept milk', 1, 'piece', 5, 5)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO invoice_lines (invoice_id, name, name_key, qty, unit, unit_price, line_total) VALUES (2, 'Kept Rice', 'kept rice', 1, 'piece', 7, 7)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO cart_lines (user_id, store, line_id, product_id, name, qty, unit, unit_price, currency) VALUES (1, 'carrefour', 'L1', 'y', 'Kept Yogurt', 1, 'piece', 4, 'AED')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO cart_lines (user_id, store, line_id, product_id, name, qty, unit, unit_price, currency) VALUES (2, 'carrefour', 'L1', 'b', 'Kept Bread', 2, 'piece', 3, 'AED')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO checkouts (id, user_id, store, status, url, note, detail, created_at) VALUES ('chk', 1, 'carrefour', 'prepared', NULL, 'kept-checkout', '[]', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO oauth_codes (code_hash, client_id, user_id, redirect_uri, code_challenge, scope, expires_at, used) VALUES ('code', 'client', 1, 'http://127.0.0.1/cb', 'challenge', 'omabring', '2026-12-01T00:00:00Z', 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO oauth_tokens (user_id, client_id, access_hash, refresh_hash, scope, access_expires, refresh_expires, revoked) VALUES (1, 'client', 'kept-access-hash', 'kept-refresh-hash', 'omabring', '2026-12-01T00:00:00Z', '2026-12-01T00:00:00Z', 0)",
            [],
        )
        .unwrap();
        drop(conn);
        fs::write(dir.path().join("session.json"), "{\"token\":\"abc\"}").unwrap();

        let app = App::open(dir.path()).unwrap();
        assert!(!dir.path().join("session.json").exists());
        app.with_conn(|conn| {
            assert!(!table_exists(conn, "users")?);
            assert!(!table_exists(conn, "sessions")?);
            assert!(!column_exists(conn, "store_accounts", "user_id")?);
            assert!(!column_exists(conn, "oauth_tokens", "user_id")?);
            let schema: String = conn
                .query_row(
                    "SELECT group_concat(sql, ' ') FROM sqlite_master",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(!schema.contains("password_hash"));
            let stores: i64 = conn
                .query_row("SELECT COUNT(*) FROM store_accounts", [], |row| row.get(0))
                .unwrap();
            assert_eq!(stores, 2);
            let carrefour: Vec<u8> = conn
                .query_row(
                    "SELECT ciphertext FROM store_accounts WHERE store = 'carrefour'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(carrefour, vec![2]);
            let invoices: Vec<(String, String)> = {
                let mut stmt = conn
                    .prepare(
                        "SELECT invoices.invoice_no, invoice_lines.name FROM invoices JOIN invoice_lines ON invoice_lines.invoice_id = invoices.id ORDER BY invoice_lines.name",
                    )
                    .unwrap();
                stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                    .unwrap()
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .unwrap()
            };
            assert_eq!(invoices.len(), 2);
            assert!(invoices.iter().any(|(_, name)| name == "Kept Milk"));
            assert!(invoices.iter().any(|(_, name)| name == "Kept Rice"));
            assert!(invoices.iter().any(|(number, _)| number == "1001"));
            assert!(invoices.iter().any(|(number, _)| number != "1001"));
            let carts: Vec<String> = {
                let mut stmt = conn
                    .prepare("SELECT name FROM cart_lines ORDER BY name")
                    .unwrap();
                stmt.query_map([], |row| row.get(0))
                    .unwrap()
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .unwrap()
            };
            assert_eq!(carts, vec!["Kept Bread".to_string(), "Kept Yogurt".into()]);
            let note: String = conn
                .query_row("SELECT note FROM checkouts WHERE id = 'chk'", [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(note, "kept-checkout");
            let access: String = conn
                .query_row(
                    "SELECT access_hash FROM oauth_tokens",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(access, "kept-access-hash");
            Ok(())
        })
        .unwrap();
        drop(app);
        let marker = b"legacy-password-hash-9f3a";
        for path in [
            db_path,
            dir.path().join("omabring.db-wal"),
            dir.path().join("omabring.db-shm"),
        ] {
            if !path.is_file() {
                continue;
            }
            let raw = fs::read(&path).unwrap();
            assert!(
                !raw.windows(marker.len()).any(|window| window == marker),
                "{} still contains the old password hash",
                path.display()
            );
        }
    }
}

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::crypto::{self, hash_password, verify_password};
use crate::error::{Error, Result};
use crate::retailers::{self, Probe, Retailer, RETAILERS};

static READY: Mutex<Vec<String>> = Mutex::new(Vec::new());

#[derive(Clone, Debug)]
pub struct Db {
    dir: PathBuf,
    path: PathBuf,
}

#[derive(Clone, Debug, Serialize)]
pub struct User {
    pub id: i64,
    pub email: String,
    pub password_hash: String,
    pub mcp_token: String,
    pub created_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoreSecret {
    pub email: String,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub address: String,
    #[serde(default)]
    pub auth_token: String,
    #[serde(default)]
    pub store_user_id: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct StoreAccount {
    #[serde(flatten)]
    pub retailer: RetailerView,
    pub enabled: bool,
    pub linked: bool,
    pub login_email: Option<String>,
    pub has_password: bool,
    pub delivery_address: String,
    pub capabilities: Vec<crate::retailers::Capability>,
}

#[derive(Clone, Debug, Serialize)]
pub struct RetailerView {
    pub id: String,
    pub name: String,
    pub url: String,
    pub color: String,
    pub cart_url: String,
    pub checkout_url: String,
    pub domain: Option<String>,
    pub search: bool,
    pub receipts: bool,
    pub login: bool,
    pub shop: bool,
    pub checkout: bool,
}

fn view(r: &Retailer) -> RetailerView {
    RetailerView {
        id: r.id.into(),
        name: r.name.into(),
        url: r.url.into(),
        color: r.color.into(),
        cart_url: r.cart_url.into(),
        checkout_url: r.checkout_url.into(),
        domain: r.domain.map(|d| d.to_string()),
        search: r.search,
        receipts: r.receipts,
        login: r.login,
        shop: r.shop,
        checkout: r.checkout,
    }
}

pub fn now_secs() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

pub fn default_data_dir() -> PathBuf {
    if let Ok(p) = std::env::var("OMABRING_DATA") {
        if !p.trim().is_empty() {
            return PathBuf::from(p);
        }
    }
    std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join(".omabring"))
        .unwrap_or_else(|| PathBuf::from(".omabring"))
}

impl Db {
    pub fn open(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&dir)?;
        let db = Self { path: dir.join("omabring.db"), dir };
        let _ = db.connect()?;
        Ok(db)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn connect(&self) -> Result<Connection> {
        let conn = Connection::open(&self.path)?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let key = self.path.to_string_lossy().to_string();
        let mut guard = READY.lock().unwrap_or_else(|e| e.into_inner());
        if !guard.iter().any(|k| k == &key) {
            init_schema(&conn)?;
            guard.push(key);
        }
        Ok(conn)
    }

    fn key(&self) -> Result<[u8; 32]> {
        crypto::load_master_key(&self.dir)
    }

    pub fn create_user(&self, email: &str, password: &str) -> Result<User> {
        let email = email.trim().to_ascii_lowercase();
        if email.is_empty() || !email.contains('@') || password.len() < 6 {
            return Err(Error::Invalid("email and password (min 6) required".into()));
        }
        let token = crypto::random_token(32);
        let conn = self.connect()?;
        conn.execute(
            "INSERT INTO users(email, password_hash, mcp_token, created_at) VALUES (?1,?2,?3,?4)",
            rusqlite::params![email, hash_password(password), token, now_secs()],
        )
        .map_err(|e| match e {
            rusqlite::Error::SqliteFailure(code, _)
                if code.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                Error::Conflict("email already registered".into())
            }
            other => Error::Sql(other),
        })?;
        self.user_by_email(&email)?.ok_or_else(|| Error::Invalid("user missing after insert".into()))
    }

    pub fn user_by_email(&self, email: &str) -> Result<Option<User>> {
        let conn = self.connect()?;
        Ok(conn
            .query_row(
                "SELECT id, email, password_hash, mcp_token, created_at FROM users WHERE email=?1",
                [email.trim().to_ascii_lowercase()],
                row_user,
            )
            .optional()?)
    }

    pub fn user_by_id(&self, id: i64) -> Result<Option<User>> {
        let conn = self.connect()?;
        Ok(conn
            .query_row(
                "SELECT id, email, password_hash, mcp_token, created_at FROM users WHERE id=?1",
                [id],
                row_user,
            )
            .optional()?)
    }

    pub fn user_by_token(&self, token: &str) -> Result<Option<User>> {
        let token = token.trim();
        if token.is_empty() {
            return Ok(None);
        }
        let conn = self.connect()?;
        Ok(conn
            .query_row(
                "SELECT id, email, password_hash, mcp_token, created_at FROM users WHERE mcp_token=?1",
                [token],
                row_user,
            )
            .optional()?)
    }

    pub fn verify_user(&self, user: &User, password: &str) -> bool {
        verify_password(&user.password_hash, password)
    }

    pub fn set_password(&self, user_id: i64, password: &str) -> Result<()> {
        if password.len() < 6 {
            return Err(Error::Invalid("password must be at least 6 characters".into()));
        }
        let conn = self.connect()?;
        conn.execute(
            "UPDATE users SET password_hash=?1 WHERE id=?2",
            rusqlite::params![hash_password(password), user_id],
        )?;
        Ok(())
    }

    pub fn create_session(&self, user_id: i64, days: i64) -> Result<(String, String)> {
        let token = crypto::random_token(32);
        let csrf = crypto::random_token(18);
        let now = now_secs();
        let conn = self.connect()?;
        conn.execute(
            "INSERT INTO sessions(token, user_id, csrf, created_at, expires_at) VALUES (?1,?2,?3,?4,?5)",
            rusqlite::params![token, user_id, csrf, now, now + days * 86400],
        )?;
        Ok((token, csrf))
    }

    pub fn session_user(&self, token: &str) -> Result<Option<(User, String)>> {
        if token.is_empty() {
            return Ok(None);
        }
        let conn = self.connect()?;
        let row = conn
            .query_row(
                "SELECT user_id, csrf, expires_at FROM sessions WHERE token=?1",
                [token],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?)),
            )
            .optional()?;
        let Some((uid, csrf, exp)) = row else { return Ok(None) };
        if exp < now_secs() {
            conn.execute("DELETE FROM sessions WHERE token=?1", [token])?;
            return Ok(None);
        }
        Ok(self.user_by_id(uid)?.map(|u| (u, csrf)))
    }

    pub fn delete_session(&self, token: &str) -> Result<()> {
        let conn = self.connect()?;
        conn.execute("DELETE FROM sessions WHERE token=?1", [token])?;
        Ok(())
    }

    pub fn create_reset(&self, email: &str) -> Result<Option<String>> {
        let Some(user) = self.user_by_email(email)? else { return Ok(None) };
        let token = crypto::random_token(32);
        let conn = self.connect()?;
        conn.execute("DELETE FROM password_resets WHERE user_id=?1", [user.id])?;
        conn.execute(
            "INSERT INTO password_resets(token, user_id, created_at) VALUES (?1,?2,?3)",
            rusqlite::params![token, user.id, now_secs()],
        )?;
        Ok(Some(token))
    }

    pub fn consume_reset(&self, token: &str, max_age: i64) -> Result<Option<User>> {
        let token = token.trim();
        if token.is_empty() {
            return Ok(None);
        }
        let conn = self.connect()?;
        let row = conn
            .query_row(
                "SELECT user_id, created_at FROM password_resets WHERE token=?1",
                [token],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
            )
            .optional()?;
        let Some((uid, created)) = row else { return Ok(None) };
        conn.execute("DELETE FROM password_resets WHERE token=?1", [token])?;
        if now_secs() - created > max_age {
            return Ok(None);
        }
        self.user_by_id(uid)
    }

    pub fn set_retailer_account(&self, user_id: i64, retailer: &str, email: &str, password: &str, address: &str) -> Result<()> {
        if !retailers::known(retailer) {
            return Err(Error::Invalid(format!("unknown retailer {retailer}")));
        }
        let key = self.key()?;
        let conn = self.connect()?;
        let existing = conn
            .query_row(
                "SELECT secret, address FROM retailer_accounts WHERE user_id=?1 AND retailer=?2",
                rusqlite::params![user_id, retailer],
                |r| Ok((r.get::<_, Option<Vec<u8>>>(0)?, r.get::<_, Option<String>>(1)?)),
            )
            .optional()?;
        let mut prev = StoreSecret {
            email: email.into(),
            password: String::new(),
            address: String::new(),
            auth_token: String::new(),
            store_user_id: String::new(),
        };
        if let Some((Some(blob), _)) = &existing {
            if let Ok(plain) = crypto::open(&key, blob) {
                if let Ok(old) = serde_json::from_slice::<StoreSecret>(&plain) {
                    prev.password = old.password;
                    prev.auth_token = old.auth_token;
                    prev.store_user_id = old.store_user_id;
                    if email.is_empty() {
                        prev.email = old.email;
                    }
                }
            }
        }
        if !email.is_empty() {
            prev.email = email.into();
        }
        if !password.is_empty() {
            prev.password = password.into();
        }
        let blob = crypto::seal(&key, &serde_json::to_vec(&prev).unwrap_or_default())?;
        let addr = if address.trim().is_empty() {
            existing.and_then(|(_, a)| a).unwrap_or_default()
        } else {
            address.trim().to_string()
        };
        conn.execute(
            "INSERT INTO retailer_accounts(user_id, retailer, email, secret, address) VALUES (?1,?2,?3,?4,?5)
             ON CONFLICT(user_id, retailer) DO UPDATE SET email=excluded.email, secret=excluded.secret, address=excluded.address",
            rusqlite::params![user_id, retailer, prev.email, blob, addr],
        )?;
        Ok(())
    }

    pub fn retailer_secret(&self, user_id: i64, retailer: &str) -> Result<Option<StoreSecret>> {
        let conn = self.connect()?;
        let row = conn
            .query_row(
                "SELECT email, secret, address FROM retailer_accounts WHERE user_id=?1 AND retailer=?2",
                rusqlite::params![user_id, retailer],
                |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<Vec<u8>>>(1)?, r.get::<_, Option<String>>(2)?)),
            )
            .optional()?;
        let Some((email, secret, address)) = row else { return Ok(None) };
        let mut data = StoreSecret {
            email: email.unwrap_or_default(),
            password: String::new(),
            address: address.unwrap_or_default(),
            auth_token: String::new(),
            store_user_id: String::new(),
        };
        if let Some(blob) = secret {
            if let Ok(plain) = crypto::open(&self.key()?, &blob) {
                if let Ok(payload) = serde_json::from_slice::<StoreSecret>(&plain) {
                    if !payload.email.is_empty() {
                        data.email = payload.email;
                    }
                    data.password = payload.password;
                    data.auth_token = payload.auth_token;
                    data.store_user_id = payload.store_user_id;
                }
            }
        }
        Ok(Some(data))
    }

    pub fn save_store_session(&self, user_id: i64, retailer: &str, token: &str, store_user_id: &str) -> Result<()> {
        let current = self.retailer_secret(user_id, retailer)?.unwrap_or(StoreSecret {
            email: String::new(),
            password: String::new(),
            address: String::new(),
            auth_token: String::new(),
            store_user_id: String::new(),
        });
        let payload = StoreSecret {
            auth_token: token.into(),
            store_user_id: store_user_id.into(),
            ..current.clone()
        };
        let blob = crypto::seal(&self.key()?, &serde_json::to_vec(&payload).unwrap_or_default())?;
        let conn = self.connect()?;
        conn.execute(
            "INSERT INTO retailer_accounts(user_id, retailer, email, secret, address) VALUES (?1,?2,?3,?4,?5)
             ON CONFLICT(user_id, retailer) DO UPDATE SET secret=excluded.secret",
            rusqlite::params![user_id, retailer, payload.email, blob, payload.address],
        )?;
        Ok(())
    }

    pub fn clear_retailer(&self, user_id: i64, retailer: &str) -> Result<()> {
        let conn = self.connect()?;
        conn.execute(
            "DELETE FROM retailer_accounts WHERE user_id=?1 AND retailer=?2",
            rusqlite::params![user_id, retailer],
        )?;
        Ok(())
    }

    pub fn is_store_enabled(&self, retailer: &str) -> Result<bool> {
        let default = retailers::retailer(retailer).map(|r| r.enabled_default).unwrap_or(false);
        let conn = self.connect()?;
        let row = conn
            .query_row("SELECT enabled FROM store_flags WHERE retailer=?1", [retailer], |r| r.get::<_, i64>(0))
            .optional()?;
        Ok(row.map(|n| n != 0).unwrap_or(default))
    }

    pub fn set_store_enabled(&self, retailer: &str, enabled: bool) -> Result<()> {
        if !retailers::known(retailer) {
            return Err(Error::Invalid(format!("unknown retailer {retailer}")));
        }
        let conn = self.connect()?;
        conn.execute(
            "INSERT INTO store_flags(retailer, enabled) VALUES (?1,?2)
             ON CONFLICT(retailer) DO UPDATE SET enabled=excluded.enabled",
            rusqlite::params![retailer, if enabled { 1 } else { 0 }],
        )?;
        Ok(())
    }

    pub fn record_probe(&self, user_id: i64, retailer: &str, capability: &str, ok: bool, error_code: Option<&str>) -> Result<()> {
        let conn = self.connect()?;
        conn.execute(
            "INSERT INTO store_probes(user_id, retailer, capability, ok, at, error_code) VALUES (?1,?2,?3,?4,?5,?6)
             ON CONFLICT(user_id, retailer, capability) DO UPDATE SET ok=excluded.ok, at=excluded.at, error_code=excluded.error_code",
            rusqlite::params![user_id, retailer, capability, if ok { 1 } else { 0 }, now_secs(), error_code],
        )?;
        Ok(())
    }

    pub fn probes(&self, user_id: i64) -> Result<Vec<(String, String, Probe)>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            "SELECT retailer, capability, ok, at, error_code FROM store_probes WHERE user_id=?1",
        )?;
        let rows = stmt.query_map([user_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, Option<String>>(4)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (retailer, cap, ok, at, error_code) = row?;
            out.push((retailer, cap, Probe { ok: ok != 0, at, error_code }));
        }
        Ok(out)
    }

    pub fn list_accounts(&self, user_id: i64) -> Result<Vec<StoreAccount>> {
        let probes = self.probes(user_id)?;
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            "SELECT retailer, email, address, secret FROM retailer_accounts WHERE user_id=?1",
        )?;
        let rows = stmt.query_map([user_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<Vec<u8>>>(3)?,
            ))
        })?;
        let mut linked = Vec::new();
        for row in rows {
            linked.push(row?);
        }
        let key = self.key()?;
        let mut out = Vec::new();
        for r in RETAILERS {
            let row = linked.iter().find(|(id, _, _, _)| id == r.id);
            let mut password = String::new();
            if let Some((_, _, _, Some(blob))) = row {
                if let Ok(plain) = crypto::open(&key, blob) {
                    if let Ok(v) = serde_json::from_slice::<Value>(&plain) {
                        password = v.get("password").and_then(|p| p.as_str()).unwrap_or("").to_string();
                    }
                }
            }
            out.push(StoreAccount {
                capabilities: retailers::capabilities(r.id, &probes),
                enabled: self.is_store_enabled(r.id)?,
                linked: row.is_some(),
                login_email: row.and_then(|(_, e, _, _)| e.clone()),
                has_password: !password.is_empty(),
                delivery_address: row.and_then(|(_, _, a, _)| a.clone()).unwrap_or_default(),
                retailer: view(r),
            });
        }
        Ok(out)
    }

    pub fn save_oauth_code(&self, user_id: i64, redirect_uri: &str, client_id: &str, challenge: &str, method: &str) -> Result<String> {
        let code = crypto::random_token(24);
        let conn = self.connect()?;
        conn.execute(
            "INSERT INTO oauth_codes(code, user_id, redirect_uri, created_at, client_id, code_challenge, code_challenge_method)
             VALUES (?1,?2,?3,?4,?5,?6,?7)",
            rusqlite::params![code, user_id, redirect_uri, now_secs(), client_id, challenge, method],
        )?;
        Ok(code)
    }

    pub fn consume_oauth_code(&self, code: &str, redirect_uri: Option<&str>) -> Result<Option<(User, String, String, String)>> {
        let conn = self.connect()?;
        let row = conn
            .query_row(
                "SELECT user_id, redirect_uri, created_at, client_id, code_challenge, code_challenge_method FROM oauth_codes WHERE code=?1",
                [code],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, Option<String>>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, Option<String>>(3)?,
                        r.get::<_, Option<String>>(4)?,
                        r.get::<_, Option<String>>(5)?,
                    ))
                },
            )
            .optional()?;
        let Some((uid, stored_redir, created, client_id, challenge, method)) = row else {
            return Ok(None);
        };
        if now_secs() - created > 300 {
            conn.execute("DELETE FROM oauth_codes WHERE code=?1", [code])?;
            return Ok(None);
        }
        if let Some(want) = redirect_uri {
            if !want.is_empty() {
                if let Some(stored) = &stored_redir {
                    if !stored.is_empty() && stored != want {
                        return Ok(None);
                    }
                }
            }
        }
        conn.execute("DELETE FROM oauth_codes WHERE code=?1", [code])?;
        let user = self.user_by_id(uid)?;
        Ok(user.map(|u| {
            (
                u,
                client_id.unwrap_or_default(),
                challenge.unwrap_or_default(),
                method.unwrap_or_else(|| "S256".into()),
            )
        }))
    }

    pub fn register_oauth_client(&self, redirect_uris: &[String], auth_method: &str) -> Result<Value> {
        let client_id = format!("ob_{}", crypto::random_token(16));
        let conn = self.connect()?;
        let issued = now_secs();
        conn.execute(
            "INSERT INTO oauth_clients(client_id, client_secret, redirect_uris, token_endpoint_auth_method, created_at)
             VALUES (?1, NULL, ?2, ?3, ?4)",
            rusqlite::params![client_id, serde_json::to_string(redirect_uris).unwrap_or_else(|_| "[]".into()), auth_method, issued],
        )?;
        Ok(serde_json::json!({
            "client_id": client_id,
            "redirect_uris": redirect_uris,
            "token_endpoint_auth_method": auth_method,
            "client_id_issued_at": issued,
        }))
    }

    pub fn oauth_client(&self, client_id: &str) -> Result<Option<Value>> {
        if client_id.is_empty() {
            return Ok(None);
        }
        if client_id == "omabring" {
            return Ok(Some(serde_json::json!({
                "client_id": "omabring",
                "redirect_uris": Value::Null,
                "token_endpoint_auth_method": "none",
            })));
        }
        let conn = self.connect()?;
        let row = conn
            .query_row(
                "SELECT redirect_uris, token_endpoint_auth_method FROM oauth_clients WHERE client_id=?1",
                [client_id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)),
            )
            .optional()?;
        Ok(row.map(|(uris, method)| {
            let parsed: Value = serde_json::from_str(&uris).unwrap_or(Value::Null);
            serde_json::json!({
                "client_id": client_id,
                "redirect_uris": parsed,
                "token_endpoint_auth_method": method.unwrap_or_else(|| "none".into()),
            })
        }))
    }

    pub fn load_cart(&self, user_id: i64, retailer: &str) -> Result<Vec<Value>> {
        let conn = self.connect()?;
        let raw: Option<String> = conn
            .query_row(
                "SELECT items_json FROM carts WHERE user_id=?1 AND retailer=?2",
                rusqlite::params![user_id, retailer],
                |r| r.get(0),
            )
            .optional()?;
        Ok(raw.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default())
    }

    pub fn save_cart(&self, user_id: i64, retailer: &str, items: &[Value]) -> Result<()> {
        let conn = self.connect()?;
        conn.execute(
            "INSERT INTO carts(user_id, retailer, items_json) VALUES (?1,?2,?3)
             ON CONFLICT(user_id, retailer) DO UPDATE SET items_json=excluded.items_json",
            rusqlite::params![user_id, retailer, serde_json::to_string(items).unwrap_or_else(|_| "[]".into())],
        )?;
        Ok(())
    }

    pub fn create_order(&self, user_id: i64, retailer: &str, items: &Value, address: &str, status: &str, checkout_url: &str) -> Result<i64> {
        let conn = self.connect()?;
        conn.execute(
            "INSERT INTO orders(user_id, retailer, items_json, address, status, checkout_url, created_at) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            rusqlite::params![user_id, retailer, items.to_string(), address, status, checkout_url, now_secs()],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn record_quote(&self, user_id: i64, product_key: &str, retailer: &str, price: Option<f64>, found_name: &str, sku: &str, source: &str, error: &str, url: &str) -> Result<()> {
        let conn = self.connect()?;
        let fetched = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        conn.execute(
            "INSERT INTO catalog_prices(user_id, product_key, retailer, price, found_name, sku, source, error, fetched_at, url)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            rusqlite::params![user_id, product_key, retailer, price, found_name, sku, source, error, fetched, url],
        )?;
        Ok(())
    }

    pub fn latest_quotes(&self, user_id: i64, product_key: &str) -> Result<Vec<(String, Option<f64>, String, String, String, String, String)>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            "SELECT retailer, price, found_name, sku, error, fetched_at, url FROM catalog_prices
             WHERE user_id=?1 AND product_key=?2 ORDER BY fetched_at DESC, id DESC",
        )?;
        let rows = stmt.query_map(rusqlite::params![user_id, product_key], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<f64>>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, Option<String>>(6)?.unwrap_or_default(),
            ))
        })?;
        let mut seen = Vec::new();
        let mut out = Vec::new();
        for row in rows {
            let row = row?;
            if seen.iter().any(|s: &String| s == &row.0) {
                continue;
            }
            seen.push(row.0.clone());
            out.push(row);
        }
        Ok(out)
    }

    pub fn backup_settings(&self) -> Result<(String, String, Option<i64>)> {
        let conn = self.connect()?;
        let row = conn.query_row(
            "SELECT server_path, status, last_backup_at FROM backup_settings WHERE id=1",
            [],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<i64>>(2)?)),
        )?;
        Ok(row)
    }

    pub fn set_backup_path(&self, path: &str) -> Result<()> {
        let conn = self.connect()?;
        conn.execute(
            "UPDATE backup_settings SET server_path=?1, status='connected', connected_at=?2, last_check_error='' WHERE id=1",
            rusqlite::params![path, now_secs()],
        )?;
        Ok(())
    }

    pub fn set_backup_status(&self, status: &str, error: &str) -> Result<()> {
        let conn = self.connect()?;
        conn.execute(
            "UPDATE backup_settings SET status=?1, last_check_error=?2, last_check_at=?3 WHERE id=1",
            rusqlite::params![status, error, now_secs()],
        )?;
        Ok(())
    }

    pub fn set_last_backup_at(&self, ts: i64) -> Result<()> {
        let conn = self.connect()?;
        conn.execute("UPDATE backup_settings SET last_backup_at=?1, status='connected', last_check_error='' WHERE id=1", [ts])?;
        Ok(())
    }
}

fn row_user(r: &rusqlite::Row<'_>) -> rusqlite::Result<User> {
    Ok(User {
        id: r.get(0)?,
        email: r.get(1)?,
        password_hash: r.get(2)?,
        mcp_token: r.get(3)?,
        created_at: r.get(4)?,
    })
}

fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS users (
            id INTEGER PRIMARY KEY,
            email TEXT UNIQUE NOT NULL,
            password_hash TEXT NOT NULL,
            mcp_token TEXT UNIQUE NOT NULL,
            created_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS sessions (
            token TEXT PRIMARY KEY,
            user_id INTEGER NOT NULL,
            csrf TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            expires_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS retailer_accounts (
            id INTEGER PRIMARY KEY,
            user_id INTEGER NOT NULL,
            retailer TEXT NOT NULL,
            email TEXT,
            secret BLOB,
            address TEXT,
            UNIQUE(user_id, retailer)
        );
        CREATE TABLE IF NOT EXISTS orders (
            id INTEGER PRIMARY KEY,
            user_id INTEGER NOT NULL,
            retailer TEXT NOT NULL,
            items_json TEXT NOT NULL,
            address TEXT,
            status TEXT NOT NULL,
            checkout_url TEXT,
            created_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS carts (
            user_id INTEGER NOT NULL,
            retailer TEXT NOT NULL,
            items_json TEXT NOT NULL,
            PRIMARY KEY (user_id, retailer)
        );
        CREATE TABLE IF NOT EXISTS oauth_codes (
            code TEXT PRIMARY KEY,
            user_id INTEGER NOT NULL,
            redirect_uri TEXT,
            created_at INTEGER NOT NULL,
            client_id TEXT,
            code_challenge TEXT,
            code_challenge_method TEXT
        );
        CREATE TABLE IF NOT EXISTS oauth_clients (
            client_id TEXT PRIMARY KEY,
            client_secret TEXT,
            redirect_uris TEXT NOT NULL,
            token_endpoint_auth_method TEXT,
            created_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS store_flags (
            retailer TEXT PRIMARY KEY,
            enabled INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS store_probes (
            user_id INTEGER NOT NULL,
            retailer TEXT NOT NULL,
            capability TEXT NOT NULL,
            ok INTEGER NOT NULL,
            at INTEGER NOT NULL,
            error_code TEXT,
            PRIMARY KEY (user_id, retailer, capability)
        );
        CREATE TABLE IF NOT EXISTS password_resets (
            token TEXT PRIMARY KEY,
            user_id INTEGER NOT NULL,
            created_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS invoices (
            id INTEGER PRIMARY KEY,
            user_id INTEGER NOT NULL,
            retailer TEXT NOT NULL,
            invoice_no TEXT NOT NULL,
            order_no TEXT,
            invoice_date TEXT,
            store_name TEXT,
            gmail_id TEXT,
            source_file TEXT,
            UNIQUE(user_id, retailer, invoice_no)
        );
        CREATE TABLE IF NOT EXISTS invoice_items (
            id INTEGER PRIMARY KEY,
            invoice_id INTEGER NOT NULL,
            barcode TEXT,
            name TEXT NOT NULL,
            qty REAL NOT NULL,
            unit_price REAL,
            line_total REAL NOT NULL,
            image_url TEXT,
            product_key TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS product_meta (
            product_key TEXT PRIMARY KEY,
            sku TEXT,
            category TEXT,
            official_name TEXT,
            image_url TEXT,
            source TEXT,
            official_ean TEXT,
            macro_category TEXT
        );
        CREATE TABLE IF NOT EXISTS product_aliases (
            alias_key TEXT PRIMARY KEY,
            canonical_key TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS forecast_exclusions (
            user_id INTEGER NOT NULL,
            product_key TEXT NOT NULL,
            created_at TEXT NOT NULL,
            PRIMARY KEY (user_id, product_key)
        );
        CREATE TABLE IF NOT EXISTS forecast_votes (
            user_id INTEGER NOT NULL,
            product_key TEXT NOT NULL,
            vote TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (user_id, product_key)
        );
        CREATE TABLE IF NOT EXISTS catalog_prices (
            id INTEGER PRIMARY KEY,
            user_id INTEGER NOT NULL,
            product_key TEXT NOT NULL,
            retailer TEXT NOT NULL,
            price REAL,
            found_name TEXT,
            sku TEXT,
            source TEXT,
            error TEXT,
            fetched_at TEXT NOT NULL,
            url TEXT
        );
        CREATE TABLE IF NOT EXISTS backup_settings (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            server_path TEXT NOT NULL DEFAULT '',
            status TEXT NOT NULL DEFAULT 'disconnected',
            last_check_at INTEGER,
            last_check_error TEXT,
            last_backup_at INTEGER,
            connected_at INTEGER
        );
        CREATE INDEX IF NOT EXISTS idx_invoices_user_date ON invoices(user_id, invoice_date);
        CREATE INDEX IF NOT EXISTS idx_invoice_items_invoice ON invoice_items(invoice_id);
        CREATE INDEX IF NOT EXISTS idx_invoice_items_product ON invoice_items(product_key);
        CREATE INDEX IF NOT EXISTS idx_catalog_prices_lookup ON catalog_prices(user_id, product_key, retailer, fetched_at);
        "#,
    )?;
    for r in RETAILERS {
        conn.execute(
            "INSERT OR IGNORE INTO store_flags(retailer, enabled) VALUES (?1,?2)",
            rusqlite::params![r.id, if r.enabled_default { 1 } else { 0 }],
        )?;
    }
    let has: i64 = conn.query_row("SELECT COUNT(*) FROM backup_settings WHERE id=1", [], |r| r.get(0))?;
    if has == 0 {
        conn.execute("INSERT INTO backup_settings(id) VALUES (1)", [])?;
    }
    Ok(())
}

//! Product images, aliases, purchase history, backup, and app settings.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{NaiveDate, Utc};
use rusqlite::{params, OptionalExtension};

use crate::error::{Error, Result};
use crate::model::{junk_product_name, normalize_name, round_money, StoreId};
use crate::{now_rfc3339, App};

#[derive(Debug, Clone)]
pub struct PurchasePoint {
    pub date: NaiveDate,
    pub store: StoreId,
    pub qty: f64,
    pub unit_price: f64,
    pub invoice_id: i64,
    pub invoice_no: String,
}

#[derive(Debug, Clone)]
pub struct StorePrice {
    pub store: StoreId,
    pub last: f64,
    pub min: f64,
    pub max: f64,
    pub buys: u32,
}

/// A receipt product that still has no picture on this machine.
#[derive(Debug, Clone)]
pub struct MissingImage {
    pub name: String,
    pub store: StoreId,
    pub product_key: String,
}

#[derive(Debug, Clone)]
pub struct ProductHistory {
    pub name: String,
    pub times: u32,
    pub spent: f64,
    pub avg: f64,
    pub min: f64,
    pub max: f64,
    pub first: String,
    pub last: String,
    pub frequency: String,
    pub likely: u8,
    pub next: String,
    pub points: Vec<PurchasePoint>,
    pub by_store: Vec<StorePrice>,
}

impl App {
    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT value FROM app_settings WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()
            .map_err(Error::from)
        })
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO app_settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )?;
            Ok(())
        })
    }

    pub fn backup_dir(&self) -> PathBuf {
        self.setting("backup_dir")
            .ok()
            .flatten()
            .filter(|value| !value.trim().is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(default_backup_dir)
    }

    pub fn set_backup_dir(&self, path: &Path) -> Result<()> {
        self.set_setting("backup_dir", &path.to_string_lossy())
    }

    pub fn last_backup_at(&self) -> Option<String> {
        self.setting("last_backup").ok().flatten()
    }

    pub fn backup_now(&self) -> Result<String> {
        let dir = self.backup_dir();
        fs::create_dir_all(&dir)?;
        let stamp = Utc::now().format("%Y%m%d-%H%M%S");
        let dest = dir.join(format!("omabring-{stamp}.db"));
        let sql = format!(
            "VACUUM INTO '{}'",
            dest.to_string_lossy().replace('\'', "''")
        );
        let db_path = self.data_dir().join("omabring.db");
        let conn = rusqlite::Connection::open(&db_path)?;
        conn.execute_batch(&sql)
            .map_err(|err| Error::Message(format!("backup failed: {err}")))?;
        set_mode_file(&dest);
        prune_backups(&dir);
        let when = now_rfc3339();
        self.set_setting("last_backup", &when)?;
        Ok(when)
    }

    /// Writes a backup when the last one is missing or older than a day.
    pub fn backup_if_due(&self) -> Result<Option<String>> {
        if let Some(last) = self.last_backup_at() {
            if let Ok(then) = chrono::DateTime::parse_from_rfc3339(&last) {
                let age = Utc::now().signed_duration_since(then.with_timezone(&Utc));
                if age.num_hours() < 24 {
                    return Ok(None);
                }
            }
        }
        if self.backup_dir().as_os_str().is_empty() {
            return Ok(None);
        }
        self.backup_now().map(Some)
    }

    pub fn product_image(&self, name: &str) -> Option<PathBuf> {
        for key in image_lookup_keys(self, name) {
            if let Some(path) = image_file(self, &key) {
                return Some(path);
            }
        }
        None
    }

    pub fn store_product_image(&self, name: &str, bytes: &[u8]) -> Result<PathBuf> {
        let ext = image_ext(bytes).ok_or_else(|| Error::Message("not an image".into()))?;
        let key = normalize_name(name);
        let file_name = format!("{}.{}", safe_key(&key), ext);
        let dir = self.data_dir().join("product-images");
        fs::create_dir_all(&dir)?;
        let path = dir.join(&file_name);
        {
            use std::os::unix::fs::OpenOptionsExt;
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o644)
                .open(&path)?;
            file.write_all(bytes)?;
        }
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO product_images (name_key, path, source_url) VALUES (?1, ?2, '')
                 ON CONFLICT(name_key) DO UPDATE SET path = excluded.path",
                params![key, file_name],
            )?;
            Ok(())
        })?;
        Ok(path)
    }

    /// True when this name was looked up in the last week, hit or miss.
    pub fn image_search_recent(&self, name: &str) -> Result<bool> {
        let key = normalize_name(name);
        if key.is_empty() {
            return Ok(false);
        }
        let searched_at: Option<String> = self.with_conn(|conn| {
            conn.query_row(
                "SELECT searched_at FROM image_searches WHERE name_key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()
            .map_err(Error::from)
        })?;
        let Some(searched_at) = searched_at else {
            return Ok(false);
        };
        let Ok(then) = chrono::DateTime::parse_from_rfc3339(&searched_at) else {
            return Ok(false);
        };
        let age = Utc::now().signed_duration_since(then.with_timezone(&Utc));
        Ok(age.num_hours() < 24 * 7)
    }

    /// Remembers a catalog search so the next import does not repeat it.
    pub fn remember_image_search(&self, name: &str, found: bool) -> Result<()> {
        let key = normalize_name(name);
        if key.is_empty() {
            return Ok(());
        }
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO image_searches (name_key, searched_at, found) VALUES (?1, ?2, ?3)
                 ON CONFLICT(name_key) DO UPDATE SET searched_at = excluded.searched_at, found = excluded.found",
                params![key, now_rfc3339(), i64::from(found)],
            )?;
            Ok(())
        })
    }

    /// Receipt names with no saved picture, and the store that sold them.
    pub fn products_missing_images(&self) -> Result<Vec<MissingImage>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT MIN(l.name), MIN(i.store), MAX(ifnull(l.product_key, ''))
                 FROM invoice_lines l
                 JOIN invoices i ON i.id = l.invoice_id
                 WHERE NOT EXISTS (
                   SELECT 1 FROM product_images p
                   WHERE p.name_key = l.name_key
                      OR (l.product_key != '' AND p.name_key = l.product_key)
                 )
                 GROUP BY l.name_key
                 ORDER BY MIN(l.name)",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            let mut out = Vec::new();
            for row in rows {
                let (name, store, product_key) = row?;
                let Some(store) = StoreId::parse(&store) else {
                    continue;
                };
                out.push(MissingImage {
                    name,
                    store,
                    product_key,
                });
            }
            Ok(out)
        })
    }

    /// Picture URLs already stored on products imported from Bring Fast.
    pub fn stored_image_urls(&self) -> Result<std::collections::HashMap<String, Vec<String>>> {
        self.with_conn(|conn| {
            let mut map = std::collections::HashMap::new();
            let mut stmt = conn.prepare(
                "SELECT product_key, name, ifnull(image_url, '') FROM products WHERE ifnull(image_url, '') != ''",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            for row in rows {
                let (key, name, url) = row?;
                push_image_url(&mut map, &key, &url);
                push_image_url(&mut map, &normalize_name(&name), &url);
            }
            Ok(map)
        })
    }

    /// Copies a Bring Fast cache file or downloads a stored image URL.
    pub fn adopt_product_image(
        &self,
        name: &str,
        keys: &[String],
        urls: &[String],
        dirs: &[PathBuf],
    ) -> Result<bool> {
        if self.product_image(name).is_some() {
            return Ok(true);
        }
        if let Some(bytes) = find_cached_image(keys, urls, dirs) {
            self.store_product_image(name, &bytes)?;
            return Ok(true);
        }
        for url in urls {
            if let Some(bytes) = fetch_image(url) {
                self.store_product_image(name, &bytes)?;
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Downloads one catalog picture and stores it next to the other product images.
    pub fn cache_remote_image(&self, name: &str, url: &str) -> Result<bool> {
        if self.product_image(name).is_some() {
            return Ok(true);
        }
        let Some(bytes) = fetch_image(url) else {
            return Ok(false);
        };
        self.store_product_image(name, &bytes)?;
        Ok(true)
    }

    pub fn product_history(&self, name: &str) -> Result<ProductHistory> {
        let wanted = canonical_key(self, name);
        let invoices = self.list_invoices()?;
        let mut points = Vec::new();
        let mut display = name.to_string();
        for invoice in invoices {
            let Ok(date) = NaiveDate::parse_from_str(&invoice.purchased_on, "%Y-%m-%d") else {
                continue;
            };
            for line in &invoice.lines {
                let key = if line.product_key.is_empty() {
                    normalize_name(&line.name)
                } else {
                    canonical_stored(self, &line.product_key)
                };
                let name_key = normalize_name(&line.name);
                if key != wanted
                    && name_key != wanted
                    && canonical_stored(self, &name_key) != wanted
                {
                    continue;
                }
                if display.eq_ignore_ascii_case(name) || display.is_empty() {
                    display = line.name.clone();
                }
                points.push(PurchasePoint {
                    date,
                    store: invoice.store,
                    qty: line.qty,
                    unit_price: line.unit_price,
                    invoice_id: invoice.id,
                    invoice_no: invoice.invoice_no.clone(),
                });
            }
        }
        points.sort_by_key(|point| point.date);
        let spent = round_money(points.iter().map(|p| p.unit_price * p.qty).sum());
        let prices: Vec<f64> = points.iter().map(|p| p.unit_price).collect();
        let min = prices.iter().copied().fold(f64::MAX, f64::min);
        let max = prices.iter().copied().fold(0.0_f64, f64::max);
        let avg = if prices.is_empty() {
            0.0
        } else {
            round_money(prices.iter().sum::<f64>() / prices.len() as f64)
        };
        let (first, last) = match (points.first(), points.last()) {
            (Some(a), Some(b)) => (
                a.date.format("%Y-%m-%d").to_string(),
                b.date.format("%Y-%m-%d").to_string(),
            ),
            _ => (String::new(), String::new()),
        };
        let frequency = if points.len() < 2 {
            "once".into()
        } else {
            let span = (points.last().unwrap().date - points.first().unwrap().date).num_days();
            let every = span as f64 / (points.len() - 1) as f64;
            format!("every {:.0} days", every.max(0.0))
        };
        let today = Utc::now().date_naive();
        let (raw_likely, next) = likely_next(&points, today);
        let push = self
            .vote_index()
            .ok()
            .and_then(|votes| {
                votes
                    .get(&normalize_name(&display))
                    .copied()
                    .or_else(|| votes.get(&wanted).copied())
            })
            .unwrap_or(0);
        let likely = crate::shopping::apply_likely_vote(raw_likely, push);
        let mut by_store: Vec<StorePrice> = Vec::new();
        for point in &points {
            if let Some(row) = by_store.iter_mut().find(|row| row.store == point.store) {
                row.buys += 1;
                row.last = point.unit_price;
                row.min = row.min.min(point.unit_price);
                row.max = row.max.max(point.unit_price);
            } else {
                by_store.push(StorePrice {
                    store: point.store,
                    last: point.unit_price,
                    min: point.unit_price,
                    max: point.unit_price,
                    buys: 1,
                });
            }
        }
        Ok(ProductHistory {
            name: display,
            times: points.len() as u32,
            spent,
            avg,
            min: if prices.is_empty() {
                0.0
            } else {
                round_money(min)
            },
            max: round_money(max),
            first,
            last,
            frequency,
            likely,
            next,
            points,
            by_store,
        })
    }
}

pub fn default_backup_dir() -> PathBuf {
    crate::home_dir().join("Documents/OmaBring-backups")
}

pub fn flexible_day(raw: &str) -> Option<NaiveDate> {
    let raw = raw.trim();
    if raw.len() >= 10 {
        if let Ok(day) = NaiveDate::parse_from_str(&raw[..10], "%Y-%m-%d") {
            return Some(day);
        }
    }
    None
}

pub(crate) fn ensure_household(conn: &rusqlite::Connection) -> Result<()> {
    if !crate::column_exists(conn, "invoice_lines", "product_key")? {
        conn.execute(
            "ALTER TABLE invoice_lines ADD COLUMN product_key TEXT NOT NULL DEFAULT ''",
            [],
        )?;
    }
    purge_junk_lines(conn)?;
    Ok(())
}

fn purge_junk_lines(conn: &rusqlite::Connection) -> Result<()> {
    let ids = {
        let mut stmt = conn.prepare("SELECT id, name FROM invoice_lines")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut ids = Vec::new();
        for row in rows {
            let (id, name) = row?;
            if junk_product_name(&name) {
                ids.push(id);
            }
        }
        ids
    };
    for id in ids {
        conn.execute("DELETE FROM invoice_lines WHERE id = ?1", params![id])?;
    }
    Ok(())
}

fn canonical_key(app: &App, name: &str) -> String {
    canonical_stored(app, &normalize_name(name))
}

fn canonical_stored(app: &App, key: &str) -> String {
    let key = key.trim();
    if key.is_empty() {
        return String::new();
    }
    app.with_conn(|conn| {
        let mapped: Option<String> = conn
            .query_row(
                "SELECT canonical_key FROM product_aliases WHERE alias_key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()?;
        Ok(mapped.unwrap_or_else(|| key.to_string()))
    })
    .unwrap_or_else(|_| key.to_string())
}

fn likely_next(points: &[PurchasePoint], today: NaiveDate) -> (u8, String) {
    if points.len() < 2 {
        return (0, String::new());
    }
    let last = points.last().unwrap().date;
    let mut intervals = Vec::new();
    for pair in points.windows(2) {
        let gap = (pair[1].date - pair[0].date).num_days();
        if gap > 0 {
            intervals.push(gap as f64);
        }
    }
    if intervals.is_empty() {
        return (0, String::new());
    }
    intervals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median = intervals[intervals.len() / 2];
    let next = last + chrono::Duration::days(median.round() as i64);
    let until = (next - today).num_days();
    // A long gap does not wipe the score. A staple bought every few days
    // stays overdue, with a high likely, until it is bought again.
    let likely = if until <= 0 {
        90
    } else if until <= 2 {
        75
    } else if until <= 7 {
        55
    } else {
        30
    };
    (likely, next.format("%Y-%m-%d").to_string())
}

fn prune_backups(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let cutoff = Utc::now() - chrono::Duration::days(14);
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("omabring-") || !name.ends_with(".db") {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        let modified = chrono::DateTime::<Utc>::from(modified);
        if modified < cutoff {
            let _ = fs::remove_file(entry.path());
        }
    }
}

fn set_mode_file(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
}

pub(crate) fn image_ext(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        Some("png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("jpg")
    } else if bytes.len() > 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("webp")
    } else if bytes.starts_with(b"GIF8") {
        Some("gif")
    } else {
        None
    }
}

pub(crate) fn safe_key(key: &str) -> String {
    safe_key_len(key, 160)
}

/// Bring Fast names cache files with a 160-character key. Older OmaBring
/// files used 80. Lookups try both.
pub(crate) fn safe_key_names(key: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut push = |name: String| {
        if !names.iter().any(|have| have == &name) {
            names.push(name);
        }
    };
    for max in [160usize, 80] {
        push(safe_key_len(key, max));
        push(safe_key_collapsed(key, max));
    }
    names
}

/// Bring Fast names a picture `safe_key("name:{slug}")`, with runs of
/// punctuation collapsed to one hyphen.
pub fn bringfast_name_key(name: &str) -> String {
    let mut slug = String::new();
    let mut gap = false;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            if gap && !slug.is_empty() {
                slug.push('-');
            }
            gap = false;
            slug.push(c.to_ascii_lowercase());
        } else {
            gap = true;
        }
    }
    format!("name:{slug}")
}

fn safe_key_len(key: &str, max: usize) -> String {
    let cleaned: String = key
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(max)
        .collect();
    if cleaned.is_empty() {
        "product".into()
    } else {
        cleaned
    }
}

/// Bring Fast: `re.sub(r"[^a-zA-Z0-9._-]+", "_", key)[:max]`.
fn safe_key_collapsed(key: &str, max: usize) -> String {
    let mut out = String::new();
    let mut gap = false;
    for c in key.chars() {
        if out.chars().count() >= max {
            break;
        }
        if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
            if gap {
                out.push('_');
                gap = false;
                if out.chars().count() >= max {
                    break;
                }
            }
            out.push(c);
        } else {
            gap = true;
        }
    }
    if gap && out.chars().count() < max {
        out.push('_');
    }
    if out.is_empty() {
        "unknown".into()
    } else {
        out.chars().take(max).collect()
    }
}

fn image_file(app: &App, key: &str) -> Option<PathBuf> {
    let key = key.trim();
    if key.is_empty() {
        return None;
    }
    let relative: String = app
        .with_conn(|conn| {
            conn.query_row(
                "SELECT path FROM product_images WHERE name_key = ?1 COLLATE NOCASE",
                params![key],
                |row| row.get(0),
            )
            .optional()
            .map_err(Error::from)
        })
        .ok()
        .flatten()?;
    let path = app.data_dir().join("product-images").join(relative);
    path.is_file().then_some(path)
}

fn image_lookup_keys(app: &App, name: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let mut push = |key: &str| {
        let key = key.trim();
        if key.is_empty() || keys.iter().any(|have| have == key) {
            return;
        }
        keys.push(key.to_string());
    };
    push(&canonical_key(app, name));
    let wanted = normalize_name(name);
    let slug = bringfast_name_key(name);
    push(&wanted);
    push(&slug);
    let query_tokens = image_tokens(name);
    let _ = app.with_conn(|conn| {
        let mut related: Vec<(String, String, String)> = Vec::new();
        {
            let mut stmt = conn.prepare(
                "SELECT name, ifnull(product_key, ''), name_key FROM invoice_lines
                 WHERE name_key = ?1 OR lower(name) = lower(?2) OR product_key = ?1 OR product_key = ?3",
            )?;
            let rows = stmt.query_map(params![wanted, name, slug], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            for row in rows {
                related.push(row?);
            }
        }
        let product_keys: Vec<String> = related
            .iter()
            .map(|(_, product_key, _)| product_key.clone())
            .filter(|key| !key.trim().is_empty())
            .collect();
        for product_key in &product_keys {
            let mut stmt = conn.prepare(
                "SELECT name, ifnull(product_key, ''), name_key FROM invoice_lines
                 WHERE product_key = ?1",
            )?;
            let rows = stmt.query_map(params![product_key], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            for row in rows {
                let found = row?;
                if !related.iter().any(|have| have.0 == found.0 && have.1 == found.1) {
                    related.push(found);
                }
            }
        }
        if !product_keys.is_empty() {
            let mut stmt = conn.prepare(
                "SELECT name, product_key, product_key FROM products WHERE product_key = ?1",
            )?;
            for product_key in &product_keys {
                let rows = stmt.query_map(params![product_key], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })?;
                for row in rows {
                    related.push(row?);
                }
            }
        }
        for (line_name, product_key, name_key) in &related {
            push(line_name);
            push(&normalize_name(line_name));
            push(&bringfast_name_key(line_name));
            push(product_key);
            push(name_key);
        }
        let mut stored_keys = Vec::new();
        {
            let mut stmt = conn.prepare("SELECT name_key FROM product_images")?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            for row in rows {
                stored_keys.push(row?);
            }
        }
        let mut best: Option<(usize, String)> = None;
        for stored in stored_keys {
            if stored.eq_ignore_ascii_case(&wanted) || stored.eq_ignore_ascii_case(&slug) {
                push(&stored);
                continue;
            }
            let stored_tokens = image_tokens(&stored);
            if !image_tokens_match(&query_tokens, &stored_tokens) {
                continue;
            }
            let distance = stored_tokens.len().abs_diff(query_tokens.len());
            let closer = best
                .as_ref()
                .is_none_or(|(have, _)| distance < *have);
            if closer {
                best = Some((distance, stored));
            }
        }
        if let Some((_, stored)) = best {
            push(&stored);
        }
        Ok(())
    });
    keys
}

fn image_tokens(key: &str) -> Vec<String> {
    let mut raw = key.trim();
    if let Some(rest) = raw.strip_prefix("name:") {
        raw = rest;
    } else if let Some(rest) = raw.strip_prefix("ean:") {
        raw = rest;
    }
    let mut tokens = Vec::new();
    let mut current = String::new();
    for c in raw.chars() {
        if c.is_ascii_alphanumeric() {
            current.push(c.to_ascii_lowercase());
        } else if !current.is_empty() {
            push_image_token(&mut tokens, &current);
            current.clear();
        }
    }
    if !current.is_empty() {
        push_image_token(&mut tokens, &current);
    }
    tokens
}

fn push_image_token(tokens: &mut Vec<String>, token: &str) {
    if token.len() < 2 || image_size_token(token) {
        return;
    }
    tokens.push(token.to_string());
}

fn image_size_token(token: &str) -> bool {
    if token.chars().all(|c| c.is_ascii_digit()) {
        return true;
    }
    let letters: String = token.chars().filter(|c| c.is_ascii_alphabetic()).collect();
    let digits = token.chars().any(|c| c.is_ascii_digit());
    if digits
        && matches!(
            letters.as_str(),
            "ml" | "l" | "ltr" | "kg" | "g" | "pcs" | "pc" | "cl" | "oz" | "in" | "cm" | "mm"
        )
    {
        return true;
    }
    matches!(
        token,
        "ml" | "ltr"
            | "litre"
            | "liter"
            | "kg"
            | "pcs"
            | "pc"
            | "pet"
            | "bottle"
            | "pack"
            | "size"
            | "small"
            | "medium"
            | "large"
            | "xl"
            | "xxl"
    )
}

/// A one-word query must not swallow a longer product ("milk" ≠ "chocolate milk").
fn image_tokens_match(query: &[String], stored: &[String]) -> bool {
    if query.is_empty() || stored.is_empty() {
        return false;
    }
    if query.len() < 2 && stored.len() < 2 {
        return query == stored;
    }
    if query.len() >= 2 && tokens_in_order(stored, query) {
        return true;
    }
    stored.len() >= 2 && tokens_in_order(query, stored)
}

fn tokens_in_order(hay: &[String], needle: &[String]) -> bool {
    let mut index = 0;
    for token in hay {
        if token == &needle[index] {
            index += 1;
            if index == needle.len() {
                return true;
            }
        }
    }
    false
}

pub fn product_image_dirs(source: Option<&Path>, receipts: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut push = |path: PathBuf| {
        if path.is_dir() && !dirs.iter().any(|have| have == &path) {
            dirs.push(path);
        }
    };
    if let Some(source) = source {
        if let Some(parent) = source.parent() {
            push(parent.join("product-images"));
        }
    }
    if let Some(receipts) = receipts {
        push(receipts.join("product-images"));
        if let Some(parent) = receipts.parent() {
            push(parent.join("product-images"));
        }
    }
    if let Ok(data) = std::env::var("BRINGFAST_DATA") {
        if !data.trim().is_empty() {
            push(PathBuf::from(data.trim()).join("product-images"));
        }
    }
    push(crate::home_dir().join(".bring-fast/product-images"));
    dirs
}

fn push_image_url(map: &mut std::collections::HashMap<String, Vec<String>>, key: &str, url: &str) {
    let key = key.trim();
    let url = url.trim();
    if key.is_empty() || url.is_empty() {
        return;
    }
    let entry = map.entry(key.to_string()).or_default();
    if !entry.iter().any(|have| have == url) {
        entry.push(url.to_string());
    }
}

fn find_cached_image(keys: &[String], urls: &[String], dirs: &[PathBuf]) -> Option<Vec<u8>> {
    let mut files = Vec::new();
    for dir in dirs {
        for key in keys {
            for safe in safe_key_names(key) {
                for ext in ["webp", "png", "jpg", "jpeg", "gif"] {
                    files.push(dir.join(format!("{safe}.{ext}")));
                }
            }
        }
        for url in urls {
            let path = Path::new(url.trim());
            if path.is_file() {
                files.push(path.to_path_buf());
            }
            if let Some(name) = path.file_name() {
                files.push(dir.join(name));
            }
        }
    }
    for path in files {
        if let Ok(bytes) = fs::read(&path) {
            if image_ext(&bytes).is_some() {
                return Some(bytes);
            }
        }
    }
    None
}

pub(crate) fn fetch_image(url: &str) -> Option<Vec<u8>> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return None;
    }
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(12))
        .user_agent(
            "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
        )
        .build()
        .ok()?;
    let response = client.get(url).send().ok()?;
    if !response.status().is_success() {
        return None;
    }
    let mut bytes = Vec::new();
    response.take(2_000_000).read_to_end(&mut bytes).ok()?;
    image_ext(&bytes).map(|_| bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_roundtrip_and_product_history() {
        let dir = tempfile::tempdir().unwrap();
        let app = App::open(dir.path()).unwrap();
        let document = r#"{"store":"carrefour","invoice_no":"A","datetime":"2024-01-15T09:00:00Z","items":[{"name":"Milk","qty":1,"unit_price":8.5}]}"#;
        app.import_document(document).unwrap();
        let later = r#"{"store":"union_coop","invoice_no":"B","purchased_at":"2025-06-02 14:00:00","items":[{"name":"Milk","qty":2,"unit_price":9.0}]}"#;
        app.import_document(later).unwrap();
        let history = app.product_history("Milk").unwrap();
        assert_eq!(history.times, 2);
        assert_eq!(history.first, "2024-01-15");
        assert_eq!(history.last, "2025-06-02");
        assert_eq!(history.min, 8.5);
        assert_eq!(history.max, 9.0);
        assert_eq!(history.by_store.len(), 2);
        let png = include_bytes!("../../../assets/demo/milk.png");
        app.store_product_image("Milk", png).unwrap();
        assert!(app.product_image("Milk").unwrap().is_file());
        let missing = app.products_missing_images().unwrap();
        assert!(missing.iter().all(|item| item.name != "Milk"));
        assert!(app
            .cache_remote_image("Milk", "https://example.invalid/milk.png")
            .unwrap());
        let folder = dir.path().join("backups");
        app.set_backup_dir(&folder).unwrap();
        let when = app.backup_now().unwrap();
        assert!(!when.is_empty());
        assert_eq!(app.last_backup_at().as_deref(), Some(when.as_str()));
        assert!(folder.read_dir().unwrap().next().is_some());
        assert!(app.backup_if_due().unwrap().is_none());
    }

    #[test]
    fn cached_bringfast_file_and_relative_url_fill_a_missing_image() {
        let dir = tempfile::tempdir().unwrap();
        let app = App::open(dir.path().join("oma")).unwrap();
        let document = r#"{"store":"waitrose","invoice_no":"W","date":"2026-04-02","items":[{"name":"Oat Milk","qty":1,"unit_price":8}]}"#;
        app.import_document(document).unwrap();
        let cache = dir.path().join("product-images");
        fs::create_dir_all(&cache).unwrap();
        let png = include_bytes!("../../../assets/demo/milk.png");
        fs::write(cache.join("oat_milk.png"), png).unwrap();
        let keys = vec!["oat milk".into(), "sku-1".into()];
        let urls = vec!["/product-images/oat_milk.png".into()];
        assert!(app
            .adopt_product_image("Oat Milk", &keys, &urls, &[cache])
            .unwrap());
        assert!(app.product_image("Oat Milk").unwrap().is_file());
        assert!(app.products_missing_images().unwrap().is_empty());
    }

    #[test]
    fn a_regular_product_stays_likely_after_ninety_days() {
        let dir = tempfile::tempdir().unwrap();
        let app = App::open(dir.path()).unwrap();
        let today = Utc::now().date_naive();
        let last = today - chrono::Duration::days(216);
        for step in 0..6 {
            let date = last - chrono::Duration::days(3 * (5 - step));
            let document = format!(
                r#"{{"store":"carrefour","invoice_no":"C-{step}","date":"{date}","currency":"AED","items":[{{"name":"Coca Cola Zero","qty":1,"unit":"piece","unit_price":3.5}}]}}"#
            );
            app.import_document(&document).unwrap();
        }
        let history = app.product_history("Coca Cola Zero").unwrap();
        assert_eq!(history.times, 6);
        assert!(history.likely >= 80, "likely {}", history.likely);
        let next = NaiveDate::parse_from_str(&history.next, "%Y-%m-%d").unwrap();
        assert!(next < today, "{}", history.next);
        let names = safe_key_names("name:coca  cola");
        assert!(names.iter().any(|name| name == "name_coca_cola"));
        assert_eq!(
            bringfast_name_key("Coca Cola Zero 330ML"),
            "name:coca-cola-zero-330ml"
        );
    }

    #[test]
    fn junk_lines_are_removed_and_a_short_name_finds_its_image() {
        let dir = tempfile::tempdir().unwrap();
        let app = App::open(dir.path()).unwrap();
        app.with_conn(|conn| {
            conn.execute(
                "INSERT INTO invoices (store, invoice_no, purchased_on, currency, total) VALUES ('grandiose', 'G-1', '2026-10-01', 'AED', 40)",
                [],
            )?;
            let id: i64 = conn.query_row(
                "SELECT id FROM invoices WHERE invoice_no = 'G-1'",
                [],
                |row| row.get(0),
            )?;
            for name in [
                "Grandiose",
                "GRANDIOSE!!!",
                "Grand\u{200b}iose",
                "Large (13\")",
                "Tuna Steak",
            ] {
                conn.execute(
                    "INSERT INTO invoice_lines (invoice_id, name, name_key, qty, unit, unit_price, line_total, product_key) VALUES (?1, ?2, ?3, 1, 'piece', 10, 10, ?3)",
                    params![id, name, normalize_name(name)],
                )?;
            }
            Ok(())
        })
        .unwrap();
        let app = App::open(dir.path()).unwrap();
        let names: Vec<String> = app
            .list_invoices()
            .unwrap()
            .into_iter()
            .flat_map(|invoice| invoice.lines.into_iter().map(|line| line.name))
            .collect();
        assert_eq!(names, vec!["Tuna Steak".to_string()]);

        let png = include_bytes!("../../../assets/demo/milk.png");
        let images = app.data_dir().join("product-images");
        fs::create_dir_all(&images).unwrap();
        fs::write(images.join("oasis.png"), png).unwrap();
        fs::write(images.join("soleil.png"), png).unwrap();
        app.with_conn(|conn| {
            conn.execute(
                "INSERT INTO product_images (name_key, path, source_url) VALUES ('name:oasis-blu-sparkling', 'oasis.png', '')",
                [],
            )?;
            conn.execute(
                "INSERT INTO product_images (name_key, path, source_url) VALUES ('name:p-soleil', 'soleil.png', '')",
                [],
            )?;
            conn.execute(
                "INSERT INTO invoices (store, invoice_no, purchased_on, currency, total) VALUES ('carrefour', 'O-1', '2026-10-02', 'AED', 5)",
                [],
            )?;
            let id: i64 = conn.query_row(
                "SELECT id FROM invoices WHERE invoice_no = 'O-1'",
                [],
                |row| row.get(0),
            )?;
            conn.execute(
                "INSERT INTO invoice_lines (invoice_id, name, name_key, qty, unit, unit_price, line_total, product_key) VALUES (?1, 'Oasis Blu', 'oasis blu', 1, 'piece', 5, 5, 'name:oasis-blu-sparkling')",
                params![id],
            )?;
            Ok(())
        })
        .unwrap();
        assert!(app.product_image("Oasis Blu").unwrap().is_file());
        assert!(app.product_image("P.SOLEIL").unwrap().is_file());
        app.store_product_image("Rana Pesto Alla Genovese", png)
            .unwrap();
        assert!(app.product_image("RANA PESTO").unwrap().is_file());
        app.store_product_image("Al Jazira Eggs 30pcs", png)
            .unwrap();
        assert!(app.product_image("AL JAZIRA EGGS").unwrap().is_file());
        app.store_product_image("Chocolate Milk", png).unwrap();
        assert!(app.product_image("Milk").is_none());
    }

    #[test]
    fn image_search_is_remembered_for_a_week() {
        let dir = tempfile::tempdir().unwrap();
        let app = App::open(dir.path()).unwrap();
        assert!(!app.image_search_recent("Tuna Steak").unwrap());
        app.remember_image_search("Tuna Steak", false).unwrap();
        assert!(app.image_search_recent("tuna  steak").unwrap());
        let old = (Utc::now() - chrono::Duration::days(30)).to_rfc3339();
        app.with_conn(|conn| {
            conn.execute(
                "UPDATE image_searches SET searched_at = ?1 WHERE name_key = ?2",
                params![old, normalize_name("Tuna Steak")],
            )?;
            Ok(())
        })
        .unwrap();
        assert!(!app.image_search_recent("Tuna Steak").unwrap());
    }
}

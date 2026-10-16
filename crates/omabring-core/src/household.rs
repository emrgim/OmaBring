//! Product images, aliases, purchase history, backup, and app settings.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{NaiveDate, Utc};
use rusqlite::{params, OptionalExtension};

use crate::error::{Error, Result};
use crate::model::{normalize_name, round_money, StoreId};
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
        let key = canonical_key(self, name);
        let relative: String = self
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT path FROM product_images WHERE name_key = ?1",
                    params![key],
                    |row| row.get(0),
                )
                .optional()
                .map_err(Error::from)
            })
            .ok()
            .flatten()?;
        let path = self.data_dir().join("product-images").join(relative);
        path.is_file().then_some(path)
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

    /// Receipt names with no saved picture, and the store that sold them.
    pub fn products_missing_images(&self) -> Result<Vec<MissingImage>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT MIN(l.name), MIN(i.store)
                 FROM invoice_lines l
                 JOIN invoices i ON i.id = l.invoice_id
                 WHERE NOT EXISTS (
                   SELECT 1 FROM product_images p WHERE p.name_key = l.name_key
                 )
                 GROUP BY l.name_key
                 ORDER BY MIN(l.name)",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            let mut out = Vec::new();
            for row in rows {
                let (name, store) = row?;
                let Some(store) = StoreId::parse(&store) else {
                    continue;
                };
                out.push(MissingImage { name, store });
            }
            Ok(out)
        })
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
        let (likely, next) = likely_next(&points, today);
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
    let age = (today - last).num_days();
    if age > 90 {
        return (0, String::new());
    }
    let mut intervals = Vec::new();
    for pair in points.windows(2) {
        intervals.push((pair[1].date - pair[0].date).num_days().max(1) as f64);
    }
    intervals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median = intervals[intervals.len() / 2];
    let next = last + chrono::Duration::days(median.round() as i64);
    let until = (next - today).num_days();
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
    let cleaned: String = key
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(80)
        .collect();
    if cleaned.is_empty() {
        "product".into()
    } else {
        cleaned
    }
}

pub(crate) fn fetch_image(url: &str) -> Option<Vec<u8>> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return None;
    }
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(8))
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
}

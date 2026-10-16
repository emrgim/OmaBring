//! Import a Bring Fast database into this machine's OmaBring store.
//!
//! The source is the sqlite file (`bringfast.db`) or a plain Postgres dump
//! (`pg_dump`, not custom format). Store passwords are Fernet tokens sealed
//! with `master.key` beside the database; they are opened and sealed again
//! with OmaBring's machine key. Running the import twice does not duplicate
//! receipts, products, cart lines, or shopping-list rows.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

use crate::crypto::{open_fernet, seal};
use crate::error::{Error, Result};
use crate::model::{StoreId, StoreSecrets};
use crate::now_rfc3339;
use crate::App;

#[derive(Debug, Clone)]
pub struct BringfastImport {
    pub dry_run: bool,
    pub receipts_dir: Option<std::path::PathBuf>,
    pub store_accounts: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportReport {
    pub dry_run: bool,
    pub invoices_new: usize,
    pub invoices_updated: usize,
    pub invoices_existing: usize,
    pub lines: usize,
    pub products_new: usize,
    pub products_existing: usize,
    pub images_new: usize,
    pub aliases_new: usize,
    pub carts_new: usize,
    pub carts_existing: usize,
    pub cart_lines: usize,
    pub shopping_new: usize,
    pub shopping_existing: usize,
    pub accounts_new: usize,
    pub accounts_existing: usize,
    pub accounts_skipped: usize,
    pub skipped_stores: Vec<String>,
    pub receipt_files: bool,
    pub files_scanned: usize,
    pub files_new: usize,
    pub files_existing: usize,
    pub files_unparsed: usize,
    pub note: String,
}

impl std::fmt::Display for ImportReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mode = if self.dry_run { "dry-run" } else { "imported" };
        writeln!(f, "bringfast {mode}")?;
        writeln!(
            f,
            "invoices: {} new, {} updated, {} already stored, {} lines",
            self.invoices_new, self.invoices_updated, self.invoices_existing, self.lines
        )?;
        writeln!(
            f,
            "products: {} new, {} already stored, {} images, {} aliases",
            self.products_new, self.products_existing, self.images_new, self.aliases_new
        )?;
        writeln!(
            f,
            "carts: {} new, {} already stored, {} lines",
            self.carts_new, self.carts_existing, self.cart_lines
        )?;
        writeln!(
            f,
            "shopping list: {} new, {} already stored",
            self.shopping_new, self.shopping_existing
        )?;
        writeln!(
            f,
            "store accounts: {} re-encrypted, {} already linked, {} skipped",
            self.accounts_new, self.accounts_existing, self.accounts_skipped
        )?;
        if !self.skipped_stores.is_empty() {
            writeln!(f, "skipped stores: {}", self.skipped_stores.join(", "))?;
        }
        if self.receipt_files {
            writeln!(
                f,
                "receipt files: {} scanned, {} new, {} already stored, {} unparsed",
                self.files_scanned, self.files_new, self.files_existing, self.files_unparsed
            )?;
        }
        if !self.note.is_empty() {
            writeln!(f, "{}", self.note)?;
        }
        Ok(())
    }
}

impl App {
    pub fn import_bringfast(&self, path: &Path, options: &BringfastImport) -> Result<ImportReport> {
        if !path.is_file() {
            return Err(Error::Message(format!(
                "no Bring Fast database at {}",
                path.display()
            )));
        }
        let source = open_source(path)?;
        let key = master_key(path);
        let mut report = ImportReport {
            dry_run: options.dry_run,
            invoices_new: 0,
            invoices_updated: 0,
            invoices_existing: 0,
            lines: 0,
            products_new: 0,
            products_existing: 0,
            images_new: 0,
            aliases_new: 0,
            carts_new: 0,
            carts_existing: 0,
            cart_lines: 0,
            shopping_new: 0,
            shopping_existing: 0,
            accounts_new: 0,
            accounts_existing: 0,
            accounts_skipped: 0,
            skipped_stores: Vec::new(),
            receipt_files: false,
            files_scanned: 0,
            files_new: 0,
            files_existing: 0,
            files_unparsed: 0,
            note: String::new(),
        };
        if options.store_accounts && key.is_none() && table_exists(&source, "retailer_accounts") {
            report.note =
                "store passwords were left untouched (master.key was not next to the database)"
                    .into();
        }
        if !options.store_accounts && report.note.is_empty() {
            report.note =
                "store sign-ins were left untouched (pass --with-store-accounts to import them)"
                    .into();
        }
        let receipts = options.receipts_dir.clone();
        self.with_conn(|dest| {
            import_invoices(
                &source,
                dest,
                self,
                options.dry_run,
                receipts.as_deref(),
                &mut report,
            )?;
            if let Some(dir) = receipts.as_deref() {
                crate::receipts::import_receipt_files(dest, options.dry_run, dir, &mut report)?;
            }
            import_products(
                &source,
                dest,
                self,
                options.dry_run,
                receipts.as_deref(),
                &mut report,
            )?;
            import_aliases(&source, dest, options.dry_run, &mut report)?;
            import_carts(&source, dest, options.dry_run, &mut report)?;
            import_shopping(&source, dest, options.dry_run, &mut report)?;
            if options.store_accounts {
                import_accounts(
                    &source,
                    dest,
                    options.dry_run,
                    &self.machine_key,
                    key.as_deref(),
                    &mut report,
                )?;
            }
            Ok(())
        })?;
        report.skipped_stores.sort();
        report.skipped_stores.dedup();
        Ok(report)
    }
}

fn open_source(path: &Path) -> Result<Connection> {
    let bytes = fs::read(path)?;
    if bytes.starts_with(b"SQLite format 3") {
        return Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(Error::from);
    }
    if bytes.starts_with(b"PGDMP") {
        return Err(Error::Message(
            "this is a custom-format Postgres dump; use a plain SQL dump (pg_dump without -Fc) or the sqlite file"
                .into(),
        ));
    }
    let text = String::from_utf8(bytes).map_err(|_| {
        Error::Message("Bring Fast import expects a sqlite file or a plain SQL dump".into())
    })?;
    if !text.to_ascii_lowercase().contains("insert into")
        && !text.to_ascii_uppercase().contains("COPY ")
    {
        return Err(Error::Message(
            "that file is not a Bring Fast sqlite database or a plain SQL dump".into(),
        ));
    }
    load_sql_dump(&text)
}

fn master_key(path: &Path) -> Option<String> {
    if let Ok(value) = std::env::var("BRINGFAST_MASTER_KEY") {
        let value = value.trim();
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }
    let dir = path.parent()?;
    for candidate in [dir.join("master.key"), dir.join("../master.key")] {
        if let Ok(text) = fs::read_to_string(candidate) {
            let text = text.trim();
            if !text.is_empty() {
                return Some(text.to_string());
            }
        }
    }
    None
}

fn table_exists(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
        params![name],
        |_| Ok(()),
    )
    .optional()
    .ok()
    .flatten()
    .is_some()
}

fn import_invoices(
    source: &Connection,
    dest: &Connection,
    app: &App,
    dry_run: bool,
    receipts_dir: Option<&Path>,
    report: &mut ImportReport,
) -> Result<()> {
    if !table_exists(source, "invoices") {
        return Ok(());
    }
    let mut claimed: HashSet<(String, String)> = HashSet::new();
    let mut stmt = source.prepare(
        "SELECT id, user_id, retailer, invoice_no, invoice_date FROM invoices ORDER BY user_id, id",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, Option<String>>(4)?,
        ))
    })?;
    for row in rows {
        let (id, user_id, retailer, invoice_no, invoice_date) = row?;
        let Some(store) = StoreId::parse(&retailer) else {
            push_skipped(&mut report.skipped_stores, &retailer);
            continue;
        };
        let plain = (store.as_str().to_string(), invoice_no.clone());
        let suffix_no = format!("{invoice_no}#{user_id}");
        let suffix = (store.as_str().to_string(), suffix_no);
        let key = if !claimed.contains(&plain) {
            claimed.insert(plain.clone());
            plain
        } else {
            suffix
        };
        let lines = invoice_lines(source, id)?;
        let total: f64 = lines.iter().map(|line| line.line_total).sum();
        let purchased = flexible_invoice_date(invoice_date.as_deref());
        if let Some((invoice_id, old_date)) = existing_invoice(dest, &key.0, &key.1)? {
            let date_missing = old_date.is_empty() || old_date == "1970-01-01";
            let replace_date = purchased != "1970-01-01" && (date_missing || purchased != old_date);
            if replace_date {
                report.invoices_updated += 1;
            } else {
                report.invoices_existing += 1;
            }
            report.lines += lines.len();
            if !dry_run {
                if replace_date {
                    dest.execute(
                        "UPDATE invoices SET purchased_on = ?1, total = ?2 WHERE id = ?3",
                        params![purchased, total, invoice_id],
                    )?;
                }
                dest.execute(
                    "DELETE FROM invoice_lines WHERE invoice_id = ?1",
                    params![invoice_id],
                )?;
                write_lines(dest, app, invoice_id, &lines, receipts_dir, dry_run, report)?;
            }
            continue;
        }
        report.invoices_new += 1;
        report.lines += lines.len();
        if dry_run {
            for line in &lines {
                consider_image(dest, app, line, receipts_dir, true, report)?;
            }
            continue;
        }
        dest.execute(
            "INSERT INTO invoices (store, invoice_no, purchased_on, currency, total) VALUES (?1, ?2, ?3, 'AED', ?4)",
            params![key.0, key.1, purchased, total],
        )?;
        let invoice_id = dest.last_insert_rowid();
        write_lines(dest, app, invoice_id, &lines, receipts_dir, false, report)?;
    }
    Ok(())
}

fn flexible_invoice_date(raw: Option<&str>) -> String {
    raw.and_then(crate::household::flexible_day)
        .map(|day| day.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "1970-01-01".into())
}

fn existing_invoice(
    dest: &Connection,
    store: &str,
    invoice_no: &str,
) -> Result<Option<(i64, String)>> {
    dest.query_row(
        "SELECT id, purchased_on FROM invoices WHERE store = ?1 AND invoice_no = ?2",
        params![store, invoice_no],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .optional()
    .map_err(Error::from)
}

fn write_lines(
    dest: &Connection,
    app: &App,
    invoice_id: i64,
    lines: &[Line],
    receipts_dir: Option<&Path>,
    dry_run: bool,
    report: &mut ImportReport,
) -> Result<()> {
    for line in lines {
        if !dry_run {
            dest.execute(
                "INSERT INTO invoice_lines (invoice_id, name, name_key, qty, unit, unit_price, line_total, product_key) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![invoice_id, line.name, crate::model::normalize_name(&line.name), line.qty, line.unit, line.unit_price, line.line_total, line.product_key],
            )?;
        }
        consider_image(dest, app, line, receipts_dir, dry_run, report)?;
    }
    Ok(())
}

struct Line {
    name: String,
    qty: f64,
    unit: String,
    unit_price: f64,
    line_total: f64,
    product_key: String,
    image_url: String,
}

fn invoice_lines(source: &Connection, invoice_id: i64) -> Result<Vec<Line>> {
    if !table_exists(source, "invoice_items") {
        return Ok(Vec::new());
    }
    let mut stmt = source.prepare(
        "SELECT name, qty, unit_price, line_total, ifnull(product_key, ''), ifnull(image_url, '') FROM invoice_items WHERE invoice_id = ?1 ORDER BY id",
    )?;
    let rows = stmt.query_map(params![invoice_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, f64>(1)?,
            row.get::<_, Option<f64>>(2)?,
            row.get::<_, f64>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
        ))
    })?;
    let mut lines = Vec::new();
    for row in rows {
        let (name, qty, unit_price, line_total, product_key, image_url) = row?;
        let qty = if qty <= 0.0 { 1.0 } else { qty };
        let unit_price = unit_price.unwrap_or_else(|| line_total / qty);
        lines.push(Line {
            unit: pack_label(&name),
            name,
            qty,
            unit_price,
            line_total,
            product_key,
            image_url,
        });
    }
    Ok(lines)
}

fn consider_image(
    dest: &Connection,
    app: &App,
    line: &Line,
    receipts_dir: Option<&Path>,
    dry_run: bool,
    report: &mut ImportReport,
) -> Result<()> {
    let key = if line.product_key.trim().is_empty() {
        crate::model::normalize_name(&line.name)
    } else {
        line.product_key.trim().to_string()
    };
    if image_known(dest, &key)? {
        return Ok(());
    }
    let name_key = crate::model::normalize_name(&line.name);
    let slug = crate::household::bringfast_name_key(&line.name);
    let bytes = locate_image(line, receipts_dir, &key)
        .or_else(|| {
            if name_key.is_empty() || name_key == key {
                None
            } else {
                locate_image(line, receipts_dir, &name_key)
            }
        })
        .or_else(|| {
            if slug == key || slug == name_key {
                None
            } else {
                locate_image(line, receipts_dir, &slug)
            }
        });
    let Some(bytes) = bytes else {
        return Ok(());
    };
    report.images_new += 1;
    if dry_run {
        return Ok(());
    }
    write_image(dest, app, &key, &line.image_url, &bytes)?;
    if key != name_key {
        write_image(dest, app, &name_key, &line.image_url, &bytes)?;
    }
    Ok(())
}

fn image_known(dest: &Connection, key: &str) -> Result<bool> {
    Ok(dest
        .query_row(
            "SELECT 1 FROM product_images WHERE name_key = ?1",
            params![key],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

fn write_image(dest: &Connection, app: &App, key: &str, source: &str, bytes: &[u8]) -> Result<()> {
    let Some(ext) = crate::household::image_ext(bytes) else {
        return Ok(());
    };
    let file_name = format!("{}.{}", crate::household::safe_key(key), ext);
    let dir = app.data_dir().join("product-images");
    fs::create_dir_all(&dir)?;
    fs::write(dir.join(&file_name), bytes)?;
    dest.execute(
        "INSERT INTO product_images (name_key, path, source_url) VALUES (?1, ?2, ?3)
         ON CONFLICT(name_key) DO UPDATE SET path = excluded.path",
        params![key, file_name, source],
    )?;
    Ok(())
}

/// Image URLs Bring Fast stored on receipt lines and product records.
pub fn bringfast_image_index(path: &Path) -> HashMap<String, Vec<String>> {
    let mut map = HashMap::new();
    let Ok(bytes) = fs::read(path) else {
        return map;
    };
    if !bytes.starts_with(b"SQLite format 3") {
        return map;
    }
    let Ok(conn) = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
    else {
        return map;
    };
    if table_exists(&conn, "invoice_items") {
        let _ = collect_image_urls(
            &conn,
            "SELECT ifnull(name, ''), ifnull(product_key, ''), ifnull(image_url, '') FROM invoice_items",
            &mut map,
        );
    }
    if table_exists(&conn, "product_meta") {
        let _ = collect_image_urls(
            &conn,
            "SELECT ifnull(official_name, ''), ifnull(product_key, ''), ifnull(image_url, '') FROM product_meta",
            &mut map,
        );
    }
    map
}

fn collect_image_urls(
    conn: &Connection,
    sql: &str,
    map: &mut HashMap<String, Vec<String>>,
) -> Result<()> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    for row in rows {
        let (name, key, url) = row?;
        push_url(map, &crate::model::normalize_name(&name), &url);
        push_url(map, key.trim(), &url);
    }
    Ok(())
}

fn push_url(map: &mut HashMap<String, Vec<String>>, key: &str, url: &str) {
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

fn locate_image(line: &Line, receipts_dir: Option<&Path>, key: &str) -> Option<Vec<u8>> {
    let mut candidates = Vec::new();
    for safe in crate::household::safe_key_names(key) {
        for dir in crate::household::product_image_dirs(None, receipts_dir) {
            for ext in ["webp", "png", "jpg", "jpeg", "gif"] {
                candidates.push(dir.join(format!("{safe}.{ext}")));
            }
        }
        if let Some(dir) = receipts_dir {
            for ext in ["webp", "png", "jpg", "jpeg", "gif"] {
                candidates.push(dir.join(format!("{safe}.{ext}")));
                candidates.push(dir.join("product-images").join(format!("{safe}.{ext}")));
            }
        }
    }
    if !line.image_url.is_empty() {
        let url = Path::new(&line.image_url);
        if url.is_file() {
            candidates.push(url.to_path_buf());
        }
    }
    for path in candidates {
        if let Ok(bytes) = fs::read(&path) {
            if crate::household::image_ext(&bytes).is_some() {
                return Some(bytes);
            }
        }
    }
    if line.image_url.starts_with("http://") || line.image_url.starts_with("https://") {
        return crate::household::fetch_image(&line.image_url);
    }
    None
}

fn import_products(
    source: &Connection,
    dest: &Connection,
    app: &App,
    dry_run: bool,
    receipts_dir: Option<&Path>,
    report: &mut ImportReport,
) -> Result<()> {
    if !table_exists(source, "product_meta") {
        return Ok(());
    }
    let mut stmt = source.prepare(
        "SELECT product_key, sku, category, official_name, image_url, source FROM product_meta",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, Option<String>>(5)?,
        ))
    })?;
    for row in rows {
        let (key, sku, category, official, image, source_name) = row?;
        let name = official.unwrap_or_else(|| key.clone());
        if name.trim().is_empty() {
            continue;
        }
        let image_url = image.clone().unwrap_or_default();
        if row_exists(dest, "products", "product_key", &key)? {
            report.products_existing += 1;
        } else {
            report.products_new += 1;
            if !dry_run {
                dest.execute(
                    "INSERT INTO products (product_key, sku, name, category, image_url, source) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![key, sku, name, category, image, source_name],
                )?;
            }
        }
        if !image_url.is_empty() {
            let line = Line {
                name,
                qty: 1.0,
                unit: String::new(),
                unit_price: 0.0,
                line_total: 0.0,
                product_key: key,
                image_url,
            };
            consider_image(dest, app, &line, receipts_dir, dry_run, report)?;
        }
    }
    Ok(())
}

fn import_aliases(
    source: &Connection,
    dest: &Connection,
    dry_run: bool,
    report: &mut ImportReport,
) -> Result<()> {
    if !table_exists(source, "product_aliases") {
        return Ok(());
    }
    let mut stmt = source.prepare("SELECT alias_key, canonical_key FROM product_aliases")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (alias, canonical) = row?;
        if alias.trim().is_empty() || canonical.trim().is_empty() {
            continue;
        }
        let exists = dest
            .query_row(
                "SELECT canonical_key FROM product_aliases WHERE alias_key = ?1",
                params![alias],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        if exists.as_deref() == Some(canonical.as_str()) {
            continue;
        }
        report.aliases_new += 1;
        if dry_run {
            continue;
        }
        dest.execute(
            "INSERT INTO product_aliases (alias_key, canonical_key) VALUES (?1, ?2)
             ON CONFLICT(alias_key) DO UPDATE SET canonical_key = excluded.canonical_key",
            params![alias, canonical],
        )?;
    }
    Ok(())
}

fn import_carts(
    source: &Connection,
    dest: &Connection,
    dry_run: bool,
    report: &mut ImportReport,
) -> Result<()> {
    if !table_exists(source, "carts") {
        return Ok(());
    }
    let mut stmt = source
        .prepare("SELECT user_id, retailer, items_json FROM carts ORDER BY user_id, retailer")?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    let mut claimed: HashSet<(String, String)> = HashSet::new();
    for row in rows {
        let (user_id, retailer, items_json) = row?;
        let Some(store) = StoreId::parse(&retailer) else {
            push_skipped(&mut report.skipped_stores, &retailer);
            continue;
        };
        let items = cart_items(&items_json);
        for (index, item) in items.into_iter().enumerate() {
            let base_id = if item.id.is_empty() {
                format!("line-{index}")
            } else {
                item.id.clone()
            };
            let plain = (store.as_str().to_string(), base_id.clone());
            let line_id = if !claimed.contains(&plain) {
                claimed.insert(plain);
                base_id
            } else {
                format!("{base_id}#{user_id}")
            };
            if row_exists_2(
                dest,
                "cart_lines",
                "store",
                store.as_str(),
                "line_id",
                &line_id,
            )? {
                report.carts_existing += 1;
                continue;
            }
            report.carts_new += 1;
            report.cart_lines += 1;
            if dry_run {
                continue;
            }
            dest.execute(
                "INSERT INTO cart_lines (store, line_id, product_id, name, qty, unit, unit_price, currency) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![store.as_str(), line_id, item.id, item.name, item.qty, item.unit, item.price, item.currency],
            )?;
        }
    }
    Ok(())
}

struct CartItem {
    id: String,
    name: String,
    qty: f64,
    unit: String,
    price: Option<f64>,
    currency: String,
}

fn cart_items(raw: &str) -> Vec<CartItem> {
    let Ok(value) = serde_json::from_str::<Value>(raw) else {
        return Vec::new();
    };
    let currency = value
        .get("currency")
        .and_then(Value::as_str)
        .unwrap_or("AED")
        .to_string();
    let items = value
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .or_else(|| value.as_array().cloned())
        .unwrap_or_default();
    items
        .into_iter()
        .filter_map(|item| {
            let name = item
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            if name.is_empty() {
                return None;
            }
            let id = item
                .get("id")
                .or_else(|| item.get("sku"))
                .map(|value| match value {
                    Value::String(text) => text.clone(),
                    Value::Number(number) => number.to_string(),
                    _ => String::new(),
                })
                .unwrap_or_default();
            let qty = item
                .get("qty")
                .or_else(|| item.get("quantity"))
                .and_then(Value::as_f64)
                .unwrap_or(1.0);
            let price = item
                .get("price")
                .or_else(|| item.get("unit_price"))
                .and_then(Value::as_f64);
            Some(CartItem {
                unit: pack_label(&name),
                id,
                name,
                qty: if qty <= 0.0 { 1.0 } else { qty },
                price,
                currency: currency.clone(),
            })
        })
        .collect()
}

fn import_shopping(
    source: &Connection,
    dest: &Connection,
    dry_run: bool,
    report: &mut ImportReport,
) -> Result<()> {
    let mut rows: HashMap<String, (String, String, i64)> = HashMap::new();
    if table_exists(source, "forecast_votes") {
        let mut stmt = source.prepare(
            "SELECT product_key, vote FROM forecast_votes ORDER BY user_id, product_key",
        )?;
        let found = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in found {
            let (key, vote) = row?;
            rows.entry(key.clone()).or_insert((key, vote, 0));
        }
    }
    if table_exists(source, "forecast_exclusions") {
        let mut stmt = source
            .prepare("SELECT product_key FROM forecast_exclusions ORDER BY user_id, product_key")?;
        let found = stmt.query_map([], |row| row.get::<_, String>(0))?;
        for row in found {
            let key = row?;
            rows.entry(key.clone())
                .and_modify(|entry| entry.2 = 1)
                .or_insert((key, String::new(), 1));
        }
    }
    for (key, (_, vote, excluded)) in rows {
        let name = product_name(source, &key).unwrap_or_else(|| key.clone());
        if row_exists(dest, "shopping_list", "product_key", &key)? {
            report.shopping_existing += 1;
            continue;
        }
        report.shopping_new += 1;
        if dry_run {
            continue;
        }
        dest.execute(
            "INSERT INTO shopping_list (product_key, name, vote, excluded) VALUES (?1, ?2, ?3, ?4)",
            params![key, name, vote, excluded],
        )?;
    }
    Ok(())
}

fn product_name(source: &Connection, key: &str) -> Option<String> {
    if table_exists(source, "product_meta") {
        if let Ok(Some(name)) = source
            .query_row(
                "SELECT official_name FROM product_meta WHERE product_key = ?1",
                params![key],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
        {
            if let Some(name) = name {
                if !name.trim().is_empty() {
                    return Some(name);
                }
            }
        }
    }
    if table_exists(source, "invoice_items") {
        if let Ok(Some(name)) = source
            .query_row(
                "SELECT name FROM invoice_items WHERE product_key = ?1 ORDER BY id DESC LIMIT 1",
                params![key],
                |row| row.get::<_, String>(0),
            )
            .optional()
        {
            if !name.trim().is_empty() {
                return Some(name);
            }
        }
    }
    None
}

fn import_accounts(
    source: &Connection,
    dest: &Connection,
    dry_run: bool,
    machine_key: &[u8; 32],
    fernet_key: Option<&str>,
    report: &mut ImportReport,
) -> Result<()> {
    if !table_exists(source, "retailer_accounts") {
        return Ok(());
    }
    let mut stmt = source.prepare(
        "SELECT user_id, retailer, email, secret FROM retailer_accounts ORDER BY user_id, retailer",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, Option<Vec<u8>>>(3)?,
        ))
    })?;
    let mut seen = HashSet::new();
    for row in rows {
        let (_user_id, retailer, email, secret) = row?;
        let Some(store) = StoreId::parse(&retailer) else {
            push_skipped(&mut report.skipped_stores, &retailer);
            continue;
        };
        if !seen.insert(store.as_str().to_string()) {
            report.accounts_existing += 1;
            continue;
        }
        if row_exists(dest, "store_accounts", "store", store.as_str())? {
            report.accounts_existing += 1;
            continue;
        }
        let Some(fernet_key) = fernet_key else {
            report.accounts_skipped += 1;
            continue;
        };
        let Some(secret) = secret else {
            report.accounts_skipped += 1;
            continue;
        };
        let token = String::from_utf8_lossy(&secret);
        let Ok(plain) = open_fernet(fernet_key, token.trim()) else {
            report.accounts_skipped += 1;
            continue;
        };
        let Ok(payload) = serde_json::from_slice::<Value>(&plain) else {
            report.accounts_skipped += 1;
            continue;
        };
        let username = payload
            .get("email")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(str::trim)
            .or(email
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty()))
            .unwrap_or("")
            .to_string();
        let password = payload
            .get("password")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if username.is_empty() || password.is_empty() {
            report.accounts_skipped += 1;
            continue;
        }
        let token = payload
            .get("auth_token")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let user_id = payload
            .get("store_user_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        report.accounts_new += 1;
        if dry_run {
            continue;
        }
        let secrets = StoreSecrets {
            username,
            password,
            base_url: None,
            token,
            user_id,
        };
        let plaintext =
            serde_json::to_vec(&secrets).map_err(|err| Error::Message(err.to_string()))?;
        let blob = seal(machine_key, &plaintext)?;
        dest.execute(
            "INSERT INTO store_accounts (store, ciphertext, updated_at) VALUES (?1, ?2, ?3)",
            params![store.as_str(), blob, now_rfc3339()],
        )?;
    }
    Ok(())
}

fn row_exists(dest: &Connection, table: &str, column: &str, value: &str) -> Result<bool> {
    let sql = format!("SELECT 1 FROM {table} WHERE {column} = ?1");
    Ok(dest
        .query_row(&sql, params![value], |_| Ok(()))
        .optional()?
        .is_some())
}

fn row_exists_2(
    dest: &Connection,
    table: &str,
    column_a: &str,
    value_a: &str,
    column_b: &str,
    value_b: &str,
) -> Result<bool> {
    let sql = format!("SELECT 1 FROM {table} WHERE {column_a} = ?1 AND {column_b} = ?2");
    Ok(dest
        .query_row(&sql, params![value_a, value_b], |_| Ok(()))
        .optional()?
        .is_some())
}

fn push_skipped(stores: &mut Vec<String>, retailer: &str) {
    let name = retailer.trim();
    if !name.is_empty() && !stores.iter().any(|item| item == name) {
        stores.push(name.to_string());
    }
}

fn pack_label(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    for (raw, label) in [
        ("ml", "ml"),
        ("kg", "kg"),
        ("ltr", "L"),
        ("litre", "L"),
        ("liter", "L"),
    ] {
        if lower
            .split(|ch: char| !ch.is_ascii_alphanumeric())
            .any(|token| {
                token
                    .strip_prefix(|c: char| c.is_ascii_digit() || c == '.')
                    .unwrap_or(token)
                    == raw
                    || token.ends_with(raw)
            })
        {
            let _ = label;
        }
    }
    if lower.contains("ml") {
        return measure_from(&lower, &["ml"], "ml");
    }
    if lower.contains("kg") {
        return measure_from(&lower, &["kg"], "kg");
    }
    if lower.contains("ltr") || lower.contains("litre") || lower.contains("liter") {
        return measure_from(&lower, &["ltr", "litre", "liter", "l"], "L");
    }
    if lower.contains("pack") {
        return "pack".into();
    }
    "piece".into()
}

fn measure_from(lower: &str, suffixes: &[&str], label: &str) -> String {
    let bytes = lower.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index].is_ascii_digit() {
            let start = index;
            while index < bytes.len() && (bytes[index].is_ascii_digit() || bytes[index] == b'.') {
                index += 1;
            }
            let number = &lower[start..index];
            let rest = lower[index..].trim_start();
            if suffixes.iter().any(|suffix| rest.starts_with(suffix)) {
                return format!("{number} {label}");
            }
        } else {
            index += 1;
        }
    }
    label.to_string()
}

const MEMORY_SCHEMA: &str = r#"
CREATE TABLE invoices (
    id INTEGER PRIMARY KEY,
    user_id INTEGER NOT NULL,
    retailer TEXT NOT NULL,
    invoice_no TEXT NOT NULL,
    order_no TEXT,
    invoice_date TEXT,
    store_name TEXT,
    gmail_id TEXT,
    source_file TEXT
);
CREATE TABLE invoice_items (
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
CREATE TABLE product_meta (
    product_key TEXT PRIMARY KEY,
    sku TEXT,
    category TEXT,
    official_name TEXT,
    image_url TEXT,
    source TEXT
);
CREATE TABLE product_aliases (
    alias_key TEXT PRIMARY KEY,
    canonical_key TEXT NOT NULL
);
CREATE TABLE carts (
    user_id INTEGER NOT NULL,
    retailer TEXT NOT NULL,
    items_json TEXT NOT NULL
);
CREATE TABLE forecast_votes (
    user_id INTEGER NOT NULL,
    product_key TEXT NOT NULL,
    vote TEXT NOT NULL,
    updated_at TEXT
);
CREATE TABLE forecast_exclusions (
    user_id INTEGER NOT NULL,
    product_key TEXT NOT NULL,
    created_at TEXT
);
CREATE TABLE retailer_accounts (
    id INTEGER PRIMARY KEY,
    user_id INTEGER NOT NULL,
    retailer TEXT NOT NULL,
    email TEXT,
    secret BLOB,
    address TEXT
);
"#;

fn load_sql_dump(text: &str) -> Result<Connection> {
    let conn = Connection::open_in_memory()?;
    conn.execute_batch(MEMORY_SCHEMA)?;
    let mut statement = String::new();
    let mut copying: Option<CopyHeader> = None;
    for line in text.lines() {
        if let Some(header) = &copying {
            if line.trim() == "\\." {
                copying = None;
            } else {
                insert_tuple(&conn, &header.table, &header.columns, &split_copy(line))?;
            }
            continue;
        }
        let trimmed = line.trim();
        if statement.is_empty() {
            if let Some(header) = parse_copy_header(trimmed) {
                copying = Some(header);
                continue;
            }
        }
        if statement.is_empty() && !trimmed.to_ascii_uppercase().starts_with("INSERT INTO") {
            continue;
        }
        if !statement.is_empty() || trimmed.to_ascii_uppercase().starts_with("INSERT INTO") {
            statement.push_str(line);
            statement.push('\n');
            if trimmed.ends_with(';') {
                insert_statement(&conn, &statement)?;
                statement.clear();
            }
        }
    }
    Ok(conn)
}

struct CopyHeader {
    table: String,
    columns: Vec<String>,
}

fn parse_copy_header(line: &str) -> Option<CopyHeader> {
    let upper = line.to_ascii_uppercase();
    if !upper.starts_with("COPY ") || !upper.contains(" FROM STDIN") {
        return None;
    }
    let rest = line[5..].trim();
    let open = rest.find('(')?;
    let close = rest.find(')')?;
    let table = rest[..open].trim();
    let table = table
        .rsplit('.')
        .next()
        .unwrap_or(table)
        .trim_matches('"')
        .to_string();
    let columns = rest[open + 1..close]
        .split(',')
        .map(|column| column.trim().trim_matches('"').to_string())
        .filter(|column| !column.is_empty())
        .collect();
    Some(CopyHeader { table, columns })
}

fn split_copy(line: &str) -> Vec<Option<String>> {
    let mut values = Vec::new();
    let mut current = String::new();
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next() {
                Some('N') => {
                    values.push(None);
                    current.clear();
                    if chars.peek() == Some(&'\t') {
                        chars.next();
                    }
                    continue;
                }
                Some('t') => current.push('\t'),
                Some('n') => current.push('\n'),
                Some('\\') => current.push('\\'),
                Some(other) => current.push(other),
                None => current.push('\\'),
            }
        } else if ch == '\t' {
            values.push(Some(current.clone()));
            current.clear();
        } else {
            current.push(ch);
        }
    }
    if !current.is_empty() || line.ends_with('\t') {
        values.push(Some(current));
    }
    values
}

fn insert_statement(conn: &Connection, statement: &str) -> Result<()> {
    let lower = statement.to_ascii_lowercase();
    let Some(into) = lower.find("insert into") else {
        return Ok(());
    };
    let after = statement[into + "insert into".len()..].trim();
    let open = after
        .find('(')
        .ok_or_else(|| Error::Message("INSERT is missing columns".into()))?;
    let table = after[..open].trim();
    let table = table
        .rsplit('.')
        .next()
        .unwrap_or(table)
        .trim_matches('"')
        .to_string();
    let close = after
        .find(')')
        .ok_or_else(|| Error::Message("INSERT is missing columns".into()))?;
    let columns: Vec<String> = after[open + 1..close]
        .split(',')
        .map(|column| column.trim().trim_matches('"').to_string())
        .collect();
    let values_at = after
        .to_ascii_lowercase()
        .find("values")
        .ok_or_else(|| Error::Message("INSERT is missing VALUES".into()))?;
    let tuples = parse_tuples(&after[values_at + "values".len()..]);
    for tuple in tuples {
        insert_tuple(conn, &table, &columns, &tuple)?;
    }
    Ok(())
}

fn parse_tuples(input: &str) -> Vec<Vec<Option<String>>> {
    let mut rows = Vec::new();
    let mut row: Vec<Option<String>> = Vec::new();
    let mut value = String::new();
    let mut in_row = false;
    let mut in_string = false;
    let mut escape = false;
    let mut saw_value = false;
    for ch in input.chars() {
        if in_string {
            if escape {
                value.push(ch);
                escape = false;
                continue;
            }
            if ch == '\\' {
                escape = true;
                continue;
            }
            if ch == '\'' {
                in_string = false;
                continue;
            }
            value.push(ch);
            continue;
        }
        match ch {
            '(' if !in_row => {
                in_row = true;
                row.clear();
                value.clear();
                saw_value = false;
            }
            ')' if in_row => {
                row.push(sql_value(&value, saw_value));
                rows.push(row.clone());
                in_row = false;
                value.clear();
                saw_value = false;
            }
            ',' if in_row => {
                row.push(sql_value(&value, saw_value));
                value.clear();
                saw_value = false;
            }
            '\'' if in_row => in_string = true,
            ';' => break,
            other if in_row => {
                if !other.is_whitespace() || saw_value {
                    if !(other.is_whitespace() && value.is_empty()) {
                        value.push(other);
                        saw_value = true;
                    }
                }
            }
            _ => {}
        }
    }
    rows
}

fn sql_value(raw: &str, saw_value: bool) -> Option<String> {
    if !saw_value {
        return None;
    }
    let trimmed = raw.trim();
    if trimmed.eq_ignore_ascii_case("null") {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn insert_tuple(
    conn: &Connection,
    table: &str,
    columns: &[String],
    values: &[Option<String>],
) -> Result<()> {
    if !matches!(
        table,
        "invoices"
            | "invoice_items"
            | "product_meta"
            | "product_aliases"
            | "carts"
            | "forecast_votes"
            | "forecast_exclusions"
            | "retailer_accounts"
    ) {
        return Ok(());
    }
    if columns.len() != values.len() {
        return Ok(());
    }
    let mut known: HashMap<&str, Option<String>> = HashMap::new();
    for (column, value) in columns.iter().zip(values.iter()) {
        known.insert(column.as_str(), value.clone());
    }
    let get = |name: &str| known.get(name).cloned().flatten().unwrap_or_default();
    match table {
        "invoices" => {
            conn.execute(
                "INSERT INTO invoices (id, user_id, retailer, invoice_no, order_no, invoice_date, store_name, gmail_id, source_file) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![get("id"), get("user_id"), get("retailer"), get("invoice_no"), opt(&known, "order_no"), opt(&known, "invoice_date"), opt(&known, "store_name"), opt(&known, "gmail_id"), opt(&known, "source_file")],
            )?;
        }
        "invoice_items" => {
            conn.execute(
                "INSERT INTO invoice_items (id, invoice_id, barcode, name, qty, unit_price, line_total, image_url, product_key) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![get("id"), get("invoice_id"), opt(&known, "barcode"), get("name"), get("qty"), opt(&known, "unit_price"), get("line_total"), opt(&known, "image_url"), get("product_key")],
            )?;
        }
        "product_aliases" => {
            conn.execute(
                "INSERT OR REPLACE INTO product_aliases (alias_key, canonical_key) VALUES (?1, ?2)",
                params![get("alias_key"), get("canonical_key")],
            )?;
        }
        "product_meta" => {
            conn.execute(
                "INSERT OR REPLACE INTO product_meta (product_key, sku, category, official_name, image_url, source) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![get("product_key"), opt(&known, "sku"), opt(&known, "category"), opt(&known, "official_name"), opt(&known, "image_url"), opt(&known, "source")],
            )?;
        }
        "carts" => {
            conn.execute(
                "INSERT INTO carts (user_id, retailer, items_json) VALUES (?1, ?2, ?3)",
                params![get("user_id"), get("retailer"), get("items_json")],
            )?;
        }
        "forecast_votes" => {
            conn.execute(
                "INSERT INTO forecast_votes (user_id, product_key, vote, updated_at) VALUES (?1, ?2, ?3, ?4)",
                params![get("user_id"), get("product_key"), get("vote"), opt(&known, "updated_at")],
            )?;
        }
        "forecast_exclusions" => {
            conn.execute(
                "INSERT INTO forecast_exclusions (user_id, product_key, created_at) VALUES (?1, ?2, ?3)",
                params![get("user_id"), get("product_key"), opt(&known, "created_at")],
            )?;
        }
        "retailer_accounts" => {
            let secret = known.get("secret").cloned().flatten();
            conn.execute(
                "INSERT INTO retailer_accounts (id, user_id, retailer, email, secret, address) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![opt(&known, "id"), get("user_id"), get("retailer"), opt(&known, "email"), secret, opt(&known, "address")],
            )?;
        }
        _ => {}
    }
    Ok(())
}

fn opt(known: &HashMap<&str, Option<String>>, name: &str) -> Option<String> {
    known.get(name).cloned().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::seal_fernet;
    use base64::engine::general_purpose::URL_SAFE;
    use base64::Engine;

    fn sample_db(path: &Path, key: &str) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(MEMORY_SCHEMA).unwrap();
        conn.execute(
            "INSERT INTO invoices (id, user_id, retailer, invoice_no, invoice_date) VALUES (1, 1, 'carrefour', '1001', '2026-03-01')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO invoice_items (id, invoice_id, name, qty, unit_price, line_total, product_key) VALUES (1, 1, 'Fresh Milk 2L', 1, 12.5, 12.5, 'milk')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO product_meta (product_key, sku, official_name, source) VALUES ('milk', '123', 'Fresh Milk 2L', 'carrefour')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO carts (user_id, retailer, items_json) VALUES (1, 'spinneys', ?1)",
            params![r#"{"currency":"AED","items":[{"id":"335","name":"Aptamil 200ml","qty":2,"price":12.0}]}"#],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO forecast_votes (user_id, product_key, vote, updated_at) VALUES (1, 'milk', 'up', '2026-03-02')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO forecast_exclusions (user_id, product_key, created_at) VALUES (1, 'bag', '2026-03-02')",
            [],
        )
        .unwrap();
        let token = seal_fernet(
            key,
            br#"{"email":"shopper@example.com","password":"store-secret","auth_token":"tok"}"#,
        )
        .unwrap();
        conn.execute(
            "INSERT INTO retailer_accounts (id, user_id, retailer, email, secret) VALUES (1, 1, 'carrefour', 'shopper@example.com', ?1)",
            params![token.as_bytes()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO retailer_accounts (id, user_id, retailer, email, secret) VALUES (2, 1, 'amazon', 'a@b.c', ?1)",
            params![token.as_bytes()],
        )
        .unwrap();
    }

    #[test]
    fn import_is_idempotent_and_dry_run_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("bringfast.db");
        let key = URL_SAFE.encode([9u8; 32]);
        fs::write(dir.path().join("master.key"), &key).unwrap();
        sample_db(&source, &key);
        let app = App::open(dir.path().join("oma")).unwrap();
        let options = BringfastImport {
            dry_run: true,
            receipts_dir: None,
            store_accounts: true,
        };
        let preview = app.import_bringfast(&source, &options).unwrap();
        assert!(preview.dry_run);
        assert_eq!(preview.invoices_new, 1);
        assert_eq!(preview.lines, 1);
        assert_eq!(preview.products_new, 1);
        assert_eq!(preview.carts_new, 1);
        assert_eq!(preview.shopping_new, 2);
        assert_eq!(preview.accounts_new, 1);
        assert!(preview.skipped_stores.iter().any(|store| store == "amazon"));
        assert!(app.list_invoices().unwrap().is_empty());
        let text = preview.to_string();
        assert!(text.contains("dry-run"));
        assert!(text.contains("invoices: 1 new"));

        let write = BringfastImport {
            dry_run: false,
            receipts_dir: None,
            store_accounts: true,
        };
        let done = app.import_bringfast(&source, &write).unwrap();
        assert_eq!(done.invoices_new, 1);
        assert_eq!(done.accounts_new, 1);
        let again = app.import_bringfast(&source, &write).unwrap();
        assert_eq!(again.invoices_new, 0);
        assert_eq!(again.invoices_existing, 1);
        assert_eq!(again.products_existing, 1);
        assert_eq!(again.carts_existing, 1);
        assert_eq!(again.accounts_existing, 1);
        assert_eq!(app.list_invoices().unwrap().len(), 1);
        let secrets = app.secrets_for(StoreId::Carrefour).unwrap().unwrap();
        assert_eq!(secrets.username, "shopper@example.com");
        assert_eq!(secrets.password, "store-secret");
        assert_eq!(secrets.token.as_deref(), Some("tok"));
    }

    #[test]
    fn plain_sql_dump_imports_the_same_rows() {
        let dir = tempfile::tempdir().unwrap();
        let dump = dir.path().join("bringfast.sql");
        fs::write(
            &dump,
            r#"
COPY public.invoices (id, user_id, retailer, invoice_no, order_no, invoice_date, store_name, gmail_id, source_file) FROM stdin;
7	1	waitrose	W-9	\N	2026-04-02	\N	\N	\N
\.
COPY public.invoice_items (id, invoice_id, barcode, name, qty, unit_price, line_total, image_url, product_key) FROM stdin;
3	7	\N	Oat Milk 1L	2	8.5	17	\N	oat
\.
"#,
        )
        .unwrap();
        let app = App::open(dir.path().join("oma")).unwrap();
        let report = app
            .import_bringfast(
                &dump,
                &BringfastImport {
                    dry_run: false,
                    receipts_dir: None,
                    store_accounts: false,
                },
            )
            .unwrap();
        assert_eq!(report.invoices_new, 1);
        assert_eq!(report.lines, 1);
        let invoices = app.list_invoices().unwrap();
        assert_eq!(invoices[0].invoice_no, "W-9");
        assert_eq!(invoices[0].purchased_on, "2026-04-02");
        assert_eq!(invoices[0].store, StoreId::Waitrose);
    }

    #[test]
    fn reimport_backfills_the_invoice_date() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("bringfast.db");
        let conn = Connection::open(&source).unwrap();
        conn.execute_batch(MEMORY_SCHEMA).unwrap();
        conn.execute(
            "INSERT INTO invoices (id, user_id, retailer, invoice_no, invoice_date) VALUES (1, 1, 'carrefour', '1001', NULL)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO invoice_items (id, invoice_id, name, qty, unit_price, line_total, product_key, image_url) VALUES (1, 1, 'Milk', 1, 8, 8, 'milk', '')",
            [],
        )
        .unwrap();
        drop(conn);
        let app = App::open(dir.path().join("oma")).unwrap();
        let write = BringfastImport {
            dry_run: false,
            receipts_dir: None,
            store_accounts: false,
        };
        app.import_bringfast(&source, &write).unwrap();
        assert_eq!(app.list_invoices().unwrap()[0].purchased_on, "1970-01-01");
        let conn = Connection::open(&source).unwrap();
        conn.execute(
            "UPDATE invoices SET invoice_date = '2024-11-03T18:00:00Z' WHERE id = 1",
            [],
        )
        .unwrap();
        drop(conn);
        let again = app.import_bringfast(&source, &write).unwrap();
        assert_eq!(again.invoices_new, 0);
        assert_eq!(again.invoices_updated, 1);
        let invoices = app.list_invoices().unwrap();
        assert_eq!(invoices.len(), 1);
        assert_eq!(invoices[0].purchased_on, "2024-11-03");
    }

    #[test]
    fn careem_and_mcdonalds_import_and_receipt_files_dedup() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("bringfast.db");
        let conn = Connection::open(&source).unwrap();
        conn.execute_batch(MEMORY_SCHEMA).unwrap();
        conn.execute(
            "INSERT INTO invoices (id, user_id, retailer, invoice_no, invoice_date) VALUES (1, 1, 'careem', 'CRM-DB', '2026-08-01')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO invoice_items (id, invoice_id, name, qty, unit_price, line_total, product_key) VALUES (1, 1, 'Shawarma', 1, 20, 20, 'shawarma')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO invoices (id, user_id, retailer, invoice_no, invoice_date) VALUES (2, 1, 'mcdonalds', '1840105-0786-20260405', '2026-04-05')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO invoice_items (id, invoice_id, name, qty, unit_price, line_total, product_key) VALUES (2, 2, 'Cheeseburger', 1, 8, 8, 'burger')",
            [],
        )
        .unwrap();
        drop(conn);

        let receipts = dir.path().join("receipts");
        fs::create_dir_all(receipts.join("careem")).unwrap();
        fs::create_dir_all(receipts.join("mcdonalds")).unwrap();
        fs::write(
            receipts.join("mcdonalds/1840105-0786-20260405.html"),
            r#"<div>We got your order, here's your receipt.</div>
<div>Restaurant Number:</div><div>1840105</div>
<div>Order Id:</div><div>0786</div>
<div>Delivery Date:</div><div>05/04/26 17:06</div>
<table>
 <tr><th>Qty</th><th>Item Detail</th><th>Item Cost</th></tr>
 <tr><td>1</td><td>Double Cheeseburger Large Meal</td><td>AED 29.00</td></tr>
</table>"#,
        )
        .unwrap();
        fs::write(
            receipts.join("careem/CRM-NEW.html"),
            "Tax Invoice\nCareem Networks\nInvoice No: CRM-NEW\nInvoice Date: 26 Aug 2026\n1 x Falafel Wrap AED 22.00\nTotal AED 22.00\n",
        )
        .unwrap();

        let app = App::open(dir.path().join("oma")).unwrap();
        let report = app
            .import_bringfast(
                &source,
                &BringfastImport {
                    dry_run: false,
                    receipts_dir: Some(receipts),
                    store_accounts: false,
                },
            )
            .unwrap();
        assert_eq!(report.invoices_new, 2);
        assert_eq!(report.lines, 3);
        assert!(report.skipped_stores.is_empty());
        assert_eq!(report.files_scanned, 2);
        assert_eq!(report.files_existing, 1);
        assert_eq!(report.files_new, 1);
        assert_eq!(report.files_unparsed, 0);
        let invoices = app.list_invoices().unwrap();
        assert_eq!(invoices.len(), 3);
        assert!(invoices
            .iter()
            .any(|invoice| { invoice.store == StoreId::Careem && invoice.invoice_no == "CRM-DB" }));
        assert!(invoices.iter().any(|invoice| {
            invoice.store == StoreId::Mcdonalds && invoice.invoice_no == "1840105-0786-20260405"
        }));
        let added = invoices
            .iter()
            .find(|invoice| invoice.invoice_no == "CRM-NEW")
            .unwrap();
        assert_eq!(added.store, StoreId::Careem);
        assert_eq!(added.purchased_on, "2026-08-26");
        assert_eq!(added.lines.len(), 1);
        assert_eq!(added.lines[0].name, "Falafel Wrap");
    }

    #[test]
    fn image_index_keeps_bringfast_urls_by_name_and_key() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("bringfast.db");
        let conn = Connection::open(&source).unwrap();
        conn.execute_batch(
            "CREATE TABLE invoice_items (name TEXT, product_key TEXT, image_url TEXT);
             CREATE TABLE product_meta (official_name TEXT, product_key TEXT, image_url TEXT);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO invoice_items (name, product_key, image_url) VALUES ('Oat Milk', 'oat', '/product-images/oat.webp')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO product_meta (official_name, product_key, image_url) VALUES ('Oat Milk', 'oat', 'https://cdn.example/oat.png')",
            [],
        )
        .unwrap();
        drop(conn);
        let index = bringfast_image_index(&source);
        let urls = index.get("oat milk").unwrap();
        assert!(urls.iter().any(|url| url == "/product-images/oat.webp"));
        assert!(urls.iter().any(|url| url == "https://cdn.example/oat.png"));
        assert_eq!(index.get("oat").unwrap().len(), 2);
    }

    #[test]
    fn a_broken_receipt_file_is_skipped_and_the_next_file_is_imported() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("bringfast.db");
        let conn = Connection::open(&source).unwrap();
        conn.execute_batch(MEMORY_SCHEMA).unwrap();
        drop(conn);
        let receipts = dir.path().join("receipts");
        fs::create_dir_all(receipts.join("careem")).unwrap();
        fs::write(receipts.join("careem/not-a-pdf.pdf"), b"not a pdf").unwrap();
        fs::write(
            receipts.join("careem/CRM-OK.html"),
            "Tax Invoice\nCareem Networks\nInvoice No: CRM-OK\nInvoice Date: 26 Aug 2026\n2 × Falafel Wrap AED 22.00\nTotal AED 22.00\n",
        )
        .unwrap();
        let app = App::open(dir.path().join("oma")).unwrap();
        let report = app
            .import_bringfast(
                &source,
                &BringfastImport {
                    dry_run: false,
                    receipts_dir: Some(receipts),
                    store_accounts: false,
                },
            )
            .unwrap();
        assert_eq!(report.files_scanned, 2);
        assert_eq!(report.files_unparsed, 1);
        assert_eq!(report.files_new, 1);
        let invoices = app.list_invoices().unwrap();
        assert_eq!(invoices.len(), 1);
        assert_eq!(invoices[0].invoice_no, "CRM-OK");
        assert_eq!(invoices[0].purchased_on, "2026-08-26");
    }
}

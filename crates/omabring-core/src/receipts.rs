//! Read a Bring Fast receipts folder (`store/invoice.pdf` or `.html`).
//!
//! A file whose receipt number is already stored is skipped. The line readers
//! follow Bring Fast's invoice parser: supermarket PDFs, Careem mail, and
//! McDonald's order HTML.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use chrono::NaiveDate;
use rusqlite::{params, Connection, OptionalExtension};

use crate::bringfast::ImportReport;
use crate::error::{Error, Result};
use crate::model::{is_size_upcharge, junk_product_name, normalize_name, StoreId};
use crate::App;

struct ParsedLine {
    name: String,
    qty: f64,
    unit_price: f64,
    line_total: f64,
}

struct ParsedReceipt {
    store: StoreId,
    invoice_no: String,
    order_no: String,
    date: String,
    items: Vec<ParsedLine>,
}

pub(crate) fn import_receipt_files(
    dest: &Connection,
    dry_run: bool,
    dir: &Path,
    report: &mut ImportReport,
) -> Result<()> {
    report.receipt_files = true;
    if !dir.is_dir() {
        return Err(Error::Message(format!(
            "receipts folder is not a directory: {}",
            dir.display()
        )));
    }
    let mut files = Vec::new();
    collect_receipts(dir, &mut files)?;
    files.sort();
    for path in files {
        report.files_scanned += 1;
        let parsed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| read_receipt(&path)));
        let parsed = match parsed {
            Ok(parsed) => parsed,
            Err(_) => {
                eprintln!("skipped unreadable receipt {}", path.display());
                report.files_unparsed += 1;
                continue;
            }
        };
        match parsed {
            Some(parsed) if !parsed.invoice_no.is_empty() => {
                if let Some(invoice_id) = known_receipt(
                    dest,
                    parsed.store,
                    &parsed.invoice_no,
                    &parsed.order_no,
                    &path,
                )? {
                    report.files_existing += 1;
                    if !dry_run {
                        let _ = remember_receipt_file(dest, invoice_id, &path);
                        report.lines += add_missing_lines(dest, invoice_id, &parsed)?;
                    }
                    continue;
                }
                report.files_new += 1;
                report.lines += parsed.items.len();
                if dry_run {
                    continue;
                }
                if let Err(err) = insert_receipt(dest, &parsed, Some(&path)) {
                    eprintln!("skipped receipt {}: {err}", path.display());
                    report.files_new = report.files_new.saturating_sub(1);
                    report.lines = report.lines.saturating_sub(parsed.items.len());
                    report.files_unparsed += 1;
                }
            }
            _ => report.files_unparsed += 1,
        }
    }
    Ok(())
}

fn collect_receipts(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let entries = fs::read_dir(dir).map_err(|err| Error::Message(err.to_string()))?;
    for entry in entries {
        let entry = entry.map_err(|err| Error::Message(err.to_string()))?;
        let path = entry.path();
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name == "_preview" || name.starts_with('.'))
        {
            continue;
        }
        if path.is_dir() {
            collect_receipts(&path, out)?;
            continue;
        }
        let ext = path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if matches!(ext.as_str(), "pdf" | "html" | "htm") {
            out.push(path);
        }
    }
    Ok(())
}

fn known_receipt(
    dest: &Connection,
    store: StoreId,
    invoice_no: &str,
    order_no: &str,
    path: &Path,
) -> Result<Option<i64>> {
    let stem = path
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let mut keys = vec![invoice_no.to_string()];
    if !order_no.is_empty() {
        keys.push(order_no.to_string());
        keys.push(format!("order-{order_no}"));
    }
    if !stem.is_empty() {
        keys.push(stem.clone());
        keys.push(format!("order-{stem}"));
        if let Some(rest) = stem.strip_prefix("order-") {
            keys.push(rest.to_string());
        }
    }
    if let Some(rest) = invoice_no.strip_prefix("order-") {
        keys.push(rest.to_string());
    }
    for key in keys {
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        let found: Option<i64> = dest
            .query_row(
                "SELECT id FROM invoices WHERE store = ?1 AND invoice_no = ?2",
                params![store.as_str(), key],
                |row| row.get(0),
            )
            .ok();
        if let Some(id) = found {
            return Ok(Some(id));
        }
    }
    Ok(None)
}

/// A later import puts back lines that were removed, without duplicating the rest.
fn add_missing_lines(dest: &Connection, invoice_id: i64, parsed: &ParsedReceipt) -> Result<usize> {
    let mut added = 0usize;
    for line in &parsed.items {
        // An existing receipt keeps its stored lines. Only a size upcharge that
        // was dropped on an earlier import is put back.
        if junk_product_name(&line.name) || !is_size_upcharge(&line.name) {
            continue;
        }
        let key = normalize_name(&line.name);
        let exists: Option<i64> = dest
            .query_row(
                "SELECT id FROM invoice_lines WHERE invoice_id = ?1 AND name_key = ?2",
                params![invoice_id, key],
                |row| row.get(0),
            )
            .optional()?;
        if exists.is_some() {
            continue;
        }
        dest.execute(
            "INSERT INTO invoice_lines (invoice_id, name, name_key, qty, unit, unit_price, line_total, product_key) VALUES (?1, ?2, ?3, ?4, 'piece', ?5, ?6, ?7)",
            params![
                invoice_id,
                line.name,
                key,
                line.qty,
                line.unit_price,
                line.line_total,
                key
            ],
        )?;
        added += 1;
    }
    if added > 0 {
        let sum: f64 = dest.query_row(
            "SELECT coalesce(sum(line_total), 0) FROM invoice_lines WHERE invoice_id = ?1",
            params![invoice_id],
            |row| row.get(0),
        )?;
        let header: f64 = dest.query_row(
            "SELECT total FROM invoices WHERE id = ?1",
            params![invoice_id],
            |row| row.get(0),
        )?;
        if sum > header + 0.009 {
            dest.execute(
                "UPDATE invoices SET total = ?1 WHERE id = ?2",
                params![sum, invoice_id],
            )?;
        }
    }
    Ok(added)
}

fn insert_receipt(dest: &Connection, parsed: &ParsedReceipt, source: Option<&Path>) -> Result<()> {
    let items: Vec<&ParsedLine> = parsed
        .items
        .iter()
        .filter(|line| !junk_product_name(&line.name))
        .collect();
    let total: f64 = items.iter().map(|line| line.line_total).sum();
    let date = if parsed.date.is_empty() {
        "1970-01-01".to_string()
    } else {
        parsed.date.clone()
    };
    dest.execute(
        "INSERT INTO invoices (store, invoice_no, purchased_on, currency, total) VALUES (?1, ?2, ?3, 'AED', ?4)",
        params![parsed.store.as_str(), parsed.invoice_no, date, total],
    )?;
    let invoice_id = dest.last_insert_rowid();
    for line in items {
        dest.execute(
            "INSERT INTO invoice_lines (invoice_id, name, name_key, qty, unit, unit_price, line_total, product_key) VALUES (?1, ?2, ?3, ?4, 'piece', ?5, ?6, ?7)",
            params![
                invoice_id,
                line.name,
                normalize_name(&line.name),
                line.qty,
                line.unit_price,
                line.line_total,
                normalize_name(&line.name)
            ],
        )?;
    }
    if let Some(source) = source {
        let _ = remember_receipt_file(dest, invoice_id, source);
    }
    Ok(())
}

fn remember_receipt_file(dest: &Connection, invoice_id: i64, source: &Path) -> Result<()> {
    let stored: String = dest
        .query_row(
            "SELECT ifnull(source_file, '') FROM invoices WHERE id = ?1",
            params![invoice_id],
            |row| row.get(0),
        )
        .unwrap_or_default();
    if !stored.trim().is_empty() && Path::new(stored.trim()).is_file() {
        return Ok(());
    }
    let path =
        archive_receipt_file(dest, invoice_id, source).unwrap_or_else(|_| source.to_path_buf());
    dest.execute(
        "UPDATE invoices SET source_file = ?1 WHERE id = ?2",
        params![path.display().to_string(), invoice_id],
    )?;
    Ok(())
}

fn archive_receipt_file(dest: &Connection, invoice_id: i64, source: &Path) -> Result<PathBuf> {
    let store: String = dest.query_row(
        "SELECT store FROM invoices WHERE id = ?1",
        params![invoice_id],
        |row| row.get(0),
    )?;
    let db = dest
        .path()
        .map(PathBuf::from)
        .ok_or_else(|| Error::Message("database path is missing".into()))?;
    let root = db
        .parent()
        .ok_or_else(|| Error::Message("database directory is missing".into()))?
        .join("receipts")
        .join(store);
    fs::create_dir_all(&root).map_err(|err| Error::Message(err.to_string()))?;
    let name = source
        .file_name()
        .ok_or_else(|| Error::Message("receipt file has no name".into()))?;
    let dest_path = root.join(name);
    if source != dest_path && !same_file(source, &dest_path) {
        if fs::hard_link(source, &dest_path).is_err() {
            fs::copy(source, &dest_path).map_err(|err| Error::Message(err.to_string()))?;
        }
    }
    Ok(dest_path)
}

fn same_file(left: &Path, right: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Ok(left) = fs::metadata(left) else {
        return false;
    };
    let Ok(right) = fs::metadata(right) else {
        return false;
    };
    left.dev() == right.dev() && left.ino() == right.ino()
}

/// Bring Fast keeps the scan at `receipts/<store>/<invoice>.pdf` (`purchases.py`
/// `receipt_dir` / `receipt_path`) and stores that path on `invoices.source_file`.
/// An upgrade copies or links the file into this app's data directory and
/// fills `source_file` for invoices that were imported without it.
pub(crate) fn attach_missing_receipts(conn: &Connection, data_dir: &Path) -> Result<usize> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS receipt_file_seen (path TEXT PRIMARY KEY)")?;
    let pending = invoices_missing_file(conn)?;
    if pending.is_empty() {
        return Ok(0);
    }
    let roots = receipt_roots(conn, data_dir);
    let databases = bringfast_databases(conn);
    let mut index: std::collections::HashMap<String, Vec<FileHit>> =
        std::collections::HashMap::new();
    for root in &roots {
        index_receipt_tree(root, 0, &mut index);
    }
    for database in &databases {
        index_bringfast_db(database, &mut index);
    }
    let mut linked = 0usize;
    let mut still = Vec::new();
    for invoice in &pending {
        if let Some(path) = best_hit(&index, &invoice.store, &invoice.invoice_no, data_dir) {
            if remember_receipt_file(conn, invoice.id, &path).is_ok() {
                linked += 1;
                continue;
            }
        }
        still.push(invoice.clone());
    }
    if still.is_empty() {
        return Ok(linked);
    }
    linked += link_by_reading(conn, &roots)?;
    Ok(linked)
}

/// Point one invoice at a file that already exists. The import uses this when
/// Bring Fast's `source_file` column names the original PDF or image.
pub(crate) fn bind_receipt_file(
    dest: &Connection,
    invoice_id: i64,
    candidates: &[PathBuf],
) -> Result<bool> {
    for path in candidates {
        if path.is_file() {
            remember_receipt_file(dest, invoice_id, path)?;
            return Ok(true);
        }
    }
    Ok(false)
}

pub(crate) fn remember_import_locations(
    dest: &Connection,
    source_db: &Path,
    receipts_dir: Option<&Path>,
) -> Result<()> {
    save_setting(dest, "bringfast_source", &source_db.display().to_string())?;
    if let Some(dir) = receipts_dir {
        save_setting(dest, "receipts_dir", &dir.display().to_string())?;
    }
    Ok(())
}

#[derive(Clone)]
struct PendingInvoice {
    id: i64,
    store: String,
    invoice_no: String,
}

struct FileHit {
    path: PathBuf,
    store: String,
}

fn invoices_missing_file(conn: &Connection) -> Result<Vec<PendingInvoice>> {
    if !crate::table_exists(conn, "invoices")? {
        return Ok(Vec::new());
    }
    let mut stmt = conn.prepare(
        "SELECT id, store, invoice_no, ifnull(source_file, '') FROM invoices ORDER BY id",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    let mut pending = Vec::new();
    for row in rows {
        let (id, store, invoice_no, stored) = row?;
        let stored = stored.trim();
        if !stored.is_empty() && Path::new(stored).is_file() {
            continue;
        }
        pending.push(PendingInvoice {
            id,
            store,
            invoice_no,
        });
    }
    Ok(pending)
}

fn invoice_needs_file(conn: &Connection, invoice_id: i64) -> Result<bool> {
    let stored: String = conn.query_row(
        "SELECT ifnull(source_file, '') FROM invoices WHERE id = ?1",
        params![invoice_id],
        |row| row.get(0),
    )?;
    Ok(stored.trim().is_empty() || !Path::new(stored.trim()).is_file())
}

fn save_setting(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO app_settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

fn load_setting(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row(
        "SELECT value FROM app_settings WHERE key = ?1",
        params![key],
        |row| row.get(0),
    )
    .ok()
}

fn receipt_roots(conn: &Connection, data_dir: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let mut push = |path: PathBuf| {
        if path.is_dir() && !roots.iter().any(|have| have == &path) {
            roots.push(path);
        }
    };
    push(data_dir.join("receipts"));
    if let Some(dir) = load_setting(conn, "receipts_dir") {
        push(PathBuf::from(dir));
    }
    if let Ok(data) = std::env::var("BRINGFAST_DATA") {
        let data = data.trim();
        if !data.is_empty() {
            push(PathBuf::from(data).join("receipts"));
        }
    }
    if let Some(source) = load_setting(conn, "bringfast_source") {
        if let Some(parent) = Path::new(&source).parent() {
            push(parent.join("receipts"));
        }
    }
    push(crate::home_dir().join(".bring-fast/receipts"));
    roots
}

fn bringfast_databases(conn: &Connection) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut push = |path: PathBuf| {
        if path.is_file() && !found.iter().any(|have| have == &path) {
            found.push(path);
        }
    };
    if let Some(source) = load_setting(conn, "bringfast_source") {
        push(PathBuf::from(source));
    }
    if let Ok(data) = std::env::var("BRINGFAST_DATA") {
        let data = data.trim();
        if !data.is_empty() {
            push(PathBuf::from(data).join("bringfast.db"));
        }
    }
    push(crate::home_dir().join(".bring-fast/bringfast.db"));
    found
}

fn index_receipt_tree(
    dir: &Path,
    depth: usize,
    index: &mut std::collections::HashMap<String, Vec<FileHit>>,
) {
    if depth > 3 || !dir.is_dir() {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || name == "_preview" {
            continue;
        }
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            index_receipt_tree(&path, depth + 1, index);
            continue;
        }
        if !kind.is_file() || !is_receipt_ext(&path) {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        let store = path
            .parent()
            .and_then(|parent| parent.file_name())
            .and_then(|name| name.to_str())
            .filter(|_| depth > 0)
            .map(canonical_store)
            .unwrap_or_default();
        add_hit(index, &store, stem, &path);
    }
}

fn index_bringfast_db(path: &Path, index: &mut std::collections::HashMap<String, Vec<FileHit>>) {
    let Ok(source) = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
    else {
        return;
    };
    if !crate::table_exists(&source, "invoices").unwrap_or(false) {
        return;
    }
    let has_file = crate::column_exists(&source, "invoices", "source_file").unwrap_or(false);
    let has_order = crate::column_exists(&source, "invoices", "order_no").unwrap_or(false);
    let sql = format!(
        "SELECT retailer, invoice_no, {order}, {file} FROM invoices",
        order = if has_order {
            "ifnull(order_no, '')"
        } else {
            "''"
        },
        file = if has_file {
            "ifnull(source_file, '')"
        } else {
            "''"
        },
    );
    let Ok(mut stmt) = source.prepare(&sql) else {
        return;
    };
    let Ok(rows) = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
        ))
    }) else {
        return;
    };
    let parent = path.parent().unwrap_or(path);
    for row in rows.flatten() {
        let (retailer, invoice_no, order_no, source_file) = row;
        let store = canonical_store(&retailer);
        if let Some(file) = resolve_stored_file(parent, &source_file) {
            add_hit(index, &store, &invoice_no, &file);
            add_hit(index, &store, &order_no, &file);
        }
        if let Some(found) = best_hit(index, &store, &invoice_no, Path::new("")) {
            add_hit(index, &store, &order_no, &found);
        } else if let Some(found) = best_hit(index, &store, &order_no, Path::new("")) {
            add_hit(index, &store, &invoice_no, &found);
        }
    }
}

fn resolve_stored_file(base: &Path, stored: &str) -> Option<PathBuf> {
    let stored = stored.trim();
    if stored.is_empty() {
        return None;
    }
    let direct = PathBuf::from(stored);
    if direct.is_file() {
        return Some(direct);
    }
    let name = Path::new(stored)
        .file_name()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(stored));
    for candidate in [
        base.join(stored),
        base.join("receipts").join(stored),
        base.join("receipts").join(&name),
    ] {
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn link_by_reading(conn: &Connection, roots: &[PathBuf]) -> Result<usize> {
    let mut linked = 0usize;
    for root in roots {
        let mut files = Vec::new();
        collect_receipts(root, &mut files)?;
        for path in files {
            if file_was_seen(conn, &path)? {
                continue;
            }
            if let Some(parsed) = read_receipt(&path) {
                if let Some(invoice_id) = known_receipt(
                    conn,
                    parsed.store,
                    &parsed.invoice_no,
                    &parsed.order_no,
                    &path,
                )? {
                    if invoice_needs_file(conn, invoice_id)?
                        && remember_receipt_file(conn, invoice_id, &path).is_ok()
                    {
                        linked += 1;
                    }
                }
            }
            mark_seen(conn, &path)?;
        }
    }
    Ok(linked)
}

fn file_was_seen(conn: &Connection, path: &Path) -> Result<bool> {
    let found: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM receipt_file_seen WHERE path = ?1",
            params![path.display().to_string()],
            |row| row.get(0),
        )
        .optional()?;
    Ok(found.is_some())
}

fn mark_seen(conn: &Connection, path: &Path) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO receipt_file_seen (path) VALUES (?1)",
        params![path.display().to_string()],
    )?;
    Ok(())
}

fn add_hit(
    index: &mut std::collections::HashMap<String, Vec<FileHit>>,
    store: &str,
    raw: &str,
    path: &Path,
) {
    if raw.trim().is_empty() || !path.is_file() {
        return;
    }
    for key in receipt_name_keys(raw) {
        let hits = index.entry(key).or_default();
        if hits
            .iter()
            .any(|hit| hit.path == path && hit.store == store)
        {
            continue;
        }
        hits.push(FileHit {
            path: path.to_path_buf(),
            store: store.to_string(),
        });
    }
}

fn best_hit(
    index: &std::collections::HashMap<String, Vec<FileHit>>,
    store: &str,
    invoice_no: &str,
    data_dir: &Path,
) -> Option<PathBuf> {
    let store = canonical_store(store);
    let mut found: Vec<&FileHit> = Vec::new();
    for key in receipt_name_keys(invoice_no) {
        if let Some(hits) = index.get(&key) {
            for hit in hits {
                if hit.store != store && !hit.store.is_empty() {
                    continue;
                }
                if !found.iter().any(|have| have.path == hit.path) {
                    found.push(hit);
                }
            }
        }
    }
    found
        .into_iter()
        .min_by_key(|hit| {
            let owned = data_dir.as_os_str().is_empty() || hit.path.starts_with(data_dir);
            if owned {
                0
            } else {
                1
            }
        })
        .map(|hit| hit.path.clone())
}

fn receipt_name_keys(raw: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let mut push = |value: &str| {
        let value = value.trim();
        if value.is_empty() || keys.iter().any(|have: &String| have == value) {
            return;
        }
        keys.push(value.to_string());
        let lower = value.to_lowercase();
        if !keys.iter().any(|have| have == &lower) {
            keys.push(lower);
        }
    };
    push(raw);
    if let Some((head, _)) = raw.split_once('#') {
        push(head);
    }
    let safe: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        .collect();
    push(&safe);
    let trimmed = raw.trim();
    if let Some(rest) = trimmed.strip_prefix("order-") {
        push(rest);
    } else if !trimmed.is_empty() {
        push(&format!("order-{trimmed}"));
    }
    if let Some(rest) = safe.strip_prefix("order-") {
        push(rest);
    }
    keys
}

fn canonical_store(raw: &str) -> String {
    if let Some(id) = StoreId::parse(raw) {
        return id.as_str().to_string();
    }
    let folded: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect();
    for id in StoreId::all() {
        let id_fold: String = id
            .as_str()
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect();
        if id_fold == folded {
            return id.as_str().to_string();
        }
    }
    folded
}

fn is_receipt_ext(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "pdf" | "html" | "htm" | "png" | "jpg" | "jpeg" | "webp" | "gif"
    )
}

/// Where Bring Fast keeps a receipt scan: the stored path, then
/// `receipts/<store>/<invoice>.pdf` (and the `order-` name) under this
/// app, `BRINGFAST_DATA`, and `~/.bring-fast`.
pub fn locate_receipt_file(
    data_dir: &Path,
    store: &str,
    invoice_no: &str,
    stored: &str,
) -> Option<PathBuf> {
    let stored = stored.trim();
    if !stored.is_empty() {
        let path = PathBuf::from(stored);
        if path.is_file() {
            return Some(path);
        }
    }
    let mut roots = vec![data_dir.join("receipts")];
    if let Ok(extra) = std::env::var("BRINGFAST_DATA") {
        let extra = extra.trim();
        if !extra.is_empty() {
            roots.push(PathBuf::from(extra).join("receipts"));
        }
    }
    roots.push(crate::home_dir().join(".bring-fast/receipts"));
    let folded: String = store
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect();
    let mut folders = vec![store.to_string()];
    if !folded.is_empty() && folded != store {
        folders.push(folded);
    }
    let mut names = Vec::new();
    let mut push_name = |raw: &str| {
        let raw = raw.trim();
        if raw.is_empty() || names.iter().any(|have: &String| have == raw) {
            return;
        }
        names.push(raw.to_string());
        let safe: String = raw
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
            .collect();
        if !safe.is_empty() && !names.iter().any(|have| have == &safe) {
            names.push(safe);
        }
    };
    push_name(invoice_no);
    if let Some(rest) = invoice_no.strip_prefix("order-") {
        push_name(rest);
    } else {
        push_name(&format!("order-{invoice_no}"));
    }
    const EXTS: &[&str] = &["pdf", "html", "htm", "png", "jpg", "jpeg", "webp", "gif"];
    for root in roots {
        for folder in &folders {
            for name in &names {
                for ext in EXTS {
                    let path = root.join(folder).join(format!("{name}.{ext}"));
                    if path.is_file() {
                        return Some(path);
                    }
                }
            }
        }
    }
    None
}

/// Page images for a receipt file. A picture is itself. A PDF is rendered
/// with the same `pdftoppm` pass Bring Fast uses. HTML has no native page.
pub fn receipt_page_images(path: &Path) -> Vec<PathBuf> {
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "webp" | "gif") {
        return vec![path.to_path_buf()];
    }
    if ext != "pdf" {
        return Vec::new();
    }
    let Some(parent) = path.parent() else {
        return Vec::new();
    };
    let dest = parent.join("_preview");
    let stem = path
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("receipt");
    let _ = fs::create_dir_all(&dest);
    let mut found = preview_pngs(&dest, stem);
    if !found.is_empty() {
        return found;
    }
    let prefix = dest.join(stem);
    for bin in ["pdftoppm", "pdftocairo"] {
        let ok = Command::new(bin)
            .args(["-png", "-r", "110"])
            .arg(path)
            .arg(&prefix)
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false);
        if ok {
            found = preview_pngs(&dest, stem);
            if !found.is_empty() {
                return found;
            }
        }
    }
    Vec::new()
}

fn preview_pngs(dir: &Path, stem: &str) -> Vec<PathBuf> {
    let mut pages = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return pages;
    };
    let prefix = format!("{stem}-");
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name.starts_with(&prefix) && name.to_ascii_lowercase().ends_with(".png") {
            pages.push(path);
        }
    }
    pages.sort();
    pages
}

impl App {
    /// The original receipt file for this invoice, when one was imported.
    pub fn receipt_attachment(&self, invoice_id: i64) -> Result<Option<PathBuf>> {
        let row = self.with_conn(|conn| {
            conn.query_row(
                "SELECT store, invoice_no, ifnull(source_file, '') FROM invoices WHERE id = ?1",
                params![invoice_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(Error::from)
        })?;
        let Some((store, invoice_no, stored)) = row else {
            return Ok(None);
        };
        Ok(locate_receipt_file(
            self.data_dir(),
            &store,
            &invoice_no,
            &stored,
        ))
    }
}

fn read_receipt(path: &Path) -> Option<ParsedReceipt> {
    let folder = store_hint(path);
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ext == "pdf" {
        let text = pdf_text(path)?;
        return parse_text(&text, folder, path);
    }
    let raw = fs::read_to_string(path).ok()?;
    if let Some(parsed) = parse_html(&raw, folder) {
        return Some(parsed);
    }
    parse_text(&html_to_text(&raw), folder, path)
}

fn pdf_text(path: &Path) -> Option<String> {
    let output = Command::new("pdftotext")
        .arg("-layout")
        .arg("-q")
        .arg(path)
        .arg("-")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    if text.trim().is_empty() {
        None
    } else {
        Some(text)
    }
}

fn store_hint(path: &Path) -> Option<StoreId> {
    let folder = path.parent().and_then(|parent| {
        parent
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(StoreId::parse)
    });
    if folder.is_some() {
        return folder;
    }
    path.file_stem()
        .and_then(|name| name.to_str())
        .and_then(StoreId::parse)
}

fn parse_html(html: &str, folder: Option<StoreId>) -> Option<ParsedReceipt> {
    if html.contains("product-name") && html.contains("item-qty") {
        return parse_grandiose_html(html);
    }
    let text = html_to_text(html);
    let low = text.to_lowercase();
    let store = folder.or_else(|| sniff_store(&low))?;
    match store {
        StoreId::Careem => parse_careem(&text),
        StoreId::Mcdonalds => parse_mcdonalds(&text),
        StoreId::AfricanEastern => parse_african(&text),
        StoreId::Carrefour => parse_carrefour(&text),
        StoreId::Grandiose => parse_grandiose(&text),
        StoreId::Mmi => parse_mmi(&text),
        _ => None,
    }
}

fn parse_text(text: &str, folder: Option<StoreId>, path: &Path) -> Option<ParsedReceipt> {
    let low = text.to_lowercase();
    let store = sniff_store(&low).or(folder)?;
    let mut parsed = match store {
        StoreId::Carrefour => parse_carrefour(text),
        StoreId::Grandiose => parse_grandiose(text),
        StoreId::Mmi => parse_mmi(text),
        StoreId::AfricanEastern => parse_african(text),
        StoreId::Careem => parse_careem(text),
        StoreId::Mcdonalds => parse_mcdonalds(text),
        StoreId::AmazonIt | StoreId::AmazonAe => parse_amazon(store, text),
        StoreId::UnionCoop | StoreId::Waitrose | StoreId::Spinneys => None,
    };
    if parsed.is_none() {
        if let Some(store) = folder {
            let stem = path.file_stem()?.to_str()?.trim();
            if !stem.is_empty() && stem != "receipt" {
                parsed = Some(ParsedReceipt {
                    store,
                    invoice_no: stem.to_string(),
                    order_no: String::new(),
                    date: String::new(),
                    items: Vec::new(),
                });
            }
        }
    }
    parsed
}

fn sniff_store(low: &str) -> Option<StoreId> {
    if low.contains("grandiose") {
        return Some(StoreId::Grandiose);
    }
    if low.contains("mmi home delivery") || low.contains("mmihomedelivery") {
        return Some(StoreId::Mmi);
    }
    if low.contains("african") && low.contains("eastern") {
        return Some(StoreId::AfricanEastern);
    }
    if low.contains("careem") {
        return Some(StoreId::Careem);
    }
    if low.contains("mcdonald") || low.contains("we got your order") {
        return Some(StoreId::Mcdonalds);
    }
    if low.contains("amazon.it") {
        return Some(StoreId::AmazonIt);
    }
    if low.contains("amazon.ae") || low.contains("amazon ae") {
        return Some(StoreId::AmazonAe);
    }
    if low.contains("majid al futtaim") || low.contains("carrefour") || low.contains("tax invoice")
    {
        return Some(StoreId::Carrefour);
    }
    None
}

fn parse_carrefour(text: &str) -> Option<ParsedReceipt> {
    let invoice_no = after_label(text, &["Invoice No.", "Invoice No", "Invoice Number"]);
    let date = parse_cf_date(&after_label(text, &["Invoice Date"]));
    let order_no = after_label(text, &["Order No.", "Order No"]);
    let mut items = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(item) = carrefour_line(line) {
            if !skip_name(&item.name) {
                items.push(item);
            }
            continue;
        }
        let _ = after_label(line, &["Barcode"]);
    }
    if invoice_no.is_empty() && items.is_empty() {
        return None;
    }
    Some(ParsedReceipt {
        store: StoreId::Carrefour,
        invoice_no,
        order_no,
        date,
        items,
    })
}

fn carrefour_line(line: &str) -> Option<ParsedLine> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 8 {
        return None;
    }
    let mut vals = Vec::new();
    let mut tail = 0usize;
    for tok in parts.iter().rev() {
        if let Some(number) = parse_number(tok) {
            vals.push(number);
            tail += 1;
        } else {
            break;
        }
    }
    vals.reverse();
    if vals.len() < 7 {
        return None;
    }
    let name = parts[..parts.len() - tail].join(" ");
    if name.is_empty() || !name.chars().any(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    cf_from_numbers(&name, &vals)
}

fn cf_from_numbers(name: &str, nums: &[f64]) -> Option<ParsedLine> {
    if nums.len() >= 9 {
        let ordered = nums[nums.len() - 9];
        let delivered = nums[nums.len() - 8];
        let unit = nums[nums.len() - 7];
        let vatp = nums[nums.len() - 4];
        let total = nums[nums.len() - 1];
        if (0.0..5.06).contains(&vatp) && close(delivered * unit, total) {
            return Some(priced(name, delivered, unit, total));
        }
        if (0.0..5.06).contains(&vatp) && close(ordered * unit, total) {
            return Some(priced(name, ordered, unit, total));
        }
    }
    if nums.len() >= 7 {
        let qty = nums[nums.len() - 7];
        let unit = nums[nums.len() - 6];
        let vatp = nums[nums.len() - 3];
        let total = nums[nums.len() - 1];
        if (0.0..5.06).contains(&vatp) && close(qty * unit, total) {
            return Some(priced(name, qty, unit, total));
        }
    }
    None
}

fn parse_grandiose(text: &str) -> Option<ParsedReceipt> {
    if text.contains("Slip:") && text.lines().any(|line| slip_line(line.trim()).is_some()) {
        return parse_grandiose_slip(text);
    }
    let invoice_no = after_label(text, &["Tax Invoice #", "Tax Invoice#"]);
    let date_raw = after_label(text, &["Tax invoice Date:", "Tax invoice Date"]);
    let order_no = after_label(text, &["Order #", "Order#"]);
    let mut items = Vec::new();
    let mut buf = String::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("Products ") || line.starts_with("Net Value") {
            continue;
        }
        let candidate = if buf.is_empty() {
            line.to_string()
        } else {
            format!("{buf} {line}")
        };
        if let Some(item) = grandiose_line(&candidate) {
            if !skip_name(&item.name) {
                items.push(item);
            }
            buf.clear();
            continue;
        }
        if line.chars().any(|c| c.is_ascii_digit()) || !buf.is_empty() {
            buf = candidate;
        } else {
            buf = line.to_string();
        }
    }
    if invoice_no.is_empty() && items.is_empty() {
        return None;
    }
    Some(ParsedReceipt {
        store: StoreId::Grandiose,
        invoice_no,
        order_no,
        date: parse_long_date(&date_raw),
        items,
    })
}

fn parse_grandiose_slip(text: &str) -> Option<ParsedReceipt> {
    let invoice_no = after_label(text, &["Slip:"]);
    let date = parse_dmy(&after_label(text, &["Date:"]));
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let mut items = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        if let Some((sku, qty, price, amount)) = slip_line(lines[index]) {
            let mut name = sku.clone();
            if index + 1 < lines.len() {
                let next = lines[index + 1];
                if slip_line(next).is_none()
                    && !next.starts_with("----")
                    && !next.starts_with("Total")
                {
                    name = next.to_string();
                    index += 1;
                }
            }
            if !skip_name(&name) {
                items.push(priced(&name, qty, price, amount));
            }
        }
        index += 1;
    }
    if invoice_no.is_empty() && items.is_empty() {
        return None;
    }
    Some(ParsedReceipt {
        store: StoreId::Grandiose,
        invoice_no,
        order_no: String::new(),
        date,
        items,
    })
}

fn slip_line(line: &str) -> Option<(String, f64, f64, f64)> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() != 4 {
        return None;
    }
    if parts[0].len() < 6 || !parts[0].chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some((
        parts[0].to_string(),
        parse_number(parts[1])?,
        parse_number(parts[2])?,
        parse_number(parts[3])?,
    ))
}

fn grandiose_line(line: &str) -> Option<ParsedLine> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    let sku_at = parts.iter().position(|tok| {
        let digits = tok.chars().all(|c| c.is_ascii_digit());
        digits && (8..=14).contains(&tok.len())
    })?;
    if parts.len() < sku_at + 8 {
        return None;
    }
    let name = parts[..sku_at].join(" ");
    let qty = parse_number(parts[sku_at + 1])?;
    let gross = parse_number(parts[sku_at + 7])?;
    if name.is_empty() || qty <= 0.0 {
        return None;
    }
    Some(priced(&name, qty, gross / qty, gross))
}

fn parse_grandiose_html(html: &str) -> Option<ParsedReceipt> {
    let mut order = between(html, "no-link", "#");
    if order.is_empty() {
        order = after_label(html, &["Order #", "Order#"]);
    }
    let order = digits_run(&order);
    let mut items = Vec::new();
    let mut rest = html;
    while let Some(start) = rest.find("product-name") {
        let slice = &rest[start..];
        let name = between_tags(slice, '>');
        let _sku = after_label(slice, &["SKU:"]);
        let qty = after_label(slice, &["item-qty"]);
        let price = after_label(slice, &["AED"]);
        let qty_n = parse_number(qty.trim()).unwrap_or(0.0);
        let total = parse_number(&first_number(&price)).unwrap_or(0.0);
        if !name.is_empty() && qty_n > 0.0 && total > 0.0 && !skip_name(&name) {
            items.push(priced(&name, qty_n, total / qty_n, total));
        }
        rest = &rest[start + "product-name".len()..];
        if !slice.contains("item-qty") {
            break;
        }
    }
    if order.is_empty() && items.is_empty() {
        return None;
    }
    Some(ParsedReceipt {
        store: StoreId::Grandiose,
        invoice_no: if order.is_empty() {
            String::new()
        } else {
            format!("order-{order}")
        },
        order_no: order,
        date: String::new(),
        items,
    })
}

fn parse_mmi(text: &str) -> Option<ParsedReceipt> {
    let invoice_no = after_label(text, &["Invoice No:"]);
    let date = parse_cf_date(&after_label(text, &["Invoice Date:"]));
    let order_no = after_label(text, &["Customer Ref:"]);
    let mut items = Vec::new();
    for raw in text.lines() {
        if let Some(item) = mmi_line(raw.trim()) {
            if !skip_name(&item.name) {
                items.push(item);
            }
        }
    }
    if invoice_no.is_empty() && items.is_empty() {
        return None;
    }
    Some(ParsedReceipt {
        store: StoreId::Mmi,
        invoice_no,
        order_no,
        date,
        items,
    })
}

fn mmi_line(line: &str) -> Option<ParsedLine> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    let uom = parts.iter().position(|tok| {
        let up = tok.to_ascii_uppercase();
        up == "EACH" || up == "BTL" || up == "BOTTLE" || up == "CASE" || up.starts_with("CS")
    })?;
    if uom < 2 {
        return None;
    }
    let qty = parse_number(parts[uom - 1])?;
    let code = parts[0];
    if code.len() < 3 {
        return None;
    }
    let name = parts[1..uom - 1].join(" ");
    let total = parts.iter().rev().find_map(|tok| parse_number(tok))?;
    if total < 1.0 || name.is_empty() {
        return None;
    }
    Some(priced(&name, qty, total / qty.max(0.0001), total))
}

fn parse_african(text: &str) -> Option<ParsedReceipt> {
    let invoice_no = after_label(text, &["Your Invoice #", "Invoice #"]);
    let order_no = after_label(text, &["Order #"]);
    let date = parse_long_date(&after_label(text, &["Delivery Date:"]));
    let mut items = Vec::new();
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    for (index, line) in lines.iter().enumerate() {
        let sku = line
            .strip_prefix("SKU:")
            .or_else(|| line.strip_prefix("SKU :"))
            .map(str::trim)
            .filter(|sku| !sku.is_empty())
            .map(|sku| sku.split_whitespace().next().unwrap_or(sku).to_string());
        let Some(sku) = sku else { continue };
        let name = lines[..index]
            .iter()
            .rev()
            .find(|line| line.chars().any(|c| c.is_ascii_alphabetic()) && !line.contains("SKU"))
            .unwrap_or(&"")
            .to_string();
        let numbers: Vec<f64> = lines[index + 1..]
            .iter()
            .filter_map(|line| parse_number(line.split_whitespace().next().unwrap_or(line)))
            .take(8)
            .collect();
        if numbers.len() < 7 {
            continue;
        }
        let qty = numbers[0];
        let total = numbers[6];
        if qty > 0.0 && total > 0.0 && !skip_name(&name) {
            items.push(priced(&name, qty, total / qty, total));
            let _ = sku;
        }
    }
    let invoice_no = if invoice_no.is_empty() && !order_no.is_empty() {
        format!("order-{order_no}")
    } else {
        invoice_no
    };
    if invoice_no.is_empty() && items.is_empty() {
        return None;
    }
    Some(ParsedReceipt {
        store: StoreId::AfricanEastern,
        invoice_no,
        order_no,
        date,
        items,
    })
}

fn parse_careem(text: &str) -> Option<ParsedReceipt> {
    let order_no = field_value(text, &["Order ID", "Order No", "Order Number", "Order #"]);
    let mut invoice_no = field_value(text, &["Invoice No", "Invoice Number", "Tax Invoice No"]);
    let date_raw = field_rest(
        text,
        &["Invoice Date", "Order Date", "Delivery Date", "Date"],
    );
    let mut _place = field_value(
        text,
        &[
            "Your order from",
            "Order from",
            "Delivered from",
            "Restaurant",
        ],
    );
    let lines = clean_lines(text);
    for (index, line) in lines.iter().enumerate() {
        if line.eq_ignore_ascii_case("delivery details") {
            if let Some(next) = lines.get(index + 1) {
                _place = next.clone();
            }
        }
    }
    let mut date = parse_any_date(&date_raw);
    if date.is_empty() {
        date = careem_when(&lines);
    }
    let mut items = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let line = &lines[index];
        if careem_summary(line) || money_only(line).is_some() {
            index += 1;
            continue;
        }
        if let Some(name) = line.strip_prefix('+') {
            let name = name.trim();
            let (name, name_at) = if name.is_empty() {
                (lines.get(index + 1).cloned().unwrap_or_default(), index + 1)
            } else {
                (name.to_string(), index)
            };
            if let Some((price, next)) = following_price(&lines, name_at) {
                if let Some(item) = careem_item(&name, 1.0, price) {
                    items.push(item);
                    index = next;
                    continue;
                }
            }
            index += 1;
            continue;
        }
        if let Some(item) = careem_inline(line) {
            items.push(item);
            index += 1;
            continue;
        }
        if let Some((qty, name)) = careem_qty_only(line) {
            if let Some((price, next)) = following_price(&lines, index) {
                if let Some(item) = careem_item(&name, qty, price) {
                    items.push(item);
                    index = next;
                    continue;
                }
            }
        }
        index += 1;
    }
    if invoice_no.is_empty() && !order_no.is_empty() {
        invoice_no = format!("order-{order_no}");
    }
    if invoice_no.is_empty() && items.is_empty() {
        return None;
    }
    Some(ParsedReceipt {
        store: StoreId::Careem,
        invoice_no,
        order_no,
        date,
        items,
    })
}

fn careem_inline(line: &str) -> Option<ParsedLine> {
    let compact = line.replace('×', "x");
    let lower = compact.to_ascii_lowercase();
    if careem_summary(&compact) {
        return None;
    }
    if let Some(qty_end) = compact.find(" x ") {
        let qty = parse_number(compact[..qty_end].trim())?;
        let rest = compact[qty_end + 3..].trim();
        let (name, total) = split_trailing_money(rest)?;
        return careem_item(&name, qty, total);
    }
    if let Some(pos) = lower.rfind(" x ") {
        let name = compact[..pos].trim();
        let rest = compact[pos + 3..].trim();
        let mut parts = rest.split_whitespace();
        let qty = parse_number(parts.next()?)?;
        let money = parts.collect::<Vec<_>>().join(" ");
        let total = trailing_money(&money).or_else(|| parse_number(money.trim()))?;
        return careem_item(name, qty, total);
    }
    if compact.to_lowercase().contains("aed") || compact.contains('.') {
        let (name, total) = split_trailing_money(&compact)?;
        if name.chars().filter(|c| c.is_ascii_alphabetic()).count() >= 2 {
            return careem_item(&name, 1.0, total);
        }
    }
    None
}

fn careem_qty_only(line: &str) -> Option<(f64, String)> {
    let compact = line.replace('×', "x");
    if let Some(qty_end) = compact.find(" x ") {
        let qty = parse_number(compact[..qty_end].trim())?;
        let name = compact[qty_end + 3..].trim().to_string();
        if name.chars().any(|c| c.is_ascii_alphabetic()) {
            return Some((qty, name));
        }
    }
    if let Some(pos) = compact.to_ascii_lowercase().rfind(" x ") {
        let name = compact[..pos].trim().to_string();
        let qty = parse_number(compact[pos + 3..].trim())?;
        if name.chars().any(|c| c.is_ascii_alphabetic()) {
            return Some((qty, name));
        }
    }
    None
}

fn careem_item(name: &str, qty: f64, total: f64) -> Option<ParsedLine> {
    let mut clean = name.split_whitespace().collect::<Vec<_>>().join(" ");
    clean = clean
        .trim_matches(|c: char| " -–—·|".contains(c))
        .to_string();
    if clean.to_lowercase().starts_with("aed ") {
        clean = clean[4..].trim().to_string();
    }
    if careem_summary(&clean) || skip_name(&clean) || total <= 0.0 {
        return None;
    }
    if clean.chars().filter(|c| c.is_ascii_alphabetic()).count() < 2 || clean.len() > 120 {
        return None;
    }
    let qty = if qty > 0.0 { qty } else { 1.0 };
    Some(priced(
        &clean,
        qty,
        (total / qty * 100.0).round() / 100.0,
        total,
    ))
}

fn careem_summary(name: &str) -> bool {
    let n = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { ' ' })
        .collect::<String>();
    let n = n.split_whitespace().collect::<Vec<_>>().join(" ");
    const PHRASES: &[&str] = &[
        "subtotal",
        "sub total",
        "grand total",
        "order total",
        "items total",
        "discount",
        "promo",
        "promotion",
        "voucher",
        "coupon",
        "delivery",
        "service",
        "packaging",
        "small order",
        "careem plus",
        "careem pay",
        "rounding",
        "amount paid",
        "amount due",
        "you saved",
        "surcharge",
        "your total bill",
        "original cart",
        "original basket",
        "cart total",
        "basket total",
        "restaurant discount",
        "reward your captain",
        "payment method",
        "no delivery fee",
        "promo code",
        "incl tax",
    ];
    if n.is_empty()
        || n == "total"
        || n == "vat"
        || n == "tax"
        || n == "tip"
        || n == "tips"
        || n == "paid"
    {
        return true;
    }
    PHRASES
        .iter()
        .any(|phrase| n == *phrase || n.starts_with(&format!("{phrase} ")) || n.contains(phrase))
}

fn following_price(lines: &[String], index: usize) -> Option<(f64, usize)> {
    let mut cursor = index + 1;
    let mut last = None;
    while cursor < lines.len() {
        let Some(vals) = money_only(&lines[cursor]) else {
            break;
        };
        last = vals.last().copied();
        cursor += 1;
    }
    last.map(|price| (price, cursor))
}

fn money_only(line: &str) -> Option<Vec<f64>> {
    let stripped = line.replace("AED", "").replace("aed", "");
    if stripped.chars().any(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let vals = numbers_in(line);
    if vals.is_empty() {
        None
    } else {
        Some(vals)
    }
}

fn parse_mcdonalds(text: &str) -> Option<ParsedReceipt> {
    let lines = clean_lines(text);
    let rest = next_field(&lines, "Restaurant Number");
    let place = next_field(&lines, "Restaurant Name");
    let order = next_field(&lines, "Order Id");
    let date_raw = if next_field(&lines, "Delivery Date").is_empty() {
        next_field(&lines, "Date")
    } else {
        next_field(&lines, "Delivery Date")
    };
    let date = parse_mcd_date(&date_raw);
    let start = lines
        .iter()
        .position(|line| line.eq_ignore_ascii_case("item cost"))
        .map(|index| index + 1)
        .unwrap_or(0);
    let mut items = Vec::new();
    let mut index = start;
    while index < lines.len() {
        let line = &lines[index];
        if mcd_summary(line) {
            break;
        }
        if parse_number(line).is_some_and(|qty| qty > 0.0 && qty < 100.0 && !line.contains('.'))
            && index + 1 < lines.len()
        {
            let qty = parse_number(line).unwrap_or(1.0);
            let name = lines[index + 1].clone();
            let mut cursor = index + 2;
            let mut price = None;
            if cursor < lines.len() {
                if let Some(amount) = mcd_money(&lines[cursor]) {
                    price = Some(amount);
                    cursor += 1;
                }
            }
            if let Some(price) = price {
                if price > 0.0 && !mcd_summary(&name) && !skip_name(&name) {
                    items.push(priced(
                        &name,
                        qty,
                        (price / qty * 100.0).round() / 100.0,
                        price,
                    ));
                }
            }
            index = cursor;
            continue;
        }
        index += 1;
    }
    let stamp = if date.is_empty() {
        "nodate".into()
    } else {
        date.replace('-', "")
    };
    let invoice_no = if order.is_empty() {
        String::new()
    } else {
        format!(
            "{}-{order}-{stamp}",
            if rest.is_empty() {
                "mcd"
            } else {
                rest.as_str()
            }
        )
    };
    if invoice_no.is_empty() && items.is_empty() {
        return None;
    }
    let _ = place;
    Some(ParsedReceipt {
        store: StoreId::Mcdonalds,
        invoice_no,
        order_no: order,
        date,
        items,
    })
}

fn parse_amazon(store: StoreId, text: &str) -> Option<ParsedReceipt> {
    let order = after_label(text, &["Order #", "Order number", "Order Number"]);
    let order = first_token(&order);
    if order.is_empty() {
        return None;
    }
    Some(ParsedReceipt {
        store,
        invoice_no: order,
        order_no: String::new(),
        date: parse_any_date(text),
        items: Vec::new(),
    })
}

fn next_field(lines: &[String], label: &str) -> String {
    let want = label.trim_end_matches(':').to_lowercase();
    for (index, line) in lines.iter().enumerate() {
        if line.trim_end_matches(':').eq_ignore_ascii_case(&want) {
            return lines.get(index + 1).cloned().unwrap_or_default();
        }
    }
    String::new()
}

fn mcd_summary(line: &str) -> bool {
    let n = line.to_lowercase();
    n.starts_with("sub total")
        || n.starts_with("subtotal")
        || n.starts_with("delivery fee")
        || n.starts_with("total")
}

fn mcd_money(line: &str) -> Option<f64> {
    let trimmed = line.trim();
    let bare = trimmed
        .trim_start_matches("AED")
        .trim_start_matches("aed")
        .trim()
        .trim_end_matches("AED")
        .trim_end_matches("aed")
        .trim();
    if bare.chars().any(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    parse_number(bare)
}

fn field_rest(text: &str, labels: &[&str]) -> String {
    for line in text.lines() {
        let trimmed = line.trim();
        for label in labels {
            if let Some(rest) = strip_label(trimmed, label) {
                let value = rest
                    .trim()
                    .trim_matches(|c: char| c == ':' || c == '#' || c == '-')
                    .trim()
                    .to_string();
                if !value.is_empty() {
                    return value;
                }
            }
        }
    }
    String::new()
}

fn field_value(text: &str, labels: &[&str]) -> String {
    for line in text.lines() {
        let trimmed = line.trim();
        for label in labels {
            if let Some(rest) = strip_label(trimmed, label) {
                let value = rest
                    .trim()
                    .trim_matches(|c: char| c == ':' || c == '#' || c == '-');
                if !value.is_empty() {
                    return first_token(value);
                }
            }
        }
    }
    String::new()
}

fn after_label(text: &str, labels: &[&str]) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    // ASCII lowercasing keeps byte indexes aligned. `×` is two bytes, and a
    // full `to_lowercase()` can grow, so a find() index would land inside it.
    let lower = flat.to_ascii_lowercase();
    for label in labels {
        let needle = label.to_ascii_lowercase();
        if let Some(pos) = lower.find(&needle) {
            let rest = flat[pos + needle.len()..].trim();
            let rest = rest.trim_start_matches(|c: char| c == ':' || c == '#' || c == ' ');
            if let Some(token) = rest.split_whitespace().next() {
                if !token.is_empty() && token != ":" {
                    return token
                        .trim_matches(|c: char| c == ':' || c == ',')
                        .to_string();
                }
            }
        }
    }
    String::new()
}

fn strip_label<'a>(line: &'a str, label: &str) -> Option<&'a str> {
    let lower = line.to_ascii_lowercase();
    let needle = label.to_ascii_lowercase();
    let pos = lower.find(&needle)?;
    if pos != 0
        && line
            .as_bytes()
            .get(pos.saturating_sub(1))
            .is_some_and(|b| b.is_ascii_alphanumeric())
    {
        return None;
    }
    let end = pos + needle.len();
    if !line.is_char_boundary(pos) || !line.is_char_boundary(end) {
        return None;
    }
    Some(&line[end..])
}

fn first_token(value: &str) -> String {
    value
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_matches(|c: char| c == ':' || c == '#' || c == ',')
        .to_string()
}

fn first_number(value: &str) -> String {
    numbers_in(value)
        .first()
        .map(|n| n.to_string())
        .unwrap_or_default()
}

fn digits_run(value: &str) -> String {
    value
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit())
        .collect()
}

fn between_tags(slice: &str, start: char) -> String {
    let Some(pos) = slice.find(start) else {
        return String::new();
    };
    let rest = &slice[pos + 1..];
    let end = rest.find('<').unwrap_or(rest.len());
    html_unescape(rest[..end].trim())
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn between(html: &str, marker: &str, prefix: &str) -> String {
    let Some(pos) = html.find(marker) else {
        return String::new();
    };
    let rest = &html[pos..];
    let Some(hash) = rest.find(prefix) else {
        return String::new();
    };
    digits_run(&rest[hash + prefix.len()..])
}

fn html_to_text(html: &str) -> String {
    let raw = html_unescape(html);
    // ASCII lowercasing keeps the same bytes, so a byte index stays a char
    // boundary in both strings. A char index does not: `×` is two bytes.
    let lower = raw.to_ascii_lowercase();
    let mut out = String::new();
    let mut index = 0;
    while index < raw.len() {
        if lower.as_bytes().get(index) == Some(&b'<') {
            let tail = &lower[index..];
            if tail.starts_with("<script") || tail.starts_with("<style") {
                let end = if tail.starts_with("<script") {
                    tail.find("</script>").map(|pos| pos + "</script>".len())
                } else {
                    tail.find("</style>").map(|pos| pos + "</style>".len())
                };
                index += end.unwrap_or(tail.len());
                continue;
            }
            let close = tail.find('>').unwrap_or(0);
            let end = close.min(tail.len().saturating_sub(1));
            let tag = &tail[..=end];
            let block = tag.starts_with("</p")
                || tag.starts_with("</tr")
                || tag.starts_with("</td")
                || tag.starts_with("</th")
                || tag.starts_with("</div")
                || tag.starts_with("</li")
                || tag.starts_with("</h")
                || tag.starts_with("<br");
            if block {
                out.push('\n');
            }
            index = (index + close + 1).min(raw.len());
            continue;
        }
        let Some(ch) = raw[index..].chars().next() else {
            break;
        };
        out.push(ch);
        index += ch.len_utf8();
    }
    out.lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect::<Vec<_>>()
        .join("\n")
}

fn html_unescape(text: &str) -> String {
    text.replace("&nbsp;", " ")
        .replace("&times;", "×")
        .replace("&#215;", "×")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

fn clean_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|line| !line.is_empty())
        .collect()
}

fn careem_when(lines: &[String]) -> String {
    for line in lines {
        let parts: Vec<&str> = line.split(',').collect();
        let head = parts.first().copied().unwrap_or(line).trim();
        if let Some(date) = parse_long_date_loose(head) {
            if line.contains(':') {
                return date;
            }
        }
    }
    String::new()
}

fn parse_cf_date(raw: &str) -> String {
    let raw = raw.trim().trim_matches(|c: char| c == ':' || c == ',');
    parse_with(raw, &["%d-%b-%Y", "%d-%B-%Y"])
}

fn parse_long_date(raw: &str) -> String {
    let raw = raw.trim().trim_matches(|c: char| c == ':' || c == ',');
    parse_with(raw, &["%B %d, %Y", "%b %d, %Y", "%B %d %Y", "%b %d %Y"])
}

fn parse_long_date_loose(raw: &str) -> Option<String> {
    let date = parse_with(
        raw,
        &[
            "%d %B",
            "%d %b",
            "%B %d",
            "%b %d",
            "%d %B %Y",
            "%d %b %Y",
            "%B %d, %Y",
            "%b %d, %Y",
        ],
    );
    if date.is_empty() {
        None
    } else {
        Some(date)
    }
}

fn parse_dmy(raw: &str) -> String {
    let token = raw.split_whitespace().next().unwrap_or(raw);
    parse_with(token, &["%d/%m/%Y", "%d/%m/%y"])
}

fn parse_mcd_date(raw: &str) -> String {
    let token = raw.split_whitespace().next().unwrap_or(raw);
    parse_with(token, &["%d/%m/%y", "%d/%m/%Y"])
}

fn parse_any_date(raw: &str) -> String {
    let token = raw.split_whitespace().take(3).collect::<Vec<_>>().join(" ");
    let candidates = [
        parse_cf_date(raw),
        parse_long_date(raw),
        parse_dmy(raw),
        parse_mcd_date(raw),
        parse_with(raw, &["%Y-%m-%d", "%d %b %Y", "%d %B %Y"]),
        parse_with(&token, &["%b %d, %Y", "%B %d, %Y", "%d %b %Y", "%d %B %Y"]),
    ];
    candidates
        .into_iter()
        .find(|date| !date.is_empty())
        .unwrap_or_default()
}

fn parse_with(raw: &str, formats: &[&str]) -> String {
    let raw = raw
        .trim()
        .trim_matches(|c: char| matches!(c, ':' | ',' | '.'));
    if raw.is_empty() {
        return String::new();
    }
    for format in formats {
        if let Ok(date) = NaiveDate::parse_from_str(raw, format) {
            return date.format("%Y-%m-%d").to_string();
        }
    }
    String::new()
}

fn skip_name(name: &str) -> bool {
    let n = name.trim().to_lowercase();
    if n.is_empty() || junk_product_name(name) {
        return true;
    }
    const SKIP: &[&str] = &[
        "delivery charge",
        "service fee",
        "driver tip",
        "shipping",
        "express delivery",
        "delivery/order",
        "miscellaneous charges",
    ];
    n == "tips" || SKIP.iter().any(|needle| n.contains(needle))
}

fn priced(name: &str, qty: f64, unit_price: f64, line_total: f64) -> ParsedLine {
    ParsedLine {
        name: name.split_whitespace().collect::<Vec<_>>().join(" "),
        qty,
        unit_price,
        line_total,
    }
}

fn close(left: f64, right: f64) -> bool {
    (left - right).abs() <= (0.06_f64).max(0.02 * right.abs().max(1.0))
}

fn parse_number(token: &str) -> Option<f64> {
    let token = token
        .trim()
        .trim_matches(|c: char| matches!(c, ',' | ':' | '#'));
    if token.is_empty() || !token.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    if !token
        .chars()
        .all(|c| c.is_ascii_digit() || c == '.' || c == '-')
    {
        return None;
    }
    token.parse().ok()
}

fn numbers_in(text: &str) -> Vec<f64> {
    let mut out = Vec::new();
    let mut current = String::new();
    for ch in text.chars().chain(std::iter::once(' ')) {
        if ch.is_ascii_digit() || (ch == '.' && !current.is_empty()) {
            current.push(ch);
        } else if !current.is_empty() {
            if let Ok(number) = current.parse::<f64>() {
                out.push(number);
            }
            current.clear();
        }
    }
    out
}

fn split_trailing_money(text: &str) -> Option<(String, f64)> {
    let lower = text.to_ascii_lowercase();
    let marker = lower.rfind("aed");
    let (name, money) = if let Some(pos) = marker {
        let after = text[pos + 3..].trim();
        if let Some(total) = trailing_money(after).or_else(|| numbers_in(after).last().copied()) {
            return Some((text[..pos].trim().to_string(), total));
        }
        let before = text[..pos].trim();
        if let Some(total) = trailing_money(before) {
            let name_end = before
                .rfind(|c: char| c.is_ascii_digit())
                .and_then(|end| before[..end].rfind(|c: char| !c.is_ascii_digit() && c != '.'));
            if let Some(end) = name_end {
                return Some((before[..=end].trim().to_string(), total));
            }
        }
        return None;
    } else {
        (text, text)
    };
    let total = trailing_money(money)?;
    let name = name.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.' || c == ' ');
    if name.is_empty() {
        None
    } else {
        Some((name.trim().to_string(), total))
    }
}

fn trailing_money(text: &str) -> Option<f64> {
    let values = numbers_in(text);
    let last = *values.last()?;
    if text.contains('.') {
        Some(last)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{
        html_to_text, locate_receipt_file, parse_careem, parse_carrefour, parse_grandiose,
        parse_mmi, read_receipt,
    };
    use crate::model::StoreId;
    use std::fs;

    #[test]
    fn an_upgrade_copies_bringfast_receipts_and_fills_source_file() {
        let dir = tempfile::tempdir().unwrap();
        let app = crate::App::open(dir.path().join("oma")).unwrap();
        app.with_conn(|conn| {
            conn.execute(
                "INSERT INTO invoices (store, invoice_no, purchased_on, currency, total) VALUES ('carrefour', '1001', '2026-03-01', 'AED', 9.5)",
                [],
            )?;
            conn.execute(
                "INSERT INTO invoices (store, invoice_no, purchased_on, currency, total) VALUES ('union_coop', 'U-2008', '2026-03-02', 'AED', 4.0)",
                [],
            )?;
            conn.execute(
                "INSERT INTO invoices (store, invoice_no, purchased_on, currency, total) VALUES ('grandiose', 'SLIP-9', '2026-03-03', 'AED', 19.0)",
                [],
            )?;
            conn.execute(
                "INSERT INTO invoices (store, invoice_no, purchased_on, currency, total) VALUES ('carrefour', '93084417', '2026-08-23', 'AED', 11.98)",
                [],
            )?;
            Ok(())
        })
        .unwrap();

        let incoming = dir.path().join("incoming");
        fs::create_dir_all(incoming.join("carrefour")).unwrap();
        fs::create_dir_all(incoming.join("unioncoop")).unwrap();
        fs::write(incoming.join("carrefour/1001.pdf"), b"%PDF-1.4 carrefour").unwrap();
        fs::write(incoming.join("unioncoop/order-U-2008.png"), b"png-bytes").unwrap();
        let outside = dir.path().join("mail/slip.pdf");
        fs::create_dir_all(outside.parent().unwrap()).unwrap();
        fs::write(&outside, b"%PDF-1.4 grandiose").unwrap();
        fs::write(
            incoming.join("carrefour/weird-name.html"),
            "Tax Invoice\nCarrefour\nInvoice No. : 93084417\nInvoice Date : 23-Aug-2026\nCoca-Cola Zero Sugar 2.0 2.0 5.99 5.71 11.41 5.00 0.57 0.00 11.98\n",
        )
        .unwrap();

        let source = dir.path().join("bringfast.db");
        let bf = rusqlite::Connection::open(&source).unwrap();
        bf.execute_batch(
            "CREATE TABLE invoices (retailer TEXT, invoice_no TEXT, order_no TEXT, source_file TEXT);",
        )
        .unwrap();
        bf.execute(
            "INSERT INTO invoices (retailer, invoice_no, order_no, source_file) VALUES ('grandiose', 'SLIP-9', '9', ?1)",
            rusqlite::params![outside.display().to_string()],
        )
        .unwrap();
        drop(bf);

        let data_dir = app.data_dir().to_path_buf();
        app.with_conn(|conn| {
            conn.execute(
                "INSERT INTO app_settings (key, value) VALUES ('receipts_dir', ?1), ('bringfast_source', ?2)",
                rusqlite::params![incoming.display().to_string(), source.display().to_string()],
            )?;
            let linked = super::attach_missing_receipts(conn, &data_dir)?;
            assert_eq!(linked, 4);
            let stored: Vec<(String, String)> = {
                let mut stmt = conn.prepare(
                    "SELECT invoice_no, source_file FROM invoices ORDER BY invoice_no",
                )?;
                let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
                rows.collect::<Result<Vec<_>, _>>()?
            };
            for (invoice_no, path) in &stored {
                let file = std::path::Path::new(path);
                assert!(file.is_file(), "{invoice_no} -> {path}");
                assert!(
                    path.contains("/receipts/"),
                    "{invoice_no} was not copied into the data dir: {path}"
                );
            }
            let again = super::attach_missing_receipts(conn, &data_dir)?;
            assert_eq!(again, 0);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn a_receipt_file_is_found_under_the_store_and_the_order_name() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("receipts/carrefour");
        fs::create_dir_all(&folder).unwrap();
        let file = folder.join("order-1001.png");
        fs::write(&file, b"png").unwrap();
        let found = locate_receipt_file(dir.path(), "carrefour", "1001", "").unwrap();
        assert_eq!(found, file);
        let stored =
            locate_receipt_file(dir.path(), "carrefour", "missing", file.to_str().unwrap());
        assert_eq!(stored.unwrap(), file);
        assert!(locate_receipt_file(dir.path(), "carrefour", "nope", "").is_none());
    }

    const CF: &str = "
Invoice No. : 93084417
Invoice Date : 23-Aug-2026
Order No. : 784030019566483
City Center Meaisem
Coca-Cola Zero Sugar 2.0 2.0 5.99 5.71 11.41 5.00 0.57 0.00 11.98
Carbonated Soft Drink PET
Barcode: 5000112668209
Delivery Charge 1.0 1.0 0.00 0.00 0.00 5.00 0.00 0.00 0.00
Service Fee 1.0 1.0 3.42 3.26 3.26 5.00 0.16 0.00 3.42
";

    #[test]
    fn store_names_and_size_fragments_are_not_items() {
        assert!(super::skip_name("Grandiose"));
        assert!(super::skip_name("GRANDIOSE!!!"));
        assert!(super::skip_name("Grand\u{200b}iose"));
        assert!(super::skip_name(
            "Grandiose Supermarket Sole Proprietorship LLC"
        ));
        assert!(!super::skip_name("Grandiose Chicken"));
        assert!(!super::skip_name("Large (13\")"));
        assert!(!super::skip_name("Large (13\u{2033})"));
        assert!(!super::skip_name("Tuna Steak"));
        assert!(!super::skip_name("Oasis Blu"));
        assert!(!super::skip_name("P.SOLEIL"));
        assert!(!super::skip_name("AL JAZIRA EGGS"));
        assert!(!super::skip_name("RANA PESTO"));
        assert!(!super::skip_name("Rice 5kg"));
        assert!(!super::skip_name("Carrefour Chicken"));
        assert!(super::careem_item("Grandiose", 1.0, 12.0).is_none());
        assert!(super::careem_item("Large (13\")", 1.0, 15.0).is_some());
        assert!(super::careem_item("Tuna Steak", 1.0, 12.0).is_some());
    }

    #[test]
    fn carrefour_keeps_the_product_and_drops_fees() {
        let out = parse_carrefour(CF).unwrap();
        assert_eq!(out.invoice_no, "93084417");
        assert_eq!(out.date, "2026-08-23");
        assert_eq!(out.items.len(), 1);
        assert_eq!(out.items[0].name, "Coca-Cola Zero Sugar");
        assert!((out.items[0].qty - 2.0).abs() < 0.01);
        assert!((out.items[0].line_total - 11.98).abs() < 0.01);
    }

    #[test]
    fn grandiose_slip_reads_the_name_under_the_sku() {
        let text = "
TAX INVOICE
Grandiose Supermarket Sole Proprietorship LLC
Slip: 000000ST11000167487
Date: 06/08/2026 7:50 PM
Items Qty. Price Amount
3000000003617 1 19.00 19.00
Veroni Spianata Romana Slices
8005573008356 1 19.00 19.00
Veroni Salami Napoli Slices
3000000003073 1 0.60 0.60
Pacific Printed Brown Paper Bag
Total 38.60
";
        let out = parse_grandiose(text).unwrap();
        assert_eq!(out.invoice_no, "000000ST11000167487");
        assert_eq!(out.date, "2026-08-06");
        assert_eq!(out.items.len(), 3);
        assert!((out.items[0].line_total - 19.0).abs() < 0.01);
        assert_eq!(out.items[0].name, "Veroni Spianata Romana Slices");
    }

    #[test]
    fn mmi_reads_the_last_amount() {
        let text = "
Invoice No: ARInv-12869998
Invoice Date: 01-Apr-2026
Customer Ref: 260401HVW9G0OR_Emiliano
MMI Home Delivery
02420 HEINEKEN CANS 50 CL 2.00 CS24 131.14 262.27 26.23 209.81 62.94 272.75 05 13.64 286.39
Express Delivery Charge 9.52 05 0.48 10
";
        let out = parse_mmi(text).unwrap();
        assert_eq!(out.invoice_no, "ARInv-12869998");
        assert_eq!(out.date, "2026-04-01");
        assert_eq!(out.items[0].name, "HEINEKEN CANS 50 CL");
        assert!((out.items[0].line_total - 286.39).abs() < 0.01);
    }

    #[test]
    fn careem_keeps_dishes() {
        let text = "
Tax Invoice
Careem Networks FZ LLC
Invoice No: CRM-2026-884213
Order ID: 8842137755
Your order from: Al Safadi Restaurant
Invoice Date: 26 Aug 2026
2 x Chicken Shawarma Wrap AED 36.00
1 x Mixed Grill Platter AED 58.50
Hummus Beiruty AED 18.00
Delivery fee AED 5.00
Service fee AED 2.50
Total AED 123.13
";
        let out = parse_careem(text).unwrap();
        assert_eq!(out.store, StoreId::Careem);
        assert_eq!(out.invoice_no, "CRM-2026-884213");
        assert_eq!(out.date, "2026-08-26");
        assert_eq!(
            out.items
                .iter()
                .map(|item| item.name.as_str())
                .collect::<Vec<_>>(),
            vec![
                "Chicken Shawarma Wrap",
                "Mixed Grill Platter",
                "Hummus Beiruty"
            ]
        );
        assert!((out.items[0].qty - 2.0).abs() < 0.01);
        assert!((out.items[0].line_total - 36.0).abs() < 0.01);
    }

    #[test]
    fn mcdonalds_html_keeps_priced_rows() {
        let html = r#"
<div>We got your order, here's your receipt.</div>
<div>Restaurant Number:</div><div>1840105</div>
<div>Restaurant Name:</div><div>ENOC 1039 Dubai Production City</div>
<div>Order Id:</div><div>0786</div>
<div>Delivery Date:</div><div>05/04/26 17:06</div>
<table>
 <tr><th>Qty</th><th>Item Detail</th><th>Item Cost</th></tr>
 <tr><td>1</td><td>Double Cheeseburger Large Meal</td><td>AED 29.00</td></tr>
 <tr><td>1</td><td>Cheeseburger</td><td>AED 8.00</td></tr>
 <tr><td colspan="2">Sub Total</td><td>AED 37.00</td></tr>
 <tr><td colspan="2">Total (includes 5% VAT)</td><td>AED 45.50</td></tr>
</table>
"#;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcdonalds.html");
        fs::write(&path, html).unwrap();
        let out = read_receipt(&path).unwrap();
        assert_eq!(out.store, StoreId::Mcdonalds);
        assert_eq!(out.invoice_no, "1840105-0786-20260405");
        assert_eq!(out.date, "2026-04-05");
        assert_eq!(out.items.len(), 2);
        assert!((out.items[0].line_total - 29.0).abs() < 0.01);
    }

    #[test]
    fn times_sign_does_not_split_the_html_on_a_char_boundary() {
        let html = "<div>2 &times; Chicken Shawarma</div><br><span>Hummus</span>";
        let text = html_to_text(html);
        assert!(text.contains("2 × Chicken Shawarma"), "{text}");
        assert!(text.contains("Hummus"), "{text}");
        let mut long = String::from("a");
        long.push_str(&"×".repeat(80));
        long.push_str("<br><span>Hummus</span>");
        assert!(html_to_text(&long).contains("Hummus"));
    }
}

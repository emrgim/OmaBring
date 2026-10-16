//! Read a Bring Fast receipts folder (`store/invoice.pdf` or `.html`).
//!
//! A file whose receipt number is already stored is skipped. The line readers
//! follow Bring Fast's invoice parser: supermarket PDFs, Careem mail, and
//! McDonald's order HTML.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use chrono::NaiveDate;
use rusqlite::{params, Connection};

use crate::bringfast::ImportReport;
use crate::error::{Error, Result};
use crate::model::{normalize_name, StoreId};

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
                if known_receipt(
                    dest,
                    parsed.store,
                    &parsed.invoice_no,
                    &parsed.order_no,
                    &path,
                )? {
                    report.files_existing += 1;
                    continue;
                }
                report.files_new += 1;
                report.lines += parsed.items.len();
                if dry_run {
                    continue;
                }
                if let Err(err) = insert_receipt(dest, &parsed) {
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
) -> Result<bool> {
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
        if found.is_some() {
            return Ok(true);
        }
    }
    Ok(false)
}

fn insert_receipt(dest: &Connection, parsed: &ParsedReceipt) -> Result<()> {
    let total: f64 = parsed.items.iter().map(|line| line.line_total).sum();
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
    for line in &parsed.items {
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
    Ok(())
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
    if n.is_empty() {
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
        html_to_text, parse_careem, parse_carrefour, parse_grandiose, parse_mmi, read_receipt,
    };
    use crate::model::StoreId;
    use std::fs;

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

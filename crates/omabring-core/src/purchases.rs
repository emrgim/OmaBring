use chrono::{Datelike, Duration, NaiveDate};
use serde::Serialize;

use crate::catalog::{official_ean, raw_product_key};
use crate::depts::{classify_dept, matches_dept};
use crate::error::{Error, Result};
use crate::invoice::{LineItem, ParsedInvoice};
use crate::macros::{self, classify_macro};
use crate::Db;

#[derive(Clone, Debug, Serialize)]
pub struct ProductRow {
    pub key: String,
    pub name: String,
    pub receipt_name: String,
    pub official_name: String,
    pub barcode: String,
    pub qty_total: f64,
    pub times_bought: i64,
    pub frequency: String,
    pub interval_days: f64,
    pub spend_total: f64,
    pub typical_unit_aed: Option<f64>,
    pub first_buy: String,
    pub last_buy: String,
    pub dept: String,
    pub macro_category: String,
    pub next_due: String,
    pub due_in_days: Option<i64>,
    pub status: String,
    pub likely: i32,
    pub likely_reason: String,
    pub likely_vote: String,
    pub likely_push: i32,
}

#[derive(Clone, Debug, Serialize)]
pub struct SpendBar {
    pub label: String,
    pub start: String,
    pub end: String,
    pub total: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct NamedAmount {
    pub name: String,
    pub total: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct SpendReport {
    pub range: String,
    pub since: String,
    pub until: String,
    pub total: f64,
    pub average: f64,
    pub period_count: i64,
    pub grain: String,
    pub bars: Vec<SpendBar>,
    pub by_retailer: Vec<NamedAmount>,
    pub by_category: Vec<NamedAmount>,
}

pub fn normalize_range(range_key: &str) -> String {
    let raw = range_key.trim().to_ascii_lowercase().replace([' ', '-'], "_");
    match raw.as_str() {
        "lastmonth" | "prev_month" | "previous_month" | "mese_scorso" | "last_calendar_month" => "last_month".into(),
        "thismonth" | "current_month" | "mese" => "this_month".into(),
        "1w" | "2w" | "1m" | "3m" | "1y" | "2y" | "3y" | "all" | "custom" | "this_month" | "last_month" => raw,
        _ => "all".into(),
    }
}

pub fn window(range_key: &str, start: &str, end: NaiveDate) -> (Option<NaiveDate>, NaiveDate, String) {
    if let Some(since) = parse_day(start) {
        return (Some(since), end, "custom".into());
    }
    let key = normalize_range(range_key);
    match key.as_str() {
        "last_month" => {
            let last_prev = end.with_day(1).unwrap_or(end) - Duration::days(1);
            (Some(last_prev.with_day(1).unwrap_or(last_prev)), last_prev, key)
        }
        "this_month" => (Some(end.with_day(1).unwrap_or(end)), end, key),
        "1w" => (Some(end - Duration::days(6)), end, key),
        "2w" => (Some(end - Duration::days(13)), end, key),
        "1m" => (Some(end - Duration::days(29)), end, key),
        "3m" => (Some(end - Duration::days(89)), end, key),
        "1y" => (Some(end - Duration::days(364)), end, key),
        "2y" => (Some(end - Duration::days(729)), end, key),
        "3y" => (Some(end - Duration::days(1094)), end, key),
        _ => (None, end, "all".into()),
    }
}

pub fn parse_day(raw: &str) -> Option<NaiveDate> {
    let raw = raw.trim();
    if raw.len() < 10 {
        return None;
    }
    NaiveDate::parse_from_str(&raw[..10], "%Y-%m-%d").ok()
}

fn median(vals: &[f64]) -> Option<f64> {
    if vals.is_empty() {
        return None;
    }
    let mut ordered = vals.to_vec();
    ordered.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = ordered.len();
    let mid = n / 2;
    if n % 2 == 1 {
        Some(ordered[mid])
    } else {
        Some((ordered[mid - 1] + ordered[mid]) / 2.0)
    }
}

fn same_price_unit(first: f64, last: f64) -> bool {
    if first <= 0.0 || last <= 0.0 {
        return false;
    }
    let ratio = last / first;
    (1.0 / 3.0) <= ratio && ratio <= 3.0
}

fn pretty_unit(n: f64, unit: &str) -> String {
    if (n - n.round()).abs() < 0.08 {
        let v = n.round().max(1.0) as i64;
        if v == 1 {
            format!("every {v} {unit}")
        } else {
            format!("every {v} {unit}s")
        }
    } else {
        format!("every {n:.1} {unit}s")
    }
}

fn fmt_every(days: f64) -> String {
    if days < 12.0 {
        pretty_unit(days.max(1.0), "day")
    } else if days < 25.0 {
        pretty_unit(days / 7.0, "week")
    } else if days < 700.0 {
        pretty_unit(days / 30.437, "month")
    } else {
        pretty_unit(days / 365.25, "year")
    }
}

fn frequency(first: &str, times: i64, until: NaiveDate) -> (String, f64) {
    let Some(start) = parse_day(first) else {
        return ("—".into(), 1e9);
    };
    if times < 1 {
        return ("—".into(), 1e9);
    }
    let end = if until < start { start } else { until };
    let span = (end - start).num_days().max(1) as f64;
    let interval = span / times as f64;
    (fmt_every(interval), interval)
}

fn round2(n: f64) -> f64 {
    (n * 100.0).round() / 100.0
}

impl Db {
    pub fn canonical_key(&self, key: &str) -> Result<String> {
        let key = key.trim();
        if key.is_empty() {
            return Ok(String::new());
        }
        let conn = self.connect()?;
        let alias: Option<String> = conn
            .query_row("SELECT canonical_key FROM product_aliases WHERE alias_key=?1", [key], |r| r.get(0))
            .ok();
        if let Some(hit) = alias {
            return Ok(hit);
        }
        if let Some(ean) = key.strip_prefix("ean:") {
            let full = official_ean(ean);
            if !full.is_empty() && full != ean {
                let cand = format!("ean:{full}");
                let alias: Option<String> = conn
                    .query_row(
                        "SELECT canonical_key FROM product_aliases WHERE alias_key=?1",
                        [&cand],
                        |r| r.get(0),
                    )
                    .ok();
                if let Some(hit) = alias {
                    return Ok(hit);
                }
                let meta: Option<String> = conn
                    .query_row(
                        "SELECT product_key FROM product_meta WHERE product_key=?1 OR official_ean=?2",
                        rusqlite::params![cand, full],
                        |r| r.get(0),
                    )
                    .ok();
                if let Some(hit) = meta {
                    return Ok(hit);
                }
            }
            if !full.is_empty() {
                let meta: Option<String> = conn
                    .query_row(
                        "SELECT product_key FROM product_meta WHERE official_ean=?1",
                        [&full],
                        |r| r.get(0),
                    )
                    .ok();
                if let Some(hit) = meta {
                    return Ok(hit);
                }
            }
        }
        Ok(key.to_string())
    }

    pub fn product_key(&self, barcode: &str, name: &str) -> Result<String> {
        self.canonical_key(&raw_product_key(barcode, name))
    }

    fn ensure_macro(&self, key: &str, receipt_name: &str) -> Result<String> {
        let conn = self.connect()?;
        let existing: Option<String> = conn
            .query_row(
                "SELECT macro_category FROM product_meta WHERE product_key=?1",
                [key],
                |r| r.get(0),
            )
            .ok();
        let existing = existing.unwrap_or_default();
        let normalized = macros::normalize_macro(&existing);
        if !normalized.is_empty() {
            return Ok(normalized);
        }
        let official: String = conn
            .query_row(
                "SELECT official_name FROM product_meta WHERE product_key=?1",
                [key],
                |r| r.get(0),
            )
            .unwrap_or_default();
        let category = classify_macro(receipt_name, &official);
        conn.execute(
            "INSERT INTO product_meta(product_key, sku, category, official_name, image_url, source, official_ean, macro_category)
             VALUES (?1,'','','','','','',?2)
             ON CONFLICT(product_key) DO UPDATE SET
               macro_category=CASE WHEN ifnull(product_meta.macro_category,'')!='' THEN product_meta.macro_category ELSE excluded.macro_category END",
            rusqlite::params![key, category],
        )?;
        Ok(category)
    }

    pub fn upsert_invoice(&self, user_id: i64, parsed: &ParsedInvoice) -> Result<Option<i64>> {
        if parsed.invoice_no.trim().is_empty() || parsed.items.is_empty() {
            return Ok(None);
        }
        let conn = self.connect()?;
        conn.execute(
            "INSERT INTO invoices(user_id, retailer, invoice_no, order_no, invoice_date, store_name, gmail_id, source_file)
             VALUES (?1,?2,?3,?4,?5,?6,'',?7)
             ON CONFLICT(user_id, retailer, invoice_no) DO UPDATE SET
               order_no=excluded.order_no,
               invoice_date=excluded.invoice_date,
               store_name=excluded.store_name,
               source_file=excluded.source_file",
            rusqlite::params![
                user_id,
                parsed.retailer,
                parsed.invoice_no,
                parsed.order_no,
                parsed.invoice_date,
                if parsed.store_name.is_empty() { parsed.retailer.clone() } else { parsed.store_name.clone() },
                parsed.source,
            ],
        )?;
        let invoice_id: i64 = conn.query_row(
            "SELECT id FROM invoices WHERE user_id=?1 AND retailer=?2 AND invoice_no=?3",
            rusqlite::params![user_id, parsed.retailer, parsed.invoice_no],
            |r| r.get(0),
        )?;
        conn.execute("DELETE FROM invoice_items WHERE invoice_id=?1", [invoice_id])?;
        let mut pending = Vec::new();
        for it in &parsed.items {
            let name = it.name.trim();
            if name.is_empty() {
                continue;
            }
            let key = self.product_key(&it.barcode, name)?;
            conn.execute(
                "INSERT INTO invoice_items(invoice_id, barcode, name, qty, unit_price, line_total, image_url, product_key)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                rusqlite::params![
                    invoice_id,
                    it.barcode.trim(),
                    name,
                    it.qty,
                    it.unit_price,
                    it.line_total,
                    it.image_url,
                    key,
                ],
            )?;
            pending.push((key, name.to_string()));
        }
        drop(conn);
        for (key, name) in pending {
            let _ = self.ensure_macro(&key, &name);
        }
        Ok(Some(invoice_id))
    }

    pub fn delete_invoice(
        &self,
        user_id: i64,
        invoice_id: Option<i64>,
        retailer: Option<&str>,
        invoice_no: Option<&str>,
    ) -> Result<Option<serde_json::Value>> {
        let retailer_s = retailer.unwrap_or("").trim().to_ascii_lowercase();
        let invoice_no_s = invoice_no.unwrap_or("").trim().to_string();
        let has_id = invoice_id.is_some();
        let has_pair = !retailer_s.is_empty() && !invoice_no_s.is_empty();
        if has_id == has_pair {
            return Err(Error::Invalid("pass invoice_id or retailer and invoice_no".into()));
        }
        let conn = self.connect()?;
        let row = if let Some(id) = invoice_id {
            conn.query_row(
                "SELECT id, invoice_no, retailer FROM invoices WHERE id=?1 AND user_id=?2",
                rusqlite::params![id, user_id],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)),
            )
            .ok()
        } else {
            conn.query_row(
                "SELECT id, invoice_no, retailer FROM invoices WHERE user_id=?1 AND retailer=?2 AND invoice_no=?3",
                rusqlite::params![user_id, retailer_s, invoice_no_s],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)),
            )
            .ok()
        };
        let Some((id, no, store)) = row else { return Ok(None) };
        let items: i64 = conn.query_row("SELECT COUNT(*) FROM invoice_items WHERE invoice_id=?1", [id], |r| r.get(0))?;
        conn.execute("DELETE FROM invoice_items WHERE invoice_id=?1", [id])?;
        conn.execute("DELETE FROM invoices WHERE id=?1 AND user_id=?2", rusqlite::params![id, user_id])?;
        Ok(Some(serde_json::json!({
            "invoice_id": id,
            "invoice_no": no,
            "retailer": store,
            "items_removed": items,
        })))
    }

    pub fn first_invoice_date(&self, user_id: i64) -> Result<Option<NaiveDate>> {
        let conn = self.connect()?;
        let raw: Option<String> = conn
            .query_row("SELECT MIN(invoice_date) FROM invoices WHERE user_id=?1", [user_id], |r| r.get(0))
            .ok()
            .flatten();
        Ok(raw.as_deref().and_then(parse_day))
    }

    pub fn resolve_window(&self, user_id: i64, range_key: &str, end: NaiveDate) -> Result<(Option<NaiveDate>, NaiveDate, String)> {
        let (since, until, key) = window(range_key, "", end);
        if key == "all" && since.is_none() {
            if let Some(first) = self.first_invoice_date(user_id)? {
                return Ok((Some(first), until, key));
            }
        }
        Ok((since, until, key))
    }

    fn line_rows(&self, user_id: i64, since: Option<NaiveDate>, until: NaiveDate) -> Result<Vec<(String, String, String, f64, f64, Option<f64>, String)>> {
        let conn = self.connect()?;
        let since_s = since.map(|d| d.format("%Y-%m-%d").to_string()).unwrap_or_else(|| "0000-01-01".into());
        let until_s = until.format("%Y-%m-%d").to_string();
        let mut stmt = conn.prepare(
            "SELECT substr(i.invoice_date,1,10), i.retailer, it.product_key, it.line_total, it.qty, it.unit_price, it.name
             FROM invoice_items it
             JOIN invoices i ON i.id = it.invoice_id
             WHERE i.user_id=?1 AND substr(i.invoice_date,1,10)>=?2 AND substr(i.invoice_date,1,10)<=?3",
        )?;
        let rows = stmt.query_map(rusqlite::params![user_id, since_s, until_s], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, f64>(3)?,
                r.get::<_, f64>(4)?,
                r.get::<_, Option<f64>>(5)?,
                r.get::<_, String>(6)?,
            ))
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn spend(&self, user_id: i64, range_key: &str, end: NaiveDate, grain: &str) -> Result<SpendReport> {
        let (since, until, key) = self.resolve_window(user_id, range_key, end)?;
        let since = since.unwrap_or(until);
        let grain = match grain {
            "daily" | "weekly" | "monthly" | "yearly" => grain.to_string(),
            _ => default_grain(&key),
        };
        let rows = self.line_rows(user_id, Some(since), until)?;
        let total = round2(rows.iter().map(|r| r.3).sum());
        let buckets = period_buckets(since, until, &grain);
        let mut bars = Vec::new();
        for (start, stop, label) in &buckets {
            let sum: f64 = rows
                .iter()
                .filter(|r| {
                    parse_day(&r.0).map(|d| d >= *start && d <= *stop).unwrap_or(false)
                })
                .map(|r| r.3)
                .sum();
            bars.push(SpendBar {
                label: label.clone(),
                start: start.format("%Y-%m-%d").to_string(),
                end: stop.format("%Y-%m-%d").to_string(),
                total: round2(sum),
            });
        }
        let period_count = bars.len() as i64;
        let average = if period_count == 0 { 0.0 } else { round2(total / period_count as f64) };
        let mut by_store: Vec<(String, f64)> = Vec::new();
        let mut by_cat: Vec<(String, f64)> = Vec::new();
        for row in &rows {
            add_named(&mut by_store, &row.1, row.3);
            let macro_name = self
                .macro_for(&row.2)
                .unwrap_or_else(|_| classify_macro(&row.6, ""));
            add_named(&mut by_cat, &macros::macro_label(&macro_name), row.3);
        }
        by_store.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        by_cat.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        Ok(SpendReport {
            range: key,
            since: since.format("%Y-%m-%d").to_string(),
            until: until.format("%Y-%m-%d").to_string(),
            total,
            average,
            period_count,
            grain,
            bars,
            by_retailer: by_store.into_iter().map(|(name, total)| NamedAmount { name, total: round2(total) }).collect(),
            by_category: by_cat.into_iter().map(|(name, total)| NamedAmount { name, total: round2(total) }).collect(),
        })
    }

    fn macro_for(&self, key: &str) -> Result<String> {
        let conn = self.connect()?;
        let raw: String = conn
            .query_row("SELECT macro_category FROM product_meta WHERE product_key=?1", [key], |r| r.get(0))
            .unwrap_or_default();
        Ok(macros::normalize_macro(&raw))
    }

    pub fn typical_prices(&self, user_id: i64, since: Option<NaiveDate>, until: NaiveDate) -> Result<Vec<(String, f64)>> {
        let rows = self.line_rows(user_id, since, until)?;
        let mut buckets: Vec<(String, Vec<f64>)> = Vec::new();
        for row in rows {
            let price = row.5.filter(|p| *p > 0.0).or_else(|| if row.4 > 0.0 { Some(row.3 / row.4) } else { None });
            let Some(price) = price.filter(|p| *p > 0.0) else { continue };
            if let Some(slot) = buckets.iter_mut().find(|(k, _)| k == &row.2) {
                slot.1.push(price);
            } else {
                buckets.push((row.2, vec![price]));
            }
        }
        let mut out = Vec::new();
        for (key, prices) in buckets {
            let Some(mid) = median(&prices).filter(|m| *m > 0.0) else { continue };
            let kept: Vec<f64> = prices.iter().copied().filter(|p| same_price_unit(mid, *p)).collect();
            if let Some(typical) = median(if kept.is_empty() { &prices } else { &kept }) {
                out.push((key, round2(typical)));
            }
        }
        Ok(out)
    }

    pub fn list_products(&self, user_id: i64, since: Option<NaiveDate>, until: NaiveDate, dept: &str) -> Result<Vec<ProductRow>> {
        let conn = self.connect()?;
        let since_s = since.map(|d| d.format("%Y-%m-%d").to_string()).unwrap_or_else(|| "0000-01-01".into());
        let until_s = until.format("%Y-%m-%d").to_string();
        let mut stmt = conn.prepare(
            "SELECT it.product_key,
                    MAX(it.name),
                    MAX(NULLIF(pm.official_name,'')),
                    MAX(it.barcode),
                    SUM(it.qty),
                    COUNT(DISTINCT i.id),
                    SUM(it.line_total),
                    MIN(NULLIF(substr(i.invoice_date,1,10),'')),
                    MAX(substr(i.invoice_date,1,10)),
                    MAX(NULLIF(pm.macro_category,''))
             FROM invoice_items it
             JOIN invoices i ON i.id = it.invoice_id
             LEFT JOIN product_meta pm ON pm.product_key = it.product_key
             WHERE i.user_id=?1 AND substr(i.invoice_date,1,10)>=?2 AND substr(i.invoice_date,1,10)<=?3
             GROUP BY it.product_key",
        )?;
        let rows = stmt.query_map(rusqlite::params![user_id, since_s, until_s], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1).unwrap_or_default(),
                r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                r.get::<_, f64>(4).unwrap_or(0.0),
                r.get::<_, i64>(5).unwrap_or(0),
                r.get::<_, f64>(6).unwrap_or(0.0),
                r.get::<_, Option<String>>(7)?.unwrap_or_default(),
                r.get::<_, Option<String>>(8)?.unwrap_or_default(),
                r.get::<_, Option<String>>(9)?.unwrap_or_default(),
            ))
        })?;
        let typical = self.typical_prices(user_id, since, until)?;
        let classified = self.forecast(user_id, until, 365, "", true, &[])?;
        let mut out = Vec::new();
        for row in rows {
            let (key, receipt, official, barcode, qty, times, spend, first, last, macro_raw) = row?;
            if !matches_dept(dept, &receipt, &official) {
                continue;
            }
            let (label, interval) = frequency(&first, times, until);
            let display = if official.is_empty() { receipt.clone() } else { official.clone() };
            let dept_label = classify_dept(&receipt, &official);
            let hab = classified.iter().find(|c| c.key == key);
            let (next_due, due_in, status) = cadence_status(&last, interval, times, until);
            out.push(ProductRow {
                key: key.clone(),
                name: display,
                receipt_name: receipt,
                official_name: official,
                barcode,
                qty_total: qty,
                times_bought: times,
                frequency: label,
                interval_days: interval,
                spend_total: round2(spend),
                typical_unit_aed: typical.iter().find(|(k, _)| k == &key).map(|(_, p)| *p),
                first_buy: first,
                last_buy: last,
                dept: dept_label,
                macro_category: macros::normalize_macro(&macro_raw),
                next_due,
                due_in_days: due_in,
                status,
                likely: hab.map(|h| h.score).unwrap_or(0),
                likely_reason: hab.map(|h| h.reason.clone()).unwrap_or_default(),
                likely_vote: hab.map(|h| h.vote.clone()).unwrap_or_default(),
                likely_push: hab.map(|h| h.likely_push).unwrap_or(0),
            });
        }
        Ok(out)
    }

    pub fn shopping_list(&self, user_id: i64, today: NaiveDate, horizon_days: i64, limit: usize, dept: &str) -> Result<Vec<ProductRow>> {
        let horizon = horizon_days.max(0);
        let products = self.list_products(user_id, None, today, dept)?;
        let mut rows: Vec<ProductRow> = products
            .into_iter()
            .filter(|p| p.status != "unknown" && p.status != "lapsed")
            .filter(|p| {
                let name = p.name.to_ascii_lowercase();
                !["plastic bag", "shopping bag", "plast shopping", "t shirt hndl bag"]
                    .iter()
                    .any(|bit| name.contains(bit))
            })
            .filter(|p| p.due_in_days.map(|d| d <= horizon).unwrap_or(false))
            .filter(|p| {
                parse_day(&p.last_buy).map(|d| (today - d).num_days() <= 90).unwrap_or(true)
            })
            .filter(|p| p.likely_push >= 0)
            .collect();
        rows.sort_by(|a, b| {
            b.likely.cmp(&a.likely).then(b.likely_push.cmp(&a.likely_push)).then(a.due_in_days.unwrap_or(0).cmp(&b.due_in_days.unwrap_or(0)))
        });
        rows.truncate(limit.clamp(1, 50));
        Ok(rows)
    }

    pub fn ranked_products(&self, user_id: i64, sort: &str, today: NaiveDate, range_key: &str, limit: usize) -> Result<Vec<ProductRow>> {
        let (since, until, _) = self.resolve_window(user_id, range_key, today)?;
        let mut rows = self.list_products(user_id, since, until, "")?;
        match sort {
            "unit_price" => {
                rows.retain(|p| p.typical_unit_aed.is_some());
                rows.sort_by(|a, b| b.typical_unit_aed.partial_cmp(&a.typical_unit_aed).unwrap_or(std::cmp::Ordering::Equal));
            }
            "frequency" => rows.sort_by(|a, b| freq_score(b).partial_cmp(&freq_score(a)).unwrap_or(std::cmp::Ordering::Equal)),
            _ => rows.sort_by(|a, b| b.spend_total.partial_cmp(&a.spend_total).unwrap_or(std::cmp::Ordering::Equal)),
        }
        rows.truncate(limit.clamp(1, 50));
        Ok(rows)
    }

    pub fn find_products(&self, user_id: i64, query: &str, today: NaiveDate, limit: usize) -> Result<Vec<ProductRow>> {
        let q = query.split_whitespace().collect::<Vec<_>>().join(" ").to_ascii_lowercase();
        if q.is_empty() {
            return Ok(Vec::new());
        }
        let mut hits = self.list_products(user_id, None, today, "")?;
        hits.retain(|p| {
            let blob = format!("{} {} {} {}", p.name, p.receipt_name, p.barcode, p.key).to_ascii_lowercase();
            blob.contains(&q)
        });
        hits.sort_by(|a, b| {
            let ascore = if a.name.to_ascii_lowercase() == q { 0 } else { 1 };
            let bscore = if b.name.to_ascii_lowercase() == q { 0 } else { 1 };
            ascore.cmp(&bscore).then(b.times_bought.cmp(&a.times_bought))
        });
        hits.truncate(limit.clamp(1, 20));
        Ok(hits)
    }

    pub fn list_invoices(&self, user_id: i64, limit: i64, offset: i64) -> Result<Vec<serde_json::Value>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            "SELECT i.id, i.retailer, i.invoice_no, i.invoice_date, i.store_name,
                    COALESCE(SUM(it.line_total), 0), COUNT(it.id)
             FROM invoices i
             LEFT JOIN invoice_items it ON it.invoice_id = i.id
             WHERE i.user_id=?1
             GROUP BY i.id
             ORDER BY i.invoice_date DESC, i.id DESC
             LIMIT ?2 OFFSET ?3",
        )?;
        let rows = stmt.query_map(rusqlite::params![user_id, limit, offset], |r| {
            Ok(serde_json::json!({
                "id": r.get::<_, i64>(0)?,
                "retailer": r.get::<_, String>(1)?,
                "invoice_no": r.get::<_, String>(2)?,
                "invoice_date": r.get::<_, String>(3).unwrap_or_default(),
                "store_name": r.get::<_, String>(4).unwrap_or_default(),
                "total": r.get::<_, f64>(5).unwrap_or(0.0),
                "lines": r.get::<_, i64>(6).unwrap_or(0),
            }))
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
    }
}

fn freq_score(p: &ProductRow) -> f64 {
    let times = p.times_bought as f64;
    let interval = p.interval_days;
    if times < 1.0 || interval <= 0.0 || interval >= 1e8 {
        0.0
    } else {
        times / interval
    }
}

fn cadence_status(last: &str, interval: f64, times: i64, today: NaiveDate) -> (String, Option<i64>, String) {
    let Some(last_day) = parse_day(last) else {
        return (String::new(), None, "unknown".into());
    };
    if times < 2 || interval <= 0.0 || interval >= 1e8 {
        return (String::new(), None, "unknown".into());
    }
    let nxt = last_day + Duration::days(interval.round().max(1.0) as i64);
    let due = (nxt - today).num_days();
    let age = (today - last_day).num_days();
    let status = if age > 45.max((1.5 * interval) as i64) {
        "lapsed"
    } else if due < 0 {
        "overdue"
    } else if due == 0 {
        "due_today"
    } else if due == 1 {
        "due_tomorrow"
    } else {
        "upcoming"
    };
    (nxt.format("%Y-%m-%d").to_string(), Some(due), status.into())
}

fn add_named(bucket: &mut Vec<(String, f64)>, name: &str, amount: f64) {
    if let Some(slot) = bucket.iter_mut().find(|(n, _)| n == name) {
        slot.1 += amount;
    } else {
        bucket.push((name.to_string(), amount));
    }
}

fn default_grain(range_key: &str) -> String {
    match range_key {
        "1w" | "2w" => "daily".into(),
        "1m" | "3m" | "this_month" | "last_month" => "weekly".into(),
        "1y" | "2y" | "3y" => "monthly".into(),
        _ => "monthly".into(),
    }
}

fn period_buckets(since: NaiveDate, until: NaiveDate, grain: &str) -> Vec<(NaiveDate, NaiveDate, String)> {
    let mut out = Vec::new();
    match grain {
        "daily" => {
            let mut d = since;
            while d <= until {
                out.push((d, d, d.format("%d %b").to_string()));
                d += Duration::days(1);
            }
        }
        "yearly" => {
            let mut year = since.year();
            while year <= until.year() {
                let start = NaiveDate::from_ymd_opt(year, 1, 1).unwrap_or(since).max(since);
                let stop = NaiveDate::from_ymd_opt(year, 12, 31).unwrap_or(until).min(until);
                if start <= stop {
                    out.push((start, stop, year.to_string()));
                }
                year += 1;
            }
        }
        "weekly" => {
            let mut start = since;
            while start <= until {
                let weekday = start.weekday().num_days_from_monday();
                let week_end = (start + Duration::days((6 - weekday) as i64)).min(until);
                let label = start.format("%d %b").to_string();
                out.push((start, week_end, label));
                start = week_end + Duration::days(1);
            }
        }
        _ => {
            let mut cursor = since;
            while cursor <= until {
                let next_month = if cursor.month() == 12 {
                    NaiveDate::from_ymd_opt(cursor.year() + 1, 1, 1)
                } else {
                    NaiveDate::from_ymd_opt(cursor.year(), cursor.month() + 1, 1)
                };
                let stop = next_month
                    .map(|d| d - Duration::days(1))
                    .unwrap_or(until)
                    .min(until);
                out.push((cursor, stop, cursor.format("%b %Y").to_string()));
                cursor = stop + Duration::days(1);
            }
        }
    }
    out
}

pub fn line(name: &str, qty: f64, unit: f64, barcode: &str) -> LineItem {
    LineItem {
        name: name.into(),
        qty,
        unit_price: Some(unit),
        line_total: round2(qty * unit),
        barcode: barcode.into(),
        image_url: String::new(),
    }
}

pub fn invoice(store: &str, no: &str, day: &str, items: Vec<LineItem>) -> ParsedInvoice {
    ParsedInvoice {
        retailer: store.into(),
        store_name: store.into(),
        invoice_no: no.into(),
        order_no: String::new(),
        invoice_date: day.into(),
        source: "test".into(),
        items,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Db {
        let dir = std::env::temp_dir().join(format!("omabring-test-{}", crate::crypto::random_token(6)));
        Db::open(dir).unwrap()
    }

    #[test]
    fn last_month_excludes_this_month() {
        let db = db();
        let user = db.create_user("spend@example.com", "secret1").unwrap();
        db.upsert_invoice(user.id, &invoice("carrefour", "A", "2026-09-01", vec![line("Milk", 1.0, 10.0, "m")]))
            .unwrap();
        db.upsert_invoice(user.id, &invoice("carrefour", "B", "2026-10-01", vec![line("Bread", 1.0, 4.0, "b")]))
            .unwrap();
        let end = NaiveDate::from_ymd_opt(2026, 10, 16).unwrap();
        let last = db.spend(user.id, "last_month", end, "monthly").unwrap();
        assert_eq!(last.total, 10.0);
        assert_eq!(last.period_count, 1);
        let rolling = db.spend(user.id, "1m", end, "daily").unwrap();
        assert_eq!(rolling.total, 4.0);
        assert_eq!(rolling.period_count, 30);
        assert!((rolling.average - 4.0 / 30.0).abs() < 0.02);
    }

    #[test]
    fn typical_price_drops_piece_versus_kg() {
        let db = db();
        let user = db.create_user("price@example.com", "secret1").unwrap();
        for (no, day, price) in [("P1", "2026-08-01", 0.30), ("P2", "2026-08-02", 0.32), ("P3", "2026-08-03", 0.28)] {
            db.upsert_invoice(user.id, &invoice("carrefour", no, day, vec![line("Garlic", 1.0, price, "g")]))
                .unwrap();
        }
        db.upsert_invoice(user.id, &invoice("carrefour", "P4", "2026-08-04", vec![line("Garlic", 1.0, 8.61, "g")]))
            .unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 8, 24).unwrap();
        let rows = db.list_products(user.id, None, today, "").unwrap();
        let garlic = rows.iter().find(|p| p.name == "Garlic").unwrap();
        assert!(garlic.typical_unit_aed.unwrap() < 1.0);
        assert_eq!(garlic.times_bought, 4);
    }

    #[test]
    fn delete_is_scoped_to_the_owner() {
        let db = db();
        let a = db.create_user("a@example.com", "secret1").unwrap();
        let b = db.create_user("b@example.com", "secret1").unwrap();
        let id = db
            .upsert_invoice(a.id, &invoice("carrefour", "X1", "2026-08-01", vec![line("Tea", 1.0, 5.0, "")]))
            .unwrap()
            .unwrap();
        assert!(db.delete_invoice(b.id, Some(id), None, None).unwrap().is_none());
        assert!(db.delete_invoice(a.id, Some(id), Some("carrefour"), Some("X1")).is_err());
        let gone = db.delete_invoice(a.id, None, Some("carrefour"), Some("X1")).unwrap().unwrap();
        assert_eq!(gone["items_removed"], 1);
    }

    #[test]
    fn sparkling_500ml_stays_on_the_list_with_a_low_score() {
        let db = db();
        let user = db.create_user("spark@example.com", "secret1").unwrap();
        let item = line("S.Pellegrino Sparkling Natural Mineral Water, 500ml", 1.0, 3.5, "8002270015223");
        for (i, day) in ["2026-05-01", "2026-05-20", "2026-06-10", "2026-07-01", "2026-07-25"].iter().enumerate() {
            db.upsert_invoice(user.id, &invoice("carrefour", &format!("S{i}"), day, vec![item.clone()])).unwrap();
        }
        let today = NaiveDate::from_ymd_opt(2026, 8, 24).unwrap();
        let rows = db.forecast(user.id, today, 7, "", true, &[]).unwrap();
        let pel = rows.iter().find(|p| p.name.to_ascii_lowercase().contains("pellegrino")).unwrap();
        assert!(!pel.include);
        assert_eq!(pel.reason, "occasional_small_pack");
        let on_list = db.shopping_list(user.id, today, 30, 20, "").unwrap();
        let hit = on_list.iter().find(|p| p.name.to_ascii_lowercase().contains("pellegrino")).unwrap();
        assert!(hit.likely < 25);
        assert_eq!(hit.likely_reason, "occasional_small_pack");
    }

    #[test]
    fn thumbs_move_a_staple() {
        let db = db();
        let user = db.create_user("thumb@example.com", "secret1").unwrap();
        let bread = line("White Bread", 1.0, 4.0, "b1");
        for (i, day) in ["2026-07-20", "2026-07-27", "2026-08-03", "2026-08-10", "2026-08-17"].iter().enumerate() {
            db.upsert_invoice(user.id, &invoice("carrefour", &format!("BR{i}"), day, vec![bread.clone()])).unwrap();
        }
        let today = NaiveDate::from_ymd_opt(2026, 8, 24).unwrap();
        let before = db.forecast(user.id, today, 7, "", true, &[]).unwrap();
        let row = before.iter().find(|p| p.name == "White Bread").unwrap();
        assert!(row.include);
        assert!(row.score >= 40);
        assert!(db.shopping_list(user.id, today, 7, 20, "").unwrap().iter().any(|p| p.name == "White Bread"));
        db.set_vote(user.id, &row.key, "down").unwrap();
        let after = db.forecast(user.id, today, 7, "", false, &[]).unwrap();
        assert!(after.iter().all(|p| p.name != "White Bread"));
        assert!(!db.shopping_list(user.id, today, 7, 20, "").unwrap().iter().any(|p| p.name == "White Bread"));
    }

    #[test]
    fn slug_roundtrip_unused() {
        assert!(!crate::catalog::slug_name("White Bread").is_empty());
    }
}

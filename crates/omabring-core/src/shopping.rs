use chrono::{Datelike, NaiveDate};
use rusqlite::{params, OptionalExtension};

use crate::crypto::{open, seal};
use crate::error::{Error, Result};
use crate::model::{
    normalize_name, round_money, BackendErrorKind, Cart, CartLine, CheckoutRecord, Comparison,
    ForecastItem, ForecastStatus, GroupSpend, InvoiceLine, InvoiceSummary, LiveState, MoneyTotal,
    Offer, OfferSource, PriceGroup, SpendRange, SpendReport, StoreBackend, StoreId, StoreLink,
    StoreNote, StoreSecrets,
};
use crate::{now_rfc3339, App};

impl App {
    pub fn list_store_links(&self) -> Result<Vec<StoreLink>> {
        let mut links = Vec::new();
        for store in StoreId::all() {
            let secrets = self.secrets_for(store)?;
            links.push(StoreLink {
                id: store,
                name: store.name(),
                homepage: store.homepage(),
                linked: secrets.is_some(),
                username: secrets.as_ref().map(|s| s.username.clone()),
                base_url: secrets.and_then(|s| s.base_url),
                remote_checkout: store.remote_checkout(),
            });
        }
        Ok(links)
    }

    pub fn link_store(&self, store: StoreId, secrets: &StoreSecrets) -> Result<()> {
        if secrets.username.trim().is_empty() || secrets.password.is_empty() {
            return Err(Error::Message(
                "username and password are required to link a store".into(),
            ));
        }
        let mut cleaned = secrets.clone();
        cleaned.username = cleaned.username.trim().to_string();
        if let Some(url) = cleaned.base_url.as_mut() {
            *url = url.trim().trim_end_matches('/').to_string();
            if url.is_empty() {
                cleaned.base_url = None;
            }
        }
        let plaintext =
            serde_json::to_vec(&cleaned).map_err(|err| Error::Message(err.to_string()))?;
        let blob = seal(&self.machine_key, &plaintext)?;
        let updated = now_rfc3339();
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO store_accounts (store, ciphertext, updated_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(store) DO UPDATE SET ciphertext = excluded.ciphertext, updated_at = excluded.updated_at",
                params![store.as_str(), blob, updated],
            )?;
            Ok(())
        })
    }

    pub fn unlink_store(&self, store: StoreId) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "DELETE FROM store_accounts WHERE store = ?1",
                params![store.as_str()],
            )?;
            Ok(())
        })
    }

    pub fn secrets_for(&self, store: StoreId) -> Result<Option<StoreSecrets>> {
        let blob = self.with_conn(|conn| {
            conn.query_row(
                "SELECT ciphertext FROM store_accounts WHERE store = ?1",
                params![store.as_str()],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .optional()
            .map_err(Error::from)
        })?;
        let Some(blob) = blob else {
            return Ok(None);
        };
        let plaintext = open(&self.machine_key, &blob)?;
        let secrets = serde_json::from_slice(&plaintext).map_err(|_| Error::Crypto)?;
        Ok(Some(secrets))
    }

    pub fn import_document(&self, document: &str) -> Result<Vec<InvoiceSummary>> {
        if document.len() > 2_000_000 {
            return Err(Error::InvalidInvoice("document is too large".into()));
        }
        let text = document.trim().trim_start_matches('\u{feff}');
        if text.is_empty() {
            return Err(Error::InvalidInvoice("document is empty".into()));
        }
        let invoices = if text.starts_with('{') || text.starts_with('[') {
            parse_json_invoices(text)?
        } else {
            parse_csv_invoices(text)?
        };
        if invoices.is_empty() {
            return Err(Error::InvalidInvoice("no receipts found".into()));
        }
        let mut saved = Vec::new();
        for invoice in invoices {
            saved.push(self.upsert_invoice(invoice)?);
        }
        Ok(saved)
    }

    pub fn list_invoices(&self) -> Result<Vec<InvoiceSummary>> {
        self.with_conn(|conn| load_invoices(conn, None))
    }

    pub fn delete_invoice(&self, invoice_id: i64) -> Result<()> {
        self.with_conn(|conn| {
            let changed =
                conn.execute("DELETE FROM invoices WHERE id = ?1", params![invoice_id])?;
            if changed == 0 {
                return Err(Error::Message("receipt not found".into()));
            }
            Ok(())
        })
    }

    pub fn spend(&self, range: SpendRange, today: NaiveDate) -> Result<SpendReport> {
        let (start, end) = range_bounds(range, today);
        let invoices = self.list_invoices()?;
        let mut invoice_count = 0u32;
        let mut totals: Vec<MoneyTotal> = Vec::new();
        let mut by_store: Vec<GroupSpend> = Vec::new();
        let mut by_product: Vec<GroupSpend> = Vec::new();
        for invoice in invoices {
            let date = NaiveDate::parse_from_str(&invoice.purchased_on, "%Y-%m-%d")
                .map_err(|_| Error::InvalidInvoice(invoice.purchased_on.clone()))?;
            if date < start || date > end {
                continue;
            }
            invoice_count += 1;
            add_money(&mut totals, &invoice.currency, invoice.total);
            add_group(
                &mut by_store,
                invoice.store.name(),
                &invoice.currency,
                invoice.total,
                0.0,
            );
            for line in &invoice.lines {
                add_group(
                    &mut by_product,
                    &line.name,
                    &invoice.currency,
                    line.line_total,
                    line.qty,
                );
            }
        }
        totals.sort_by(|a, b| a.currency.cmp(&b.currency));
        by_store.sort_by(|a, b| {
            b.amount
                .partial_cmp(&a.amount)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        by_product.sort_by(|a, b| {
            b.amount
                .partial_cmp(&a.amount)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(SpendReport {
            range_label: range.label(),
            start: start.format("%Y-%m-%d").to_string(),
            end: end.format("%Y-%m-%d").to_string(),
            invoice_count,
            totals,
            by_store,
            by_product,
        })
    }

    pub fn forecast(&self, today: NaiveDate) -> Result<Vec<ForecastItem>> {
        let invoices = self.list_invoices()?;
        #[derive(Clone)]
        struct Sample {
            date: NaiveDate,
            price: f64,
            unit: String,
        }
        let mut groups: Vec<(String, StoreId, String, String, Vec<Sample>)> = Vec::new();
        for invoice in invoices {
            let date = NaiveDate::parse_from_str(&invoice.purchased_on, "%Y-%m-%d")
                .map_err(|_| Error::InvalidInvoice(invoice.purchased_on.clone()))?;
            for line in invoice.lines {
                let key = format!(
                    "{}|{}|{}",
                    invoice.store.as_str(),
                    invoice.currency,
                    normalize_name(&line.name)
                );
                if let Some(group) = groups.iter_mut().find(|g| g.0 == key) {
                    group.4.push(Sample {
                        date,
                        price: line.unit_price,
                        unit: line.unit,
                    });
                } else {
                    groups.push((
                        key,
                        invoice.store,
                        invoice.currency.clone(),
                        line.name,
                        vec![Sample {
                            date,
                            price: line.unit_price,
                            unit: line.unit,
                        }],
                    ));
                }
            }
        }
        let mut items = Vec::new();
        for (_key, store, currency, name, mut samples) in groups {
            samples.sort_by_key(|sample| sample.date);
            let mut dates = Vec::new();
            for sample in &samples {
                if dates.last() != Some(&sample.date) {
                    dates.push(sample.date);
                }
            }
            if dates.len() < 2 {
                continue;
            }
            let last = *dates.last().unwrap();
            let days_since = (today - last).num_days();
            if days_since > 90 {
                continue;
            }
            let mut intervals = Vec::new();
            for pair in dates.windows(2) {
                intervals.push((pair[1] - pair[0]).num_days() as f64);
            }
            let median_days = median(&intervals);
            if median_days <= 0.0 {
                continue;
            }
            let next = last + chrono::Duration::days(median_days.round() as i64);
            let days_until = (next - today).num_days();
            let status = if days_until < 0 {
                ForecastStatus::Overdue
            } else if days_until == 0 {
                ForecastStatus::DueToday
            } else if days_until <= 3 {
                ForecastStatus::DueSoon
            } else {
                ForecastStatus::Upcoming
            };
            let prices: Vec<f64> = samples.iter().map(|sample| sample.price).collect();
            let unit = samples
                .last()
                .map(|sample| sample.unit.clone())
                .unwrap_or_else(|| "piece".into());
            items.push(ForecastItem {
                name,
                store,
                currency,
                purchases: dates.len() as u32,
                median_interval_days: round_money(median_days),
                last_purchase: last.format("%Y-%m-%d").to_string(),
                next_purchase: next.format("%Y-%m-%d").to_string(),
                days_until,
                status,
                likely: likely_score(days_until, median_days, &intervals),
                typical_unit_price: round_money(median(&prices)),
                unit,
            });
        }
        items.sort_by_key(|item| item.days_until);
        Ok(items)
    }

    pub fn cart(&self, store: StoreId) -> Result<Cart> {
        self.with_conn(|conn| {
            Ok(Cart {
                store,
                lines: load_cart(conn, store)?,
            })
        })
    }

    pub fn carts(&self) -> Result<Vec<Cart>> {
        let mut carts = Vec::new();
        for store in StoreId::all() {
            carts.push(self.cart(store)?);
        }
        Ok(carts)
    }

    pub fn cart_add(
        &self,
        store: StoreId,
        product_id: &str,
        name: &str,
        qty: f64,
        unit: &str,
        unit_price: Option<f64>,
        currency: &str,
    ) -> Result<Cart> {
        if name.trim().is_empty() {
            return Err(Error::Message("product name is required".into()));
        }
        if qty <= 0.0 {
            return Err(Error::Message("quantity must be greater than zero".into()));
        }
        let currency = normalize_currency(currency)?;
        let line_id = uuid::Uuid::new_v4().to_string();
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO cart_lines (store, line_id, product_id, name, qty, unit, unit_price, currency) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    store.as_str(),
                    line_id,
                    if product_id.trim().is_empty() { name.trim() } else { product_id.trim() },
                    name.trim(),
                    qty,
                    if unit.trim().is_empty() { "piece" } else { unit.trim() },
                    unit_price,
                    currency
                ],
            )?;
            Ok(Cart {
                store,
                lines: load_cart(conn, store)?,
            })
        })
    }

    pub fn cart_set_qty(&self, store: StoreId, line_id: &str, qty: f64) -> Result<Cart> {
        if qty < 0.0 {
            return Err(Error::Message("quantity cannot be negative".into()));
        }
        self.with_conn(|conn| {
            if qty == 0.0 {
                conn.execute(
                    "DELETE FROM cart_lines WHERE store = ?1 AND line_id = ?2",
                    params![store.as_str(), line_id],
                )?;
            } else {
                let changed = conn.execute(
                    "UPDATE cart_lines SET qty = ?1 WHERE store = ?2 AND line_id = ?3",
                    params![qty, store.as_str(), line_id],
                )?;
                if changed == 0 {
                    return Err(Error::Message("cart line not found".into()));
                }
            }
            Ok(Cart {
                store,
                lines: load_cart(conn, store)?,
            })
        })
    }

    pub fn cart_remove(&self, store: StoreId, line_id: &str) -> Result<Cart> {
        self.cart_set_qty(store, line_id, 0.0)
    }

    pub fn cart_clear(&self, store: StoreId) -> Result<Cart> {
        self.with_conn(|conn| {
            conn.execute(
                "DELETE FROM cart_lines WHERE store = ?1",
                params![store.as_str()],
            )?;
            Ok(Cart {
                store,
                lines: Vec::new(),
            })
        })
    }

    pub fn search(&self, query: &str, backend: &dyn StoreBackend) -> Result<Comparison> {
        let query = query.trim();
        if query.is_empty() {
            return Err(Error::Message("enter a product to search".into()));
        }
        let mut notes = Vec::new();
        let mut jobs = Vec::new();
        for store in StoreId::all() {
            let secrets = self.secrets_for(store)?.unwrap_or_else(StoreSecrets::blank);
            let linked = !secrets.username.trim().is_empty();
            jobs.push((store, secrets, linked));
        }
        let mut offers = Vec::new();
        std::thread::scope(|scope| {
            let mut handles = Vec::new();
            for (store, secrets, linked) in &jobs {
                let linked = *linked;
                handles.push(
                    scope.spawn(move || {
                        (*store, linked, backend.search_store(*store, secrets, query))
                    }),
                );
            }
            for handle in handles {
                let (store, linked, result) = handle.join().unwrap_or_else(|_| {
                    (
                        StoreId::Carrefour,
                        false,
                        Err(crate::model::BackendError::unavailable("search stopped")),
                    )
                });
                match result {
                    Ok(found) => {
                        let count = found.len();
                        offers.extend(found);
                        notes.push(StoreNote {
                            store,
                            linked,
                            live: LiveState::Ok,
                            detail: None,
                            count,
                        });
                    }
                    Err(err) => notes.push(StoreNote {
                        store,
                        linked,
                        live: match err.kind {
                            BackendErrorKind::NotConfigured => LiveState::NotConfigured,
                            BackendErrorKind::Unsupported => LiveState::Unsupported,
                            BackendErrorKind::Unavailable => LiveState::Unavailable,
                            BackendErrorKind::BadResponse => LiveState::BadResponse,
                        },
                        detail: Some(err.detail),
                        count: 0,
                    }),
                }
            }
        });
        offers.extend(self.history_offers(query)?);
        // Stable by store so each catalog keeps the relevance order it ranked.
        offers.sort_by(|a, b| a.store.as_str().cmp(b.store.as_str()));
        let groups = group_offers(&offers);
        notes.sort_by_key(|note| note.store.as_str());
        Ok(Comparison {
            query: query.to_string(),
            offers,
            groups,
            notes,
        })
    }

    pub fn checkout(&self, store: StoreId, backend: &dyn StoreBackend) -> Result<CheckoutRecord> {
        let cart = self.cart(store)?;
        if cart.lines.is_empty() {
            return Err(Error::EmptyCart);
        }
        let mut url = Some(store.homepage().to_string());
        let mut note = if store.remote_checkout() {
            "Cart prepared on this machine. Payment stays on the store website.".to_string()
        } else {
            format!(
                "Local cart prepared. {} does not take a remote cart from OmaBring. Payment stays with the store.",
                store.name()
            )
        };
        if let Some(secrets) = self.secrets_for(store)? {
            let signed_in = secrets
                .token
                .as_deref()
                .map(str::trim)
                .filter(|token| !token.is_empty())
                .is_some();
            if signed_in || store.remote_checkout() {
                match backend.checkout_store(store, &secrets, &cart) {
                    Ok(remote) => {
                        if let Some(remote_url) = remote.url {
                            url = Some(remote_url);
                        }
                        note = match remote.reference {
                            Some(reference) => {
                                format!("The store accepted the cart ({reference}).")
                            }
                            None => "The store accepted the cart.".into(),
                        };
                    }
                    Err(err)
                        if err.kind == BackendErrorKind::Unsupported
                            || err.kind == BackendErrorKind::NotConfigured => {}
                    Err(err) => {
                        note = format!(
                            "Local cart prepared. The store did not accept it: {}.",
                            err.detail
                        );
                    }
                }
            }
        }
        let id = uuid::Uuid::new_v4().to_string();
        let created = now_rfc3339();
        let detail = serde_json::to_string(&cart.lines).unwrap_or_else(|_| "[]".into());
        let record = CheckoutRecord {
            id: id.clone(),
            store,
            status: "prepared".into(),
            url: url.clone(),
            note: note.clone(),
            created_at: created.clone(),
            lines: cart.lines,
        };
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO checkouts (id, store, status, url, note, detail, created_at) VALUES (?1, ?2, 'prepared', ?3, ?4, ?5, ?6)",
                params![id, store.as_str(), url, note, detail, created],
            )?;
            Ok(record)
        })
    }

    fn history_offers(&self, query: &str) -> Result<Vec<Offer>> {
        let needle = normalize_name(query);
        let invoices = self.list_invoices()?;
        let mut buckets: Vec<Offer> = Vec::new();
        for invoice in invoices {
            let currency = invoice.currency.clone();
            for line in invoice.lines {
                if !normalize_name(&line.name).contains(&needle) {
                    continue;
                }
                let key = format!(
                    "{}:{}:{}",
                    invoice.store.as_str(),
                    currency,
                    normalize_name(&line.name)
                );
                if let Some(existing) = buckets.iter_mut().find(|offer| {
                    offer.store == invoice.store
                        && offer.currency == currency
                        && normalize_name(&offer.name) == normalize_name(&line.name)
                        && offer.source == OfferSource::History
                }) {
                    existing.price = round_money((existing.price + line.unit_price) / 2.0);
                } else {
                    buckets.push(Offer {
                        store: invoice.store,
                        product_id: format!("history:{key}"),
                        name: line.name,
                        price: round_money(line.unit_price),
                        currency: currency.clone(),
                        unit: line.unit,
                        source: OfferSource::History,
                    });
                }
            }
        }
        Ok(buckets)
    }

    fn upsert_invoice(&self, draft: DraftInvoice) -> Result<InvoiceSummary> {
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO invoices (store, invoice_no, purchased_on, currency, total) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(store, invoice_no) DO UPDATE SET purchased_on = excluded.purchased_on, currency = excluded.currency, total = excluded.total",
                params![draft.store.as_str(), draft.invoice_no, draft.purchased_on, draft.currency, draft.total],
            )?;
            let id = conn.query_row(
                "SELECT id FROM invoices WHERE store = ?1 AND invoice_no = ?2",
                params![draft.store.as_str(), draft.invoice_no],
                |row| row.get::<_, i64>(0),
            )?;
            conn.execute("DELETE FROM invoice_lines WHERE invoice_id = ?1", params![id])?;
            for line in &draft.lines {
                conn.execute(
                    "INSERT INTO invoice_lines (invoice_id, name, name_key, qty, unit, unit_price, line_total) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![id, line.name, normalize_name(&line.name), line.qty, line.unit, line.unit_price, line.line_total],
                )?;
            }
            Ok(InvoiceSummary {
                id,
                store: draft.store,
                invoice_no: draft.invoice_no,
                purchased_on: draft.purchased_on,
                currency: draft.currency,
                total: draft.total,
                lines: draft.lines,
            })
        })
    }
}

struct DraftInvoice {
    store: StoreId,
    invoice_no: String,
    purchased_on: String,
    currency: String,
    total: f64,
    lines: Vec<InvoiceLine>,
}

fn load_cart(conn: &rusqlite::Connection, store: StoreId) -> Result<Vec<CartLine>> {
    let mut stmt = conn.prepare(
        "SELECT line_id, product_id, name, qty, unit, unit_price, currency FROM cart_lines WHERE store = ?1 ORDER BY name",
    )?;
    let rows = stmt.query_map(params![store.as_str()], |row| {
        Ok(CartLine {
            line_id: row.get(0)?,
            product_id: row.get(1)?,
            name: row.get(2)?,
            qty: row.get(3)?,
            unit: row.get(4)?,
            unit_price: row.get(5)?,
            currency: row.get(6)?,
        })
    })?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Error::from)
}

fn load_invoices(conn: &rusqlite::Connection, _limit: Option<i64>) -> Result<Vec<InvoiceSummary>> {
    let mut stmt = conn.prepare(
        "SELECT id, store, invoice_no, purchased_on, currency, total FROM invoices ORDER BY purchased_on DESC, id DESC",
    )?;
    let invoice_rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, f64>(5)?,
        ))
    })?;
    let headers = invoice_rows.collect::<std::result::Result<Vec<_>, _>>()?;
    drop(stmt);
    let mut invoices = Vec::new();
    for (id, store_raw, invoice_no, purchased_on, currency, total) in headers {
        let Some(store) = StoreId::parse(&store_raw) else {
            continue;
        };
        let mut lines_stmt = conn.prepare(
            "SELECT name, qty, unit, unit_price, line_total FROM invoice_lines WHERE invoice_id = ?1 ORDER BY id",
        )?;
        let lines = lines_stmt
            .query_map(params![id], |row| {
                Ok(InvoiceLine {
                    name: row.get(0)?,
                    qty: row.get(1)?,
                    unit: row.get(2)?,
                    unit_price: row.get(3)?,
                    line_total: row.get(4)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        invoices.push(InvoiceSummary {
            id,
            store,
            invoice_no,
            purchased_on,
            currency,
            total,
            lines,
        });
    }
    Ok(invoices)
}

fn parse_json_invoices(text: &str) -> Result<Vec<DraftInvoice>> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|err| Error::InvalidInvoice(err.to_string()))?;
    let list = if let Some(items) = value.get("invoices").and_then(|v| v.as_array()) {
        items.clone()
    } else if let Some(items) = value.as_array() {
        items.clone()
    } else {
        vec![value]
    };
    list.iter().map(draft_from_json).collect()
}

fn draft_from_json(value: &serde_json::Value) -> Result<DraftInvoice> {
    let store = value
        .get("store")
        .and_then(|v| v.as_str())
        .and_then(StoreId::parse)
        .ok_or_else(|| Error::InvalidInvoice("store is required".into()))?;
    let invoice_no = value
        .get("invoice_no")
        .or_else(|| value.get("invoice"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if invoice_no.is_empty() {
        return Err(Error::InvalidInvoice("invoice_no is required".into()));
    }
    let purchased_on = value
        .get("date")
        .or_else(|| value.get("purchased_on"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    NaiveDate::parse_from_str(&purchased_on, "%Y-%m-%d")
        .map_err(|_| Error::InvalidInvoice("date must be YYYY-MM-DD".into()))?;
    let currency = normalize_currency(
        value
            .get("currency")
            .and_then(|v| v.as_str())
            .unwrap_or("AED"),
    )?;
    let items = value
        .get("items")
        .or_else(|| value.get("lines"))
        .and_then(|v| v.as_array())
        .ok_or_else(|| Error::InvalidInvoice("items are required".into()))?;
    let mut lines = Vec::new();
    for item in items {
        lines.push(line_from_json(item)?);
    }
    if lines.is_empty() {
        return Err(Error::InvalidInvoice("items are required".into()));
    }
    let total = round_money(lines.iter().map(|line| line.line_total).sum());
    Ok(DraftInvoice {
        store,
        invoice_no,
        purchased_on,
        currency,
        total,
        lines,
    })
}

fn line_from_json(item: &serde_json::Value) -> Result<InvoiceLine> {
    let name = item
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if name.is_empty() {
        return Err(Error::InvalidInvoice("item name is required".into()));
    }
    let qty = item.get("qty").and_then(|v| v.as_f64()).unwrap_or(0.0);
    if qty <= 0.0 {
        return Err(Error::InvalidInvoice(format!("{name} has a bad quantity")));
    }
    let unit_price = item
        .get("unit_price")
        .or_else(|| item.get("price"))
        .and_then(|v| v.as_f64())
        .ok_or_else(|| Error::InvalidInvoice(format!("{name} needs a unit price")))?;
    if unit_price < 0.0 {
        return Err(Error::InvalidInvoice(format!("{name} has a bad price")));
    }
    let unit = item
        .get("unit")
        .and_then(|v| v.as_str())
        .unwrap_or("piece")
        .trim()
        .to_string();
    Ok(InvoiceLine {
        name,
        qty,
        unit: if unit.is_empty() {
            "piece".into()
        } else {
            unit
        },
        unit_price: round_money(unit_price),
        line_total: round_money(qty * unit_price),
    })
}

fn parse_csv_invoices(text: &str) -> Result<Vec<DraftInvoice>> {
    let mut rows = text.lines().filter(|line| !line.trim().is_empty());
    let header = rows.next().unwrap_or("");
    let columns: Vec<String> = split_csv(header)
        .into_iter()
        .map(|c| c.to_lowercase())
        .collect();
    let idx = |name: &str| columns.iter().position(|c| c == name);
    let store_i =
        idx("store").ok_or_else(|| Error::InvalidInvoice("csv needs a store column".into()))?;
    let no_i = idx("invoice_no")
        .ok_or_else(|| Error::InvalidInvoice("csv needs an invoice_no column".into()))?;
    let date_i =
        idx("date").ok_or_else(|| Error::InvalidInvoice("csv needs a date column".into()))?;
    let name_i =
        idx("name").ok_or_else(|| Error::InvalidInvoice("csv needs a name column".into()))?;
    let qty_i = idx("qty").ok_or_else(|| Error::InvalidInvoice("csv needs a qty column".into()))?;
    let price_i = idx("unit_price")
        .ok_or_else(|| Error::InvalidInvoice("csv needs a unit_price column".into()))?;
    let currency_i = idx("currency");
    let unit_i = idx("unit");
    let mut grouped: Vec<DraftInvoice> = Vec::new();
    for line in rows {
        let cells = split_csv(line);
        let get = |index: usize| cells.get(index).map(|s| s.trim()).unwrap_or("");
        let store = StoreId::parse(get(store_i))
            .ok_or_else(|| Error::InvalidInvoice(format!("unknown store {}", get(store_i))))?;
        let invoice_no = get(no_i).to_string();
        let purchased_on = get(date_i).to_string();
        NaiveDate::parse_from_str(&purchased_on, "%Y-%m-%d")
            .map_err(|_| Error::InvalidInvoice("date must be YYYY-MM-DD".into()))?;
        let currency = normalize_currency(if let Some(i) = currency_i {
            get(i)
        } else {
            "AED"
        })?;
        let name = get(name_i).to_string();
        if name.is_empty() || invoice_no.is_empty() {
            return Err(Error::InvalidInvoice(
                "csv row is missing name or invoice_no".into(),
            ));
        }
        let qty: f64 = get(qty_i)
            .parse()
            .map_err(|_| Error::InvalidInvoice(format!("{name} has a bad quantity")))?;
        let unit_price: f64 = get(price_i)
            .parse()
            .map_err(|_| Error::InvalidInvoice(format!("{name} has a bad price")))?;
        let unit = if let Some(i) = unit_i {
            let unit = get(i);
            if unit.is_empty() {
                "piece".into()
            } else {
                unit.to_string()
            }
        } else {
            "piece".into()
        };
        let item = InvoiceLine {
            name,
            qty,
            unit,
            unit_price: round_money(unit_price),
            line_total: round_money(qty * unit_price),
        };
        if let Some(existing) = grouped
            .iter_mut()
            .find(|draft| draft.store == store && draft.invoice_no == invoice_no)
        {
            existing.lines.push(item);
            existing.total = round_money(existing.lines.iter().map(|line| line.line_total).sum());
        } else {
            grouped.push(DraftInvoice {
                store,
                invoice_no,
                purchased_on,
                currency,
                total: item.line_total,
                lines: vec![item],
            });
        }
    }
    Ok(grouped)
}

fn split_csv(line: &str) -> Vec<String> {
    let mut cells = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for ch in line.chars() {
        match ch {
            '"' => quoted = !quoted,
            ',' if !quoted => {
                cells.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    cells.push(current.trim().to_string());
    cells
}

fn normalize_currency(raw: &str) -> Result<String> {
    let currency = raw.trim().to_uppercase();
    let currency = if currency.is_empty() {
        "AED".into()
    } else {
        currency
    };
    if currency.len() != 3 || !currency.chars().all(|c| c.is_ascii_alphabetic()) {
        return Err(Error::InvalidInvoice(format!("bad currency {currency}")));
    }
    Ok(currency)
}

fn range_bounds(range: SpendRange, today: NaiveDate) -> (NaiveDate, NaiveDate) {
    match range {
        SpendRange::ThisMonth => (today.with_day(1).unwrap_or(today), today),
        SpendRange::LastMonth => {
            let first_this = today.with_day(1).unwrap_or(today);
            let last_prev = first_this.pred_opt().unwrap_or(today);
            (last_prev.with_day(1).unwrap_or(last_prev), last_prev)
        }
        SpendRange::LastDays(days) => {
            let span = days.max(1) - 1;
            (today - chrono::Duration::days(span), today)
        }
    }
}

fn add_money(totals: &mut Vec<MoneyTotal>, currency: &str, amount: f64) {
    if let Some(total) = totals.iter_mut().find(|t| t.currency == currency) {
        total.amount = round_money(total.amount + amount);
    } else {
        totals.push(MoneyTotal {
            currency: currency.to_string(),
            amount: round_money(amount),
        });
    }
}

fn add_group(groups: &mut Vec<GroupSpend>, key: &str, currency: &str, amount: f64, qty: f64) {
    if let Some(group) = groups
        .iter_mut()
        .find(|g| g.key == key && g.currency == currency)
    {
        group.amount = round_money(group.amount + amount);
        group.qty += qty;
    } else {
        groups.push(GroupSpend {
            key: key.to_string(),
            currency: currency.to_string(),
            amount: round_money(amount),
            qty,
        });
    }
}

fn median(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = sorted.len() / 2;
    if sorted.len() % 2 == 0 {
        (sorted[mid - 1] + sorted[mid]) / 2.0
    } else {
        sorted[mid]
    }
}

fn likely_score(days_until: i64, median_days: f64, intervals: &[f64]) -> u8 {
    let mean = intervals.iter().sum::<f64>() / intervals.len() as f64;
    let variance = intervals
        .iter()
        .map(|value| (value - mean).powi(2))
        .sum::<f64>()
        / intervals.len() as f64;
    let cv = if mean == 0.0 {
        0.0
    } else {
        (variance.sqrt() / mean).min(1.0)
    };
    let regularity = 1.0 - cv;
    let proximity = if days_until <= 0 {
        1.0
    } else if median_days <= 0.0 {
        0.0
    } else {
        (1.0 - (days_until as f64 / median_days).min(1.0)).max(0.0)
    };
    ((0.45 * regularity + 0.55 * proximity) * 100.0)
        .round()
        .clamp(0.0, 100.0) as u8
}

fn group_offers(offers: &[Offer]) -> Vec<PriceGroup> {
    let mut groups: Vec<PriceGroup> = Vec::new();
    for offer in offers {
        let key_name = normalize_name(&offer.name);
        if let Some(group) = groups.iter_mut().find(|group| {
            normalize_name(&group.name) == key_name && group.currency == offer.currency
        }) {
            group.offers.push(offer.clone());
        } else {
            groups.push(PriceGroup {
                name: offer.name.clone(),
                currency: offer.currency.clone(),
                cheapest_store: offer.store,
                cheapest_price: offer.price,
                offers: vec![offer.clone()],
            });
        }
    }
    for group in &mut groups {
        group.offers.sort_by(|a, b| {
            a.price
                .partial_cmp(&b.price)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        if let Some(cheapest) = group.offers.first() {
            group.cheapest_store = cheapest.store;
            group.cheapest_price = cheapest.price;
            group.name = cheapest.name.clone();
        }
    }
    groups.sort_by(|a, b| a.name.cmp(&b.name));
    groups
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::BackendError;
    use crate::App;

    struct NoLive;

    impl StoreBackend for NoLive {
        fn search_store(
            &self,
            _store: StoreId,
            _secrets: &StoreSecrets,
            _query: &str,
        ) -> std::result::Result<Vec<Offer>, BackendError> {
            Err(BackendError::not_configured())
        }

        fn checkout_store(
            &self,
            _store: StoreId,
            _secrets: &StoreSecrets,
            _cart: &Cart,
        ) -> std::result::Result<crate::model::RemoteCheckout, BackendError> {
            Err(BackendError::unsupported("not used"))
        }
    }

    struct Scripted {
        offers: Vec<Offer>,
    }

    impl StoreBackend for Scripted {
        fn search_store(
            &self,
            store: StoreId,
            _secrets: &StoreSecrets,
            _query: &str,
        ) -> std::result::Result<Vec<Offer>, BackendError> {
            Ok(self
                .offers
                .iter()
                .filter(|offer| offer.store == store)
                .cloned()
                .collect())
        }

        fn checkout_store(
            &self,
            _store: StoreId,
            _secrets: &StoreSecrets,
            _cart: &Cart,
        ) -> std::result::Result<crate::model::RemoteCheckout, BackendError> {
            Ok(crate::model::RemoteCheckout {
                reference: Some("ref-1".into()),
                url: Some("https://shop.example/checkout".into()),
            })
        }
    }

    fn opened() -> (tempfile::TempDir, App) {
        let dir = tempfile::tempdir().unwrap();
        let app = App::open(dir.path()).unwrap();
        (dir, app)
    }

    #[test]
    fn store_secrets_are_encrypted_at_rest() {
        let (_dir, app) = opened();
        app.link_store(
            StoreId::Carrefour,
            &StoreSecrets {
                username: "shopper".into(),
                password: "s3cret-value".into(),
                base_url: None,
                token: Some("session-token".into()),
                user_id: None,
            },
        )
        .unwrap();
        let loaded = app.secrets_for(StoreId::Carrefour).unwrap().unwrap();
        assert_eq!(loaded.password, "s3cret-value");
        let raw = std::fs::read(app.data_dir().join("omabring.db")).unwrap();
        let text = String::from_utf8_lossy(&raw);
        assert!(!text.contains("s3cret-value"));
        app.unlink_store(StoreId::Carrefour).unwrap();
        assert!(app.secrets_for(StoreId::Carrefour).unwrap().is_none());
    }

    #[test]
    fn import_spend_and_forecast() {
        let (_dir, app) = opened();
        let doc = r#"{
            "invoices": [
                {"store": "carrefour", "invoice_no": "A", "date": "2026-01-01", "currency": "AED", "items": [{"name": "Al Ain Water", "qty": 2, "unit": "piece", "unit_price": 1.5}]},
                {"store": "carrefour", "invoice_no": "B", "date": "2026-01-11", "currency": "AED", "items": [{"name": "Al Ain Water", "qty": 1, "unit": "piece", "unit_price": 1.75}]},
                {"store": "spinneys", "invoice_no": "C", "date": "2026-01-21", "currency": "AED", "items": [{"name": "Al Ain Water", "qty": 1, "unit": "piece", "unit_price": 2.0}, {"name": "Milk", "qty": 1, "unit": "piece", "unit_price": 5}]}
            ]
        }"#;
        let saved = app.import_document(doc).unwrap();
        assert_eq!(saved.len(), 3);
        let again = app.import_document(&saved_one()).unwrap();
        assert_eq!(again[0].id, saved[0].id);
        let today = NaiveDate::from_ymd_opt(2026, 1, 21).unwrap();
        let report = app.spend(SpendRange::LastDays(30), today).unwrap();
        assert_eq!(report.invoice_count, 3);
        assert_eq!(
            report.totals[0].amount,
            round_money(1.5 * 2.0 + 1.75 + 2.0 + 5.0)
        );
        let last = app.spend(SpendRange::LastMonth, today).unwrap();
        assert_eq!(last.invoice_count, 0);
        let forecast = app.forecast(today).unwrap();
        assert!(forecast.iter().all(|item| item.name != "Milk"));
        let water = forecast
            .iter()
            .find(|item| item.store == StoreId::Carrefour)
            .unwrap();
        assert_eq!(water.median_interval_days, 10.0);
        assert_eq!(water.last_purchase, "2026-01-11");
        assert_eq!(water.next_purchase, "2026-01-21");
        assert_eq!(water.days_until, 0);
        assert_eq!(water.likely, 100);
        assert_eq!(water.status, ForecastStatus::DueToday);
        let overdue = app
            .forecast(NaiveDate::from_ymd_opt(2026, 2, 5).unwrap())
            .unwrap();
        let water = overdue
            .iter()
            .find(|item| item.store == StoreId::Carrefour)
            .unwrap();
        assert_eq!(water.status, ForecastStatus::Overdue);
        assert_eq!(water.likely, 100);
        let lapsed = app
            .forecast(NaiveDate::from_ymd_opt(2026, 6, 1).unwrap())
            .unwrap();
        assert!(lapsed.is_empty());
    }

    fn saved_one() -> String {
        r#"{"store":"carrefour","invoice_no":"A","date":"2026-01-01","currency":"AED","items":[{"name":"Al Ain Water","qty":2,"unit":"piece","unit_price":1.5}]}"#.into()
    }

    #[test]
    fn csv_import_cart_and_compare() {
        let (_dir, app) = opened();
        let csv = "store,invoice_no,date,currency,name,qty,unit,unit_price\n\
union coop,U-1,2026-03-02,AED,Oat Milk,2,piece,8.5\n\
waitrose,W-1,2026-03-02,AED,Oat Milk,1,piece,11\n";
        app.import_document(csv).unwrap();
        app.link_store(
            StoreId::Spinneys,
            &StoreSecrets {
                username: "s".into(),
                password: "password1".into(),
                base_url: None,
                token: Some("t".into()),
                user_id: None,
            },
        )
        .unwrap();
        let backend = Scripted {
            offers: vec![Offer {
                store: StoreId::Spinneys,
                product_id: "oat".into(),
                name: "Oat Milk".into(),
                price: 7.25,
                currency: "AED".into(),
                unit: "piece".into(),
                source: OfferSource::Live,
            }],
        };
        let comparison = app.search("oat milk", &backend).unwrap();
        let group = comparison
            .groups
            .iter()
            .find(|g| normalize_name(&g.name) == "oat milk")
            .unwrap();
        assert_eq!(group.cheapest_store, StoreId::Spinneys);
        assert_eq!(group.cheapest_price, 7.25);
        assert!(group.offers.len() >= 3);
        let cart = app
            .cart_add(
                StoreId::Spinneys,
                "oat",
                "Oat Milk",
                2.0,
                "piece",
                Some(7.25),
                "AED",
            )
            .unwrap();
        assert_eq!(cart.lines.len(), 1);
        let line = cart.lines[0].line_id.clone();
        let cart = app.cart_set_qty(StoreId::Spinneys, &line, 3.0).unwrap();
        assert_eq!(cart.lines[0].qty, 3.0);
        let checkout = app.checkout(StoreId::Spinneys, &backend).unwrap();
        assert_eq!(checkout.status, "prepared");
        assert_eq!(
            checkout.url.as_deref(),
            Some("https://shop.example/checkout")
        );
        app.cart_clear(StoreId::Spinneys).unwrap();
        assert!(app.cart(StoreId::Spinneys).unwrap().lines.is_empty());
        assert!(matches!(
            app.checkout(StoreId::Spinneys, &NoLive).unwrap_err(),
            Error::EmptyCart
        ));
    }

    #[test]
    fn this_month_spend_window() {
        let (_dir, app) = opened();
        app.import_document(
            r#"{"store":"mmi","invoice_no":"1","date":"2026-02-02","currency":"AED","items":[{"name":"Tea","qty":1,"unit_price":10}]}"#,
        )
        .unwrap();
        app.import_document(
            r#"{"store":"mmi","invoice_no":"2","date":"2026-03-02","currency":"AED","items":[{"name":"Tea","qty":1,"unit_price":12}]}"#,
        )
        .unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 3, 16).unwrap();
        let report = app.spend(SpendRange::ThisMonth, today).unwrap();
        assert_eq!(report.invoice_count, 1);
        assert_eq!(report.totals[0].amount, 12.0);
        let prev = app.spend(SpendRange::LastMonth, today).unwrap();
        assert_eq!(prev.invoice_count, 1);
        assert_eq!(prev.totals[0].amount, 10.0);
    }
}

use serde::Serialize;
use serde_json::Value;

use crate::catalog::{digits_only, gtin_variants};
use crate::error::Result;
use crate::retailers::{self, retailer};
use crate::Db;

#[derive(Clone, Debug, Serialize)]
pub struct Quote {
    pub ok: bool,
    pub price: Option<f64>,
    pub found_name: String,
    pub sku: String,
    pub url: String,
    pub error: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct BoardRow {
    pub id: String,
    pub name: String,
    pub price: Option<f64>,
    pub found_name: String,
    pub fetched_at: String,
    pub error: String,
    pub url: String,
    pub vs_paid: Option<f64>,
    pub cheapest: bool,
    pub dearest: bool,
}

pub fn queries(barcodes: &[String], names: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for code in barcodes {
        let variants = gtin_variants(code);
        let variants = if variants.is_empty() && !code.is_empty() {
            vec![code.clone()]
        } else {
            variants
        };
        for v in variants {
            if !out.contains(&v) {
                out.push(v);
            }
        }
    }
    for name in names {
        let n = name.trim().to_string();
        if !n.is_empty() && !out.contains(&n) {
            out.push(n);
        }
    }
    out
}

fn money(raw: &Value) -> Option<f64> {
    if let Some(n) = raw.as_f64() {
        return Some(n);
    }
    raw.as_str().and_then(|s| s.replace(',', "").trim().parse().ok())
}

pub fn pick(results: &[Value], barcodes: &[String]) -> Option<Quote> {
    let digits: Vec<String> = barcodes.iter().map(|c| digits_only(c)).filter(|d| !d.is_empty()).collect();
    let mut ranked: Vec<(i32, Quote)> = Vec::new();
    for hit in results {
        let Some(price) = hit.get("price").and_then(money).filter(|p| *p > 0.0) else {
            continue;
        };
        let ean = digits_only(
            hit.get("ean")
                .or_else(|| hit.get("sku"))
                .or_else(|| hit.get("id"))
                .and_then(|v| v.as_str())
                .unwrap_or(""),
        );
        let mut score = 1;
        if !ean.is_empty()
            && digits.iter().any(|d| {
                ean == *d
                    || ean.starts_with(d)
                    || d.starts_with(if ean.len() >= 12 { &ean[..12] } else { ean.as_str() })
            })
        {
            score = 0;
        }
        ranked.push((
            score,
            Quote {
                ok: true,
                price: Some(price),
                found_name: hit.get("name").and_then(|v| v.as_str()).unwrap_or("").trim().to_string(),
                sku: hit
                    .get("sku")
                    .or_else(|| hit.get("ean"))
                    .or_else(|| hit.get("id"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                url: hit.get("url").and_then(|v| v.as_str()).unwrap_or("").trim().to_string(),
                error: String::new(),
            },
        ));
    }
    ranked.sort_by_key(|(score, _)| *score);
    ranked.into_iter().next().map(|(_, q)| q)
}

pub fn product_page(store_id: &str, raw: &str) -> String {
    let href = raw.trim();
    if href.is_empty() {
        return String::new();
    }
    let href = if let Some(rest) = href.strip_prefix("//") {
        format!("https://{rest}")
    } else if let Some(rest) = href.strip_prefix('/') {
        let base = retailer(store_id).map(|r| r.url).unwrap_or("");
        if base.is_empty() {
            return String::new();
        }
        format!("{}/{}", base.trim_end_matches('/'), rest)
    } else {
        href.to_string()
    };
    if href.starts_with("http://") || href.starts_with("https://") {
        href
    } else {
        String::new()
    }
}

impl Db {
    pub fn compare_board(&self, user_id: i64, product_key: &str, paid: Option<f64>) -> Result<Vec<BoardRow>> {
        let latest = self.latest_quotes(user_id, product_key)?;
        let prices: Vec<f64> = latest.iter().filter_map(|(_, p, _, _, _, _, _)| *p).filter(|p| *p > 0.0).collect();
        let lo = prices.iter().copied().reduce(f64::min);
        let hi = prices.iter().copied().reduce(f64::max);
        let mut board = Vec::new();
        for store in retailers::searchable() {
            let rec = latest.iter().find(|(id, _, _, _, _, _, _)| id == store.id);
            let price = rec.and_then(|(_, p, _, _, _, _, _)| *p).filter(|p| *p > 0.0);
            let vs = match (price, paid) {
                (Some(price), Some(paid)) if paid > 0.0 => Some(((price - paid) / paid * 1000.0).round() / 10.0),
                _ => None,
            };
            board.push(BoardRow {
                id: store.id.into(),
                name: store.name.into(),
                price,
                found_name: rec.map(|(_, _, n, _, _, _, _)| n.clone()).unwrap_or_default(),
                fetched_at: rec.map(|(_, _, _, _, _, at, _)| at.clone()).unwrap_or_default(),
                error: rec.map(|(_, _, _, _, e, _, _)| e.clone()).unwrap_or_default(),
                url: if price.is_some() {
                    product_page(store.id, rec.map(|(_, _, _, _, _, _, u)| u.as_str()).unwrap_or(""))
                } else {
                    String::new()
                },
                vs_paid: vs,
                cheapest: price.is_some() && lo.is_some() && (price.unwrap() - lo.unwrap()).abs() < 0.001,
                dearest: price.is_some()
                    && hi.is_some()
                    && lo != hi
                    && (price.unwrap() - hi.unwrap()).abs() < 0.001,
            });
        }
        Ok(board)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn barcode_match_beats_a_cheaper_name_hit() {
        let hits = vec![
            serde_json::json!({"name": "Other water", "price": 1.0, "sku": "111", "ean": "111"}),
            serde_json::json!({"name": "Pellegrino", "price": 3.5, "sku": "8002270015223", "ean": "8002270015223"}),
        ];
        let picked = pick(&hits, &["8002270015223".into()]).unwrap();
        assert_eq!(picked.sku, "8002270015223");
        assert!((picked.price.unwrap() - 3.5).abs() < 0.01);
    }
}

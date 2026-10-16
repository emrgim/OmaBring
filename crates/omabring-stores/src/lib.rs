//! Account-mode supermarket catalog, carts, and checkout.
//! Live store HTTP is shaped as requests. The default transport does not send them.

use omabring_core::cart_match::{match_cart_line, peel_remove_name};
use omabring_core::catalog::gtin_variants;
use omabring_core::retailers::{self, retailer};
use omabring_core::Db;
use serde_json::{json, Value};

#[derive(Clone, Copy, Debug)]
pub struct Item {
    pub retailer: &'static str,
    pub name: &'static str,
    pub sku: &'static str,
    pub ean: &'static str,
    pub price: f64,
    pub url: &'static str,
}

pub const CATALOG: &[Item] = &[
    Item { retailer: "carrefour", name: "White Bread", sku: "b1", ean: "6281000000018", price: 4.25, url: "/mafuae/en/white-bread" },
    Item { retailer: "grandiose", name: "White Bread", sku: "gb1", ean: "6281000000018", price: 4.50, url: "/white-bread" },
    Item { retailer: "unioncoop", name: "White Bread", sku: "ub1", ean: "6281000000018", price: 3.95, url: "/white-bread" },
    Item { retailer: "carrefour", name: "Almarai Full Fat Milk 2L", sku: "m1", ean: "6281007001234", price: 12.50, url: "/mafuae/en/almarai-milk" },
    Item { retailer: "grandiose", name: "Almarai Full Fat Milk 2L", sku: "gm1", ean: "6281007001234", price: 12.95, url: "/almarai-milk" },
    Item { retailer: "spinneys", name: "Almarai Full Fat Milk 2L", sku: "sm1", ean: "6281007001234", price: 13.25, url: "/almarai-milk" },
    Item { retailer: "waitrose", name: "Almarai Full Fat Milk 2L", sku: "wm1", ean: "6281007001234", price: 13.75, url: "/almarai-milk" },
    Item { retailer: "carrefour", name: "S.Pellegrino Sparkling Natural Mineral Water, 500ml", sku: "sp500", ean: "8002270015223", price: 3.75, url: "/mafuae/en/pellegrino-500" },
    Item { retailer: "grandiose", name: "S.Pellegrino Sparkling Natural Mineral Water, 500ml", sku: "gsp", ean: "8002270015223", price: 3.50, url: "/pellegrino-500" },
    Item { retailer: "mmi", name: "Heineken Can 330ml", sku: "hk", ean: "8712000031293", price: 8.50, url: "/heineken" },
    Item { retailer: "africaneastern", name: "Heineken Can 330ml", sku: "ahk", ean: "8712000031293", price: 7.95, url: "/heineken" },
    Item { retailer: "carrefour", name: "Green Tea 25 Bags", sku: "tea", ean: "6281000003330", price: 9.50, url: "/mafuae/en/green-tea" },
    Item { retailer: "unioncoop", name: "Green Tea 25 Bags", sku: "utea", ean: "6281000003330", price: 8.75, url: "/green-tea" },
    Item { retailer: "grandiose", name: "Garlic", sku: "gar", ean: "6281000004443", price: 0.35, url: "/garlic" },
    Item { retailer: "carrefour", name: "HMD Pulse Pro", sku: "hmd", ean: "6438409080121", price: 499.00, url: "/mafuae/en/hmd-pulse-pro" },
];

pub fn store_mode() -> String {
    match std::env::var("OMABRING_STORE_MODE").unwrap_or_default().as_str() {
        "live" => "live".into(),
        _ => "account".into(),
    }
}

pub fn search(retailer_id: &str, query: &str, limit: usize) -> Value {
    if !retailers::store_can_search(retailer_id) {
        return json!({"success": false, "error": "search is not available for this store", "results": []});
    }
    if store_mode() == "live" {
        let req = search_request(retailer_id, query);
        return json!({
            "success": false,
            "error": "live catalog is not dispatched from this process",
            "error_code": "live_unavailable",
            "request": req,
            "results": [],
        });
    }
    let q = query.trim().to_ascii_lowercase();
    let variants = gtin_variants(query);
    let mut hits = Vec::new();
    for item in CATALOG.iter().filter(|i| i.retailer == retailer_id) {
        let name = item.name.to_ascii_lowercase();
        let ean_hit = variants.iter().any(|v| v == item.ean || v == item.sku) || item.ean == query || item.sku == query;
        let name_hit = !q.is_empty() && name.contains(&q);
        if ean_hit || name_hit {
            hits.push(json!({
                "name": item.name,
                "sku": item.sku,
                "ean": item.ean,
                "id": item.ean,
                "price": item.price,
                "url": item.url,
            }));
        }
    }
    hits.truncate(limit.max(1));
    json!({"success": true, "retailer": retailer_id, "results": hits, "mode": "account"})
}

pub fn search_request(retailer_id: &str, query: &str) -> Value {
    match retailer_id {
        "grandiose" => json!({
            "method": "POST",
            "url": "https://www.grandiose.ae/graphql",
            "body": {"query": "search", "q": query},
        }),
        "unioncoop" => json!({
            "method": "GET",
            "url": format!("https://www.unioncoop.ae/rest/V1/products?searchCriteria[filter_groups][0][filters][0][value]={query}"),
        }),
        "carrefour" => json!({
            "method": "GET",
            "url": "https://api-prod.retailsso.com/v3/products/search",
            "headers": {"appid": "Android", "storeid": "mafuae", "lang": "en", "appversion": "26.8.21"},
            "query": query,
        }),
        "mmi" => json!({
            "method": "POST",
            "url": "https://api.mmiprod.retter.io/o9pki8qf",
            "body_encoding": "base64",
            "query": query,
        }),
        "africaneastern" => json!({
            "method": "POST",
            "url": "https://www.africaneasternonline.com/graphql",
            "operation": "customerTokenForLicenseDXB",
            "query": query,
        }),
        "waitrose" => json!({"method": "GET", "url": format!("https://www.waitrose.ae/en/search?q={query}")}),
        "spinneys" => json!({"method": "GET", "url": format!("https://www.spinneys.com/en-ae/search/?q={query}")}),
        _ => json!({"method": "GET", "query": query}),
    }
}

fn catalog_hit(retailer_id: &str, query: &str) -> Option<&'static Item> {
    let found = search(retailer_id, query, 5);
    let sku = found["results"].as_array()?.first()?.get("sku")?.as_str()?;
    CATALOG.iter().find(|i| i.retailer == retailer_id && i.sku == sku)
}

pub fn cart(db: &Db, user_id: i64, retailer_id: &str, action: &str, name: &str, product_id: &str, qty: f64, item_id: &str) -> Value {
    if !retailers::known(retailer_id) {
        return fail("unknown retailer");
    }
    if !retailers::store_can_shop(retailer_id) {
        return json!({"success": false, "error": "this store has no cart", "retailer": retailer_id});
    }
    if store_mode() == "live" {
        return json!({
            "success": false,
            "error": "live cart is not dispatched from this process",
            "error_code": "live_unavailable",
            "request": search_request(retailer_id, name),
        });
    }
    let action = action.trim().to_ascii_lowercase();
    let action = if matches!(action.as_str(), "get" | "read" | "show" | "view" | "") { "list".into() } else { action };
    let mut items = db.load_cart(user_id, retailer_id).unwrap_or_default();
    match action.as_str() {
        "list" => {
            let _ = db.record_probe(user_id, retailer_id, "cart", true, None);
            json!({"success": true, "retailer": retailer_id, "mode": "account", "items": items, "count": items.len()})
        }
        "clear" | "empty" | "new" => {
            let _ = db.save_cart(user_id, retailer_id, &[]);
            let _ = db.record_probe(user_id, retailer_id, "cart", true, None);
            json!({"success": true, "retailer": retailer_id, "items": [], "count": 0})
        }
        "add" | "set" => {
            let query = if !product_id.is_empty() { product_id } else { name };
            let Some(hit) = catalog_hit(retailer_id, query) else {
                return fail("product was not found in the account catalog");
            };
            let qty = if qty <= 0.0 { 1.0 } else { qty };
            if let Some(existing) = items.iter_mut().find(|it| it.get("sku").and_then(|v| v.as_str()) == Some(hit.sku)) {
                let next = if action == "set" { qty } else { existing.get("qty").and_then(|v| v.as_f64()).unwrap_or(0.0) + qty };
                existing["qty"] = json!(next);
            } else {
                let id = items.len() as i64 + 1;
                items.push(json!({
                    "item_id": id,
                    "id": hit.ean,
                    "sku": hit.sku,
                    "name": hit.name,
                    "qty": qty,
                    "price": hit.price,
                    "url": hit.url,
                }));
            }
            let _ = db.save_cart(user_id, retailer_id, &items);
            let _ = db.record_probe(user_id, retailer_id, "cart", true, None);
            json!({"success": true, "retailer": retailer_id, "mode": "account", "items": items, "count": items.len()})
        }
        "remove" => {
            let target = if name.trim().is_empty() { product_id } else { name };
            let query = peel_remove_name(target).unwrap_or_else(|| target.to_string());
            let Some(hit) = match_cart_line(&items, item_id, product_id, &query) else {
                return json!({"success": false, "error": "that line is still not in the cart", "items": items});
            };
            let hit_id = hit.get("item_id").cloned().unwrap_or(Value::Null);
            items.retain(|it| it.get("item_id") != Some(&hit_id));
            if items.iter().any(|it| it.get("item_id") == Some(&hit_id)) {
                return json!({"success": false, "error": "that line is still in the cart", "items": items});
            }
            let _ = db.save_cart(user_id, retailer_id, &items);
            json!({"success": true, "retailer": retailer_id, "items": items, "count": items.len()})
        }
        other => fail(&format!("unknown cart action {other}")),
    }
}

pub fn checkout(db: &Db, user_id: i64, retailer_id: &str, action: &str, payment_method: &str) -> Value {
    let Some(meta) = retailer(retailer_id) else {
        return fail("unknown retailer");
    };
    if !meta.checkout {
        return json!({
            "success": false,
            "error": "checkout stays on the store website",
            "checkout_url": meta.checkout_url,
            "retailer": retailer_id,
        });
    }
    if retailer_id == "unioncoop" && action == "place" {
        return json!({
            "success": false,
            "error": "Union Coop checkout prepares the basket only",
            "status": "prepared",
            "checkout_url": meta.checkout_url,
        });
    }
    let items = db.load_cart(user_id, retailer_id).unwrap_or_default();
    if items.is_empty() {
        return fail("cart is empty");
    }
    let address = db
        .retailer_secret(user_id, retailer_id)
        .ok()
        .flatten()
        .map(|s| s.address)
        .unwrap_or_default();
    let action = if action.is_empty() { "prepare" } else { action };
    if action == "place" {
        if retailer_id != "grandiose" {
            return json!({"success": false, "error": "this store does not place orders from OmaBring", "checkout_url": meta.checkout_url});
        }
        if payment_method != "ccod" && payment_method != "cashondelivery" {
            return fail("place accepts ccod or cashondelivery");
        }
        let id = db
            .create_order(user_id, retailer_id, &Value::Array(items.clone()), &address, "placed", meta.checkout_url)
            .unwrap_or(0);
        let _ = db.save_cart(user_id, retailer_id, &[]);
        let _ = db.record_probe(user_id, retailer_id, "checkout", true, None);
        return json!({
            "success": true,
            "order_id": id,
            "status": "placed",
            "payment_method": payment_method,
            "delivery_note": "Leave with security. Do not ring, call, or leave at the door.",
            "checkout_url": meta.checkout_url,
        });
    }
    let id = db
        .create_order(user_id, retailer_id, &Value::Array(items), &address, "prepared", meta.checkout_url)
        .unwrap_or(0);
    json!({
        "success": true,
        "order_id": id,
        "status": "prepared",
        "checkout_url": meta.checkout_url,
        "delivery_note": "Leave with security. Do not ring, call, or leave at the door.",
    })
}

fn fail(msg: &str) -> Value {
    json!({"success": false, "error": msg})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Db {
        let dir = std::env::temp_dir().join(format!("omabring-store-{}", omabring_core::crypto::random_token(6)));
        Db::open(dir).unwrap()
    }

    #[test]
    fn account_cart_add_and_remove_by_name() {
        let db = db();
        let user = db.create_user("shop@example.com", "secret1").unwrap();
        let added = cart(&db, user.id, "carrefour", "add", "white bread", "", 1.0, "");
        assert_eq!(added["success"], true);
        assert_eq!(added["count"], 1);
        let removed = cart(&db, user.id, "carrefour", "remove", "togli la White Bread", "", 0.0, "");
        assert_eq!(removed["success"], true);
        assert_eq!(removed["count"], 0);
    }

    #[test]
    fn grandiose_place_uses_cash_on_delivery() {
        let db = db();
        let user = db.create_user("order@example.com", "secret1").unwrap();
        cart(&db, user.id, "grandiose", "add", "", "gb1", 2.0, "");
        let prepared = checkout(&db, user.id, "grandiose", "prepare", "");
        assert_eq!(prepared["status"], "prepared");
        cart(&db, user.id, "grandiose", "add", "", "gb1", 1.0, "");
        let placed = checkout(&db, user.id, "grandiose", "place", "cashondelivery");
        assert_eq!(placed["status"], "placed");
        assert!(placed.get("card_number").is_none());
        let empty = cart(&db, user.id, "grandiose", "list", "", "", 0.0, "");
        assert_eq!(empty["count"], 0);
    }
}

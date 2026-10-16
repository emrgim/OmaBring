mod skill;
mod x;

use omabring_core::invoice::{self, LineItem, ParsedInvoice};
use omabring_core::retailers::RETAILERS;
use omabring_core::{Db, User};
use omabring_stores::{self, cart, checkout, search};
use serde_json::{json, Value};

pub use skill::SKILL;

const PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];
pub const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn tools() -> Vec<Value> {
    let mut out = vec![
        tool("omabring_whoami", "Signed-in OmaBring account", &[]),
        tool("omabring_stores", "Supermarkets and saved logins", &[]),
        tool("omabring_search", "Search one supermarket catalog", &[("retailer", "string"), ("query", "string")]),
        tool("omabring_compare", "Compare stored quotes", &[("product_key", "string"), ("paid", "number")]),
        tool("omabring_cart", "List or change a store cart", &[("retailer", "string"), ("action", "string"), ("name", "string"), ("product_id", "string"), ("qty", "number"), ("item_id", "string")]),
        tool("omabring_checkout", "Prepare or place a checkout", &[("retailer", "string"), ("action", "string"), ("payment_method", "string")]),
        tool("omabring_spend", "Spend for a date window", &[("range", "string"), ("grain", "string"), ("end", "string")]),
        tool("omabring_orders", "Imported invoices", &[("range", "string")]),
        tool("omabring_products", "Rank products", &[("sort", "string"), ("range", "string")]),
        tool("omabring_shopping_list", "What to buy next", &[("horizon_days", "number")]),
        tool("omabring_product", "One product from purchase history", &[("query", "string")]),
        tool("omabring_import_invoice", "Import one invoice", &[("retailer", "string"), ("invoice_no", "string"), ("text", "string")]),
        tool("omabring_delete_invoice", "Delete one invoice", &[("invoice_id", "number"), ("retailer", "string"), ("invoice_no", "string")]),
        tool("omabring_vote", "Thumb a product up or down", &[("product_key", "string"), ("vote", "string")]),
        tool("omabring_backup", "Copy the database to the configured backup path", &[]),
    ];
    for (name, desc) in [
        ("carrefour_search", "Search Carrefour"),
        ("grandiose_search", "Search Grandiose"),
        ("unioncoop_search", "Search Union Coop"),
        ("waitrose_search", "Search Waitrose"),
        ("spinneys_search", "Search Spinneys"),
        ("mmi_search", "Search MMI"),
        ("africaneastern_search", "Search African + Eastern"),
    ] {
        out.push(tool(name, desc, &[("query", "string")]));
    }
    out.push(tool("carrefour_cart", "Carrefour cart", &[("action", "string"), ("name", "string"), ("product_id", "string"), ("qty", "number"), ("item_id", "string")]));
    out.push(tool("grandiose_cart", "Grandiose cart", &[("action", "string"), ("name", "string"), ("product_id", "string"), ("qty", "number")]));
    out.push(tool("unioncoop_cart", "Union Coop cart", &[("action", "string"), ("name", "string"), ("product_id", "string"), ("qty", "number")]));
    out.push(tool("grandiose_checkout", "Grandiose checkout", &[("action", "string"), ("payment_method", "string")]));
    out.push(tool("unioncoop_checkout", "Union Coop checkout", &[("action", "string")]));
    for name in ["x_me", "x_user_by_username", "x_user_posts", "x_mentions", "x_search", "x_post"] {
        out.push(tool(name, "X request, signed and not sent", &[("username", "string"), ("query", "string"), ("text", "string")]));
    }
    out
}

fn tool(name: &str, description: &str, fields: &[(&str, &str)]) -> Value {
    let mut properties = serde_json::Map::new();
    for (key, kind) in fields {
        properties.insert((*key).into(), json!({"type": kind}));
    }
    json!({
        "name": name,
        "description": description,
        "inputSchema": {"type": "object", "properties": properties},
    })
}

pub fn handle(db: &Db, user: &User, body: &Value) -> Option<Value> {
    if body.as_array().is_some() {
        let mut replies = Vec::new();
        for item in body.as_array().unwrap() {
            if let Some(reply) = handle(db, user, item) {
                replies.push(reply);
            }
        }
        return if replies.is_empty() { None } else { Some(Value::Array(replies)) };
    }
    let method = body.get("method").and_then(|m| m.as_str()).unwrap_or("");
    if body.get("id").is_none() || method.starts_with("notifications/") {
        return None;
    }
    let id = body.get("id").cloned().unwrap_or(Value::Null);
    let params = body.get("params").cloned().unwrap_or(json!({}));
    let result = match method {
        "initialize" => {
            let requested = params.get("protocolVersion").and_then(|v| v.as_str()).unwrap_or(PROTOCOL_VERSIONS[0]);
            let version = if PROTOCOL_VERSIONS.contains(&requested) { requested } else { PROTOCOL_VERSIONS[0] };
            json!({
                "protocolVersion": version,
                "capabilities": {"tools": {"listChanged": false}, "resources": {}, "prompts": {}},
                "serverInfo": {"name": "OmaBring", "version": SERVER_VERSION},
            })
        }
        "ping" => json!({}),
        "tools/list" => json!({"tools": tools()}),
        "tools/call" => call_tool(db, user, params.get("name").and_then(|v| v.as_str()).unwrap_or(""), params.get("arguments").unwrap_or(&json!({}))),
        "resources/list" => json!({"resources": [{"uri": "omabring://skill", "name": "OmaBring skill", "mimeType": "text/markdown"}]}),
        "resources/read" => json!({"contents": [{"uri": "omabring://skill", "mimeType": "text/markdown", "text": SKILL}]}),
        "prompts/list" => json!({"prompts": [{"name": "omabring", "description": "How to use OmaBring"}]}),
        "prompts/get" => json!({"description": "OmaBring", "messages": [{"role": "user", "content": {"type": "text", "text": SKILL}}]}),
        _ => {
            return Some(json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "method not found"}}));
        }
    };
    Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
}

fn call_tool(db: &Db, user: &User, name: &str, args: &Value) -> Value {
    let payload = dispatch(db, user, name, args);
    let is_error = payload.get("success").and_then(|v| v.as_bool()) == Some(false);
    let text = serde_json::to_string(&payload).unwrap_or_else(|_| "{}".into());
    json!({"content": [{"type": "text", "text": text}], "isError": is_error})
}

fn dispatch(db: &Db, user: &User, name: &str, args: &Value) -> Value {
    let today = chrono_today(args);
    match name {
        "omabring_whoami" => json!({
            "success": true,
            "email": user.email,
            "server": "OmaBring",
            "version": SERVER_VERSION,
            "store_mode": omabring_stores::store_mode(),
        }),
        "omabring_stores" => match db.list_accounts(user.id) {
            Ok(rows) => json!({"success": true, "stores": rows}),
            Err(err) => fail(&err.to_string()),
        },
        "omabring_search" => search(arg(args, "retailer"), arg(args, "query"), num(args, "limit").unwrap_or(8.0) as usize),
        "carrefour_search" => search("carrefour", arg(args, "query"), 8),
        "grandiose_search" => search("grandiose", arg(args, "query"), 8),
        "unioncoop_search" => search("unioncoop", arg(args, "query"), 8),
        "waitrose_search" => search("waitrose", arg(args, "query"), 8),
        "spinneys_search" => search("spinneys", arg(args, "query"), 8),
        "mmi_search" => search("mmi", arg(args, "query"), 8),
        "africaneastern_search" => search("africaneastern", arg(args, "query"), 8),
        "omabring_cart" => cart(db, user.id, arg(args, "retailer"), arg(args, "action"), arg(args, "name"), arg(args, "product_id"), num(args, "qty").unwrap_or(1.0), arg(args, "item_id")),
        "carrefour_cart" => cart(db, user.id, "carrefour", arg(args, "action"), arg(args, "name"), arg(args, "product_id"), num(args, "qty").unwrap_or(1.0), arg(args, "item_id")),
        "grandiose_cart" => cart(db, user.id, "grandiose", arg(args, "action"), arg(args, "name"), arg(args, "product_id"), num(args, "qty").unwrap_or(1.0), arg(args, "item_id")),
        "unioncoop_cart" => cart(db, user.id, "unioncoop", arg(args, "action"), arg(args, "name"), arg(args, "product_id"), num(args, "qty").unwrap_or(1.0), arg(args, "item_id")),
        "omabring_checkout" => checkout(db, user.id, arg(args, "retailer"), arg(args, "action"), arg(args, "payment_method")),
        "grandiose_checkout" => checkout(db, user.id, "grandiose", arg(args, "action"), arg(args, "payment_method")),
        "unioncoop_checkout" => checkout(db, user.id, "unioncoop", arg(args, "action"), arg(args, "payment_method")),
        "omabring_compare" => compare(db, user, args),
        "omabring_spend" => spend(db, user, args, today),
        "omabring_orders" => orders(db, user, today),
        "omabring_products" => products(db, user, args, today),
        "omabring_shopping_list" => shopping(db, user, args, today),
        "omabring_product" => product(db, user, args, today),
        "omabring_import_invoice" => import_invoice(db, user, args),
        "omabring_delete_invoice" => delete_invoice(db, user, args),
        "omabring_vote" => match db.set_vote(user.id, arg(args, "product_key"), arg(args, "vote")) {
            Ok(push) => json!({"success": true, "push": push}),
            Err(err) => fail(&err.to_string()),
        },
        "omabring_backup" => match db.backup_configured() {
            Ok(path) => json!({"success": true, "path": path.display().to_string()}),
            Err(err) => fail(&err.to_string()),
        },
        "x_me" | "x_user_by_username" | "x_user_posts" | "x_mentions" | "x_search" | "x_post" => x::call(name, args),
        _ => fail("unknown tool"),
    }
}

fn compare(db: &Db, user: &User, args: &Value) -> Value {
    let key = arg(args, "product_key");
    let paid = num(args, "paid");
    match db.compare_board(user.id, key, paid) {
        Ok(board) => {
            if let Some(retailer_id) = args.get("refresh").and_then(|v| v.as_str()) {
                let query = arg(args, "query");
                let found = search(retailer_id, if query.is_empty() { key } else { query }, 5);
                if let Some(hit) = found["results"].as_array().and_then(|a| a.first()) {
                    let _ = db.record_quote(
                        user.id,
                        key,
                        retailer_id,
                        hit.get("price").and_then(|v| v.as_f64()),
                        hit.get("name").and_then(|v| v.as_str()).unwrap_or(""),
                        hit.get("sku").and_then(|v| v.as_str()).unwrap_or(""),
                        "account",
                        "",
                        hit.get("url").and_then(|v| v.as_str()).unwrap_or(""),
                    );
                }
            }
            json!({"success": true, "board": board, "stores": RETAILERS.len()})
        }
        Err(err) => fail(&err.to_string()),
    }
}

fn spend(db: &Db, user: &User, args: &Value, today: chrono::NaiveDate) -> Value {
    let end = omabring_core::purchases::parse_day(arg(args, "end")).unwrap_or(today);
    match db.spend(user.id, if arg(args, "range").is_empty() { "this_month" } else { arg(args, "range") }, end, arg(args, "grain")) {
        Ok(report) => json!({"success": true, "currency": "AED", "spend": report}),
        Err(err) => fail(&err.to_string()),
    }
}

fn orders(db: &Db, user: &User, today: chrono::NaiveDate) -> Value {
    match db.list_products(user.id, None, today, "") {
        Ok(rows) => json!({"success": true, "products": rows.len(), "note": "invoice lines are grouped on the purchases page"}),
        Err(err) => fail(&err.to_string()),
    }
}

fn products(db: &Db, user: &User, args: &Value, today: chrono::NaiveDate) -> Value {
    let sort = if arg(args, "sort").is_empty() { "spend" } else { arg(args, "sort") };
    let range = if arg(args, "range").is_empty() { "all" } else { arg(args, "range") };
    match db.ranked_products(user.id, sort, today, range, 10) {
        Ok(rows) => json!({"success": true, "products": rows}),
        Err(err) => fail(&err.to_string()),
    }
}

fn shopping(db: &Db, user: &User, args: &Value, today: chrono::NaiveDate) -> Value {
    let horizon = num(args, "horizon_days").unwrap_or(7.0) as i64;
    match db.shopping_list(user.id, today, horizon, 20, arg(args, "dept")) {
        Ok(rows) => json!({"success": true, "items": rows}),
        Err(err) => fail(&err.to_string()),
    }
}

fn product(db: &Db, user: &User, args: &Value, today: chrono::NaiveDate) -> Value {
    match db.find_products(user.id, arg(args, "query"), today, 8) {
        Ok(rows) => json!({"success": true, "products": rows}),
        Err(err) => fail(&err.to_string()),
    }
}

fn import_invoice(db: &Db, user: &User, args: &Value) -> Value {
    let parsed = if let Some(text) = args.get("text").and_then(|v| v.as_str()).filter(|t| !t.trim().is_empty()) {
        match invoice::parse_text(text, "mcp") {
            Ok(parsed) => parsed,
            Err(err) => return json!({"success": false, "error": err, "error_code": "pdf_unsupported"}),
        }
    } else {
        let items = args
            .get("items")
            .and_then(|v| v.as_array())
            .map(|rows| {
                rows.iter()
                    .filter_map(|row| {
                        let name = row.get("name").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
                        if name.is_empty() {
                            return None;
                        }
                        let qty = row.get("qty").and_then(|v| v.as_f64()).unwrap_or(1.0);
                        let unit = row.get("unit_price").and_then(|v| v.as_f64());
                        let total = row.get("line_total").and_then(|v| v.as_f64()).unwrap_or(unit.unwrap_or(0.0) * qty);
                        Some(LineItem {
                            name,
                            qty,
                            unit_price: unit.or_else(|| if qty > 0.0 { Some(total / qty) } else { None }),
                            line_total: total,
                            barcode: row.get("barcode").and_then(|v| v.as_str()).unwrap_or("").into(),
                            image_url: String::new(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        ParsedInvoice {
            retailer: arg(args, "retailer").to_string(),
            store_name: arg(args, "store_name").to_string(),
            invoice_no: {
                let no = arg(args, "invoice_no");
                if no.is_empty() { arg(args, "source_ref").to_string() } else { no.to_string() }
            },
            order_no: arg(args, "order_no").to_string(),
            invoice_date: arg(args, "date").to_string(),
            source: arg(args, "source_ref").to_string(),
            items,
        }
    };
    match db.upsert_invoice(user.id, &parsed) {
        Ok(Some(id)) => json!({"success": true, "invoice_id": id, "invoice_no": parsed.invoice_no, "retailer": parsed.retailer}),
        Ok(None) => fail("invoice_no and items are required"),
        Err(err) => fail(&err.to_string()),
    }
}

fn delete_invoice(db: &Db, user: &User, args: &Value) -> Value {
    let id = args.get("invoice_id").and_then(|v| v.as_i64());
    let retailer = args.get("retailer").and_then(|v| v.as_str());
    let no = args.get("invoice_no").and_then(|v| v.as_str());
    match db.delete_invoice(user.id, id, retailer, no) {
        Ok(Some(row)) => json!({"success": true, "deleted": row}),
        Ok(None) => fail("invoice not found"),
        Err(err) => fail(&err.to_string()),
    }
}

fn chrono_today(args: &Value) -> chrono::NaiveDate {
    omabring_core::purchases::parse_day(arg(args, "end")).unwrap_or_else(|| chrono::Utc::now().date_naive())
}

fn arg<'a>(args: &'a Value, key: &str) -> &'a str {
    args.get(key).and_then(|v| v.as_str()).unwrap_or("")
}

fn num(args: &Value, key: &str) -> Option<f64> {
    args.get(key).and_then(|v| v.as_f64().or_else(|| v.as_i64().map(|n| n as f64)).or_else(|| v.as_str().and_then(|s| s.parse().ok())))
}

fn fail(msg: &str) -> Value {
    json!({"success": false, "error": msg})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> (Db, User) {
        let dir = std::env::temp_dir().join(format!("omabring-mcp-{}", omabring_core::crypto::random_token(6)));
        let db = Db::open(dir).unwrap();
        let user = db.create_user("mcp@example.com", "secret1").unwrap();
        (db, user)
    }

    #[test]
    fn initialize_names_the_server() {
        let (db, user) = db();
        let reply = handle(&db, &user, &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}})).unwrap();
        assert_eq!(reply["result"]["serverInfo"]["name"], "OmaBring");
    }

    #[test]
    fn notification_has_no_response() {
        let (db, user) = db();
        assert!(handle(&db, &user, &json!({"jsonrpc":"2.0","method":"notifications/initialized"})).is_none());
    }

    #[test]
    fn search_tool_returns_text_content() {
        let (db, user) = db();
        let reply = handle(&db, &user, &json!({
            "jsonrpc":"2.0","id":2,"method":"tools/call",
            "params":{"name":"carrefour_search","arguments":{"query":"bread"}}
        })).unwrap();
        let text = reply["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("White Bread"));
        assert_eq!(reply["result"]["isError"], false);
    }
}

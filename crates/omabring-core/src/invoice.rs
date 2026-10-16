use chrono::NaiveDate;
use regex::Regex;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LineItem {
    pub name: String,
    #[serde(default)]
    pub qty: f64,
    #[serde(default)]
    pub unit_price: Option<f64>,
    #[serde(default)]
    pub line_total: f64,
    #[serde(default)]
    pub barcode: String,
    #[serde(default)]
    pub image_url: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ParsedInvoice {
    pub retailer: String,
    pub store_name: String,
    pub invoice_no: String,
    pub order_no: String,
    pub invoice_date: String,
    pub source: String,
    pub items: Vec<LineItem>,
}

const SKIP: &[&str] = &[
    "delivery charge",
    "service fee",
    "driver tip",
    "tips",
    "shipping",
    "express delivery",
    "delivery/order",
    "miscellaneous charges",
];

fn skip_name(name: &str) -> bool {
    let n = name.trim().to_ascii_lowercase();
    n.is_empty() || SKIP.iter().any(|s| n.contains(s))
}

fn first(re: &Regex, text: &str) -> String {
    re.captures(text)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
        .unwrap_or_default()
}

fn parse_with(raw: &str, formats: &[&str]) -> String {
    let raw = raw.trim();
    for fmt in formats {
        if let Ok(d) = NaiveDate::parse_from_str(raw, fmt) {
            return d.format("%Y-%m-%d").to_string();
        }
    }
    String::new()
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 0.06_f64.max(0.02 * b.abs().max(1.0))
}

fn cf_from_numbers(name: &str, nums: &[f64]) -> Option<LineItem> {
    if nums.len() >= 9 {
        let t = &nums[nums.len() - 9..];
        let (ord, delivered, uinc, vatp, tinc) = (t[0], t[1], t[2], t[5], t[8]);
        if vatp > 0.0 && vatp <= 5.05 && close(delivered * uinc, tinc) {
            return Some(item(name, delivered, uinc, tinc, ""));
        }
        if vatp > 0.0 && vatp <= 5.05 && close(ord * uinc, tinc) {
            return Some(item(name, ord, uinc, tinc, ""));
        }
    }
    if nums.len() >= 7 {
        let t = &nums[nums.len() - 7..];
        let (qty, uinc, vatp, tinc) = (t[0], t[1], t[4], t[6]);
        if vatp > 0.0 && vatp <= 5.05 && close(qty * uinc, tinc) {
            return Some(item(name, qty, uinc, tinc, ""));
        }
    }
    None
}

fn item(name: &str, qty: f64, unit: f64, total: f64, barcode: &str) -> LineItem {
    LineItem {
        name: name.split_whitespace().collect::<Vec<_>>().join(" "),
        qty,
        unit_price: Some(unit),
        line_total: total,
        barcode: barcode.into(),
        image_url: String::new(),
    }
}

pub fn parse_carrefour_line(line: &str) -> Option<LineItem> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 8 {
        return None;
    }
    let mut vals = Vec::new();
    let mut tail = 0usize;
    for tok in parts.iter().rev() {
        if tok.parse::<f64>().is_ok() && is_plain_number(tok) {
            vals.push(tok.parse::<f64>().unwrap());
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

fn is_plain_number(tok: &str) -> bool {
    let mut seen_dot = false;
    if tok.is_empty() {
        return false;
    }
    for c in tok.chars() {
        if c == '.' {
            if seen_dot {
                return false;
            }
            seen_dot = true;
        } else if !c.is_ascii_digit() {
            return false;
        }
    }
    true
}

pub fn parse_carrefour_text(text: &str, source: &str) -> ParsedInvoice {
    let inv_re = Regex::new(r"Invoice No\.\s*:\s*([A-Za-z0-9]+)").unwrap();
    let date_re = Regex::new(r"Invoice Date\s*:\s*([0-9]{1,2}-[A-Za-z]{3}-[0-9]{4})").unwrap();
    let ord_re = Regex::new(r"Order No\.\s*:\s*([0-9]+)").unwrap();
    let bar_re = Regex::new(r"(?i)Barcode:\s*([0-9]{6,14})").unwrap();
    let mut store = "Carrefour".to_string();
    for label in ["City Center Meaisem", "Ibn Batuta Mall", "MIDTOWN BY DYAR", "City Center Deira"] {
        if text.to_ascii_lowercase().contains(&label.to_ascii_lowercase()) {
            store = format!("Carrefour {label}");
            break;
        }
    }
    let mut items = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(parsed) = parse_carrefour_line(line) {
            items.push(parsed);
            continue;
        }
        if let Some(c) = bar_re.captures(line) {
            if let Some(last) = items.last_mut() {
                if last.barcode.is_empty() {
                    last.barcode = c.get(1).unwrap().as_str().into();
                }
            }
        }
    }
    items.retain(|it| !skip_name(&it.name));
    let date_raw = first(&date_re, text);
    ParsedInvoice {
        retailer: "carrefour".into(),
        store_name: store,
        invoice_no: first(&inv_re, text),
        order_no: first(&ord_re, text),
        invoice_date: parse_with(&date_raw, &["%d-%b-%Y"]),
        source: source.into(),
        items,
    }
}

pub fn parse_grandiose_text(text: &str, source: &str) -> ParsedInvoice {
    let slip_re = Regex::new(r"(?i)Slip:\s*([A-Za-z0-9]+)").unwrap();
    let slip_line = Regex::new(
        r"^(?P<sku>\d{6,14})\s+(?P<qty>\d+(?:\.\d+)?)\s+(?P<price>\d+(?:\.\d+)?)\s+(?P<amt>\d+(?:\.\d+)?)\s*$",
    )
    .unwrap();
    let slip_anywhere = Regex::new(
        r"(?m)^(?P<sku>\d{6,14})\s+(?P<qty>\d+(?:\.\d+)?)\s+(?P<price>\d+(?:\.\d+)?)\s+(?P<amt>\d+(?:\.\d+)?)\s*$",
    )
    .unwrap();
    if slip_re.is_match(text) && slip_anywhere.is_match(text) {
        return parse_grandiose_slip(text, source, &slip_re, &slip_line);
    }
    let inv_re = Regex::new(r"(?i)Tax Invoice\s*#\s*([0-9]+)").unwrap();
    let date_re = Regex::new(r"(?i)Tax invoice Date:\s*([A-Za-z]+ \d{1,2}, \d{4})").unwrap();
    let ord_re = Regex::new(r"(?i)Order\s*#\s*([0-9]+)").unwrap();
    let line_re = Regex::new(
        r"^(?P<name>.+?)\s+(?P<sku>\d{8,14})\s+(?P<qty>\d+(?:\.\d+)?)\s+(?P<price>\d+(?:\.\d+)?)\s+(?P<net>\d+(?:\.\d+)?)\s+(?P<disc>\d+(?:\.\d+)?)\s+(?P<vatp>\d+(?:\.\d+)?)\s+(?P<vata>\d+(?:\.\d+)?)\s+(?P<gross>\d+(?:\.\d+)?)\s*$",
    )
    .unwrap();
    let long_digit = Regex::new(r"\d{8,14}").unwrap();
    let mut items = Vec::new();
    let mut buf = String::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("Products ") || line.starts_with("Net Value") {
            continue;
        }
        let candidate = if buf.is_empty() { line.to_string() } else { format!("{buf} {line}") };
        if let Some(m) = line_re.captures(&candidate) {
            let qty: f64 = m.name("qty").unwrap().as_str().parse().unwrap_or(1.0);
            let gross: f64 = m.name("gross").unwrap().as_str().parse().unwrap_or(0.0);
            items.push(item(
                m.name("name").unwrap().as_str(),
                qty,
                gross / qty.max(0.0001),
                gross,
                m.name("sku").unwrap().as_str(),
            ));
            buf.clear();
            continue;
        }
        if long_digit.is_match(line) || !buf.is_empty() {
            buf = candidate;
        } else {
            buf = line.to_string();
        }
    }
    items.retain(|it| !skip_name(&it.name));
    let date_raw = first(&date_re, text);
    ParsedInvoice {
        retailer: "grandiose".into(),
        store_name: "Grandiose".into(),
        invoice_no: first(&inv_re, text),
        order_no: first(&ord_re, text),
        invoice_date: parse_with(&date_raw, &["%B %d, %Y", "%b %d, %Y"]),
        source: source.into(),
        items,
    }
}

fn parse_grandiose_slip(text: &str, source: &str, slip_re: &Regex, slip_line: &Regex) -> ParsedInvoice {
    let date_re = Regex::new(r"Date:\s*(\d{1,2}/\d{1,2}/\d{4})").unwrap();
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    let mut items = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if let Some(m) = slip_line.captures(lines[i]) {
            let mut name = if i + 1 < lines.len() { lines[i + 1] } else { m.name("sku").unwrap().as_str() };
            if slip_line.is_match(name) || name.starts_with("----") || name.starts_with("Total") {
                name = m.name("sku").unwrap().as_str();
            } else {
                i += 1;
            }
            let qty: f64 = m.name("qty").unwrap().as_str().parse().unwrap_or(1.0);
            let price: f64 = m.name("price").unwrap().as_str().parse().unwrap_or(0.0);
            let amt: f64 = m.name("amt").unwrap().as_str().parse().unwrap_or(0.0);
            items.push(item(name, qty, price, amt, m.name("sku").unwrap().as_str()));
        }
        i += 1;
    }
    items.retain(|it| !skip_name(&it.name));
    let dm = date_re.captures(text).and_then(|c| c.get(1)).map(|m| m.as_str().to_string()).unwrap_or_default();
    ParsedInvoice {
        retailer: "grandiose".into(),
        store_name: "Grandiose".into(),
        invoice_no: first(slip_re, text),
        order_no: String::new(),
        invoice_date: parse_with(&dm, &["%d/%m/%Y"]),
        source: source.into(),
        items,
    }
}

pub fn parse_grandiose_confirmation_html(html: &str, source: &str, date: &str) -> ParsedInvoice {
    let order = Regex::new(r#"(?i)no-link">\s*#\s*([0-9]{5,})"#)
        .ok()
        .and_then(|r| r.captures(html).and_then(|c| c.get(1).map(|m| m.as_str().to_string())))
        .or_else(|| {
            Regex::new(r"(?i)Your Order[\s\S]{0,200}#\s*([0-9]{5,})")
                .ok()
                .and_then(|r| r.captures(html).and_then(|c| c.get(1).map(|m| m.as_str().to_string())))
        })
        .unwrap_or_default();
    let item_re = Regex::new(
        r#"(?i)class="product-name"[^>]*>([^<]+)</p>\s*<p class="sku"[^>]*>SKU:\s*(\d+)</p>[\s\S]{0,800}?class="item-qty"[^>]*>([0-9.]+)</td>[\s\S]{0,500}?class="price">\s*AED\s*([0-9.]+)"#,
    )
    .unwrap();
    let mut items = Vec::new();
    for m in item_re.captures_iter(html) {
        let qty: f64 = m.get(3).unwrap().as_str().parse().unwrap_or(1.0);
        let total: f64 = m.get(4).unwrap().as_str().parse().unwrap_or(0.0);
        items.push(item(
            m.get(1).unwrap().as_str(),
            qty,
            total / qty.max(0.0001),
            total,
            m.get(2).unwrap().as_str(),
        ));
    }
    items.retain(|it| !skip_name(&it.name));
    ParsedInvoice {
        retailer: "grandiose".into(),
        store_name: "Grandiose".into(),
        invoice_no: if order.is_empty() { String::new() } else { format!("order-{order}") },
        order_no: order,
        invoice_date: date.chars().take(10).collect(),
        source: source.into(),
        items,
    }
}

pub fn parse_mmi_text(text: &str, source: &str) -> ParsedInvoice {
    let inv = Regex::new(r"Invoice No:\s*([A-Za-z0-9-]+)").unwrap();
    let date = Regex::new(r"Invoice Date:\s*([0-9]{1,2}-[A-Za-z]{3}-[0-9]{4})").unwrap();
    let order = Regex::new(r"Customer Ref:\s*([A-Za-z0-9]+)").unwrap();
    let line_rx = Regex::new(
        r"(?i)^(?P<code>[A-Z0-9-]{3,16})\s+(?P<name>.+?)\s+(?P<qty>\d+\.\d{2})\s+(?P<uom>CS\d+|EACH|BTL|BOTTLE|CASE)\s+(?P<unit>\d+\.\d{2})\s+.+\s+(?P<total>\d+\.\d{2})\s*$",
    )
    .unwrap();
    let mut items = Vec::new();
    for raw in text.lines() {
        let Some(m) = line_rx.captures(raw.trim()) else { continue };
        let qty: f64 = m.name("qty").unwrap().as_str().parse().unwrap_or(0.0);
        let total: f64 = m.name("total").unwrap().as_str().parse().unwrap_or(0.0);
        let name = m.name("name").unwrap().as_str();
        if total < 1.0 || skip_name(name) {
            continue;
        }
        items.push(item(name, qty, (total / qty.max(0.0001) * 100.0).round() / 100.0, total, m.name("code").unwrap().as_str()));
    }
    let date_raw = first(&date, text);
    ParsedInvoice {
        retailer: "mmi".into(),
        store_name: "MMI".into(),
        invoice_no: first(&inv, text),
        order_no: first(&order, text),
        invoice_date: parse_with(&date_raw, &["%d-%b-%Y"]),
        source: source.into(),
        items,
    }
}

fn strip_tag_blocks(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let mut out = String::new();
    let mut i = 0;
    while i < html.len() {
        let rest = &lower[i..];
        let script = rest.find("<script");
        let style = rest.find("<style");
        let at = match (script, style) {
            (Some(a), Some(b)) => a.min(b),
            (Some(a), None) => a,
            (None, Some(b)) => b,
            (None, None) => {
                out.push_str(&html[i..]);
                break;
            }
        };
        out.push_str(&html[i..i + at]);
        let closer = if rest[at..].starts_with("<script") { "</script>" } else { "</style>" };
        if let Some(end) = rest[at..].find(closer) {
            i += at + end + closer.len();
        } else {
            break;
        }
    }
    out
}

fn html_to_text(html: &str, keep_span: bool) -> String {
    let mut text = strip_tag_blocks(&unescape_html(html));
    let br = Regex::new(r"(?i)<br\s*/?>").unwrap();
    text = br.replace_all(&text, "\n").into_owned();
    let close = if keep_span {
        Regex::new(r"(?i)</(p|tr|td|th|div|h\d|li)>").unwrap()
    } else {
        Regex::new(r"(?i)</(p|tr|div|h\d|li)>").unwrap()
    };
    text = close.replace_all(&text, "\n").into_owned();
    let tags = Regex::new(r"<[^>]+>").unwrap();
    text = tags.replace_all(&text, " ").into_owned();
    let spaces = Regex::new(r"[ \t]+").unwrap();
    spaces.replace_all(&text, " ").into_owned()
}

fn unescape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'&' {
            if let Some(end) = s[i..].find(';') {
                let ent = &s[i + 1..i + end];
                let ch = match ent {
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "nbsp" => Some(' '),
                    "times" => Some('×'),
                    _ if ent.starts_with('#') => {
                        let n = if let Some(hex) = ent.strip_prefix("#x").or_else(|| ent.strip_prefix("#X")) {
                            u32::from_str_radix(hex, 16).ok()
                        } else {
                            ent[1..].parse().ok()
                        };
                        n.and_then(char::from_u32)
                    }
                    _ => None,
                };
                if let Some(ch) = ch {
                    out.push(ch);
                    i += end + 1;
                    continue;
                }
            }
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

pub fn parse_africaneastern_text(text: &str, source: &str) -> ParsedInvoice {
    let inv = Regex::new(r"(?i)Your Invoice\s*#\s*([0-9]+)")
        .unwrap();
    let inv2 = Regex::new(r"(?i)Invoice\s*#\s*([0-9]+)").unwrap();
    let order = Regex::new(r"(?i)Order\s*#\s*([0-9]+)").unwrap();
    let date = Regex::new(r"(?i)Delivery Date:\s*([A-Za-z]+ \d{1,2}, \d{4})").unwrap();
    let item_re = Regex::new(
        r"(?i)(?P<name>[A-Za-z0-9][^.\n]{2,80})\s+SKU:\s*(?P<sku>\d+)\s+UOM:\s*\w+\s+(?P<qty>\d+(?:\.\d+)?)\s+(?P<rsp>\d+(?:\.\d+)?)\s+(?P<disc>\d+(?:\.\d+)?)\s+(?P<net>\d+(?:\.\d+)?)\s+(?P<neta>\d+(?:\.\d+)?)\s+(?P<vat>\d+(?:\.\d+)?)\s+(?P<total>\d+(?:\.\d+)?)",
    )
    .unwrap();
    let mut items = Vec::new();
    for m in item_re.captures_iter(text) {
        let qty: f64 = m.name("qty").unwrap().as_str().parse().unwrap_or(1.0);
        let total: f64 = m.name("total").unwrap().as_str().parse().unwrap_or(0.0);
        let name = m.name("name").unwrap().as_str();
        if total <= 0.0 || skip_name(name) {
            continue;
        }
        items.push(item(name, qty, (total / qty.max(0.0001) * 100.0).round() / 100.0, total, m.name("sku").unwrap().as_str()));
    }
    let date_raw = first(&date, text);
    let invoice_no = {
        let a = first(&inv, text);
        if !a.is_empty() {
            a
        } else {
            first(&inv2, text)
        }
    };
    let order_no = first(&order, text);
    let invoice_no = if invoice_no.is_empty() && !order_no.is_empty() {
        format!("order-{order_no}")
    } else {
        invoice_no
    };
    ParsedInvoice {
        retailer: "africaneastern".into(),
        store_name: "African + Eastern".into(),
        invoice_no,
        order_no,
        invoice_date: parse_with(&date_raw, &["%b %d, %Y", "%B %d, %Y"]),
        source: source.into(),
        items,
    }
}

pub fn parse_africaneastern_html(html: &str, source: &str) -> ParsedInvoice {
    parse_africaneastern_text(&html_to_text(html, false), source)
}

const CAREEM_SUMMARY: &[&str] = &[
    "subtotal", "sub total", "total", "grand total", "order total", "items total", "vat", "tax",
    "discount", "promo", "promotion", "voucher", "coupon", "delivery", "service", "packaging",
    "small order", "careem plus", "careem pay", "tip", "tips", "rounding", "amount paid",
    "amount due", "paid", "payment", "wallet", "credit", "card", "you saved", "savings",
    "surcharge", "peak", "your total bill", "original cart", "original basket", "cart total",
    "basket total", "restaurant discount", "reward your captain", "payment method",
    "no delivery fee", "promo code", "incl tax",
];

fn careem_summary(name: &str) -> bool {
    let n = Regex::new(r"[^a-z0-9 ]+")
        .unwrap()
        .replace_all(&name.to_ascii_lowercase(), " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if n.is_empty() {
        return true;
    }
    for s in CAREEM_SUMMARY {
        if s.contains(' ') {
            if n.contains(s) {
                return true;
            }
        } else if n == *s || n.starts_with(&format!("{s} ")) {
            return true;
        }
    }
    false
}

fn careem_named(raw: &str) -> bool {
    let t = raw.trim();
    if t.is_empty() || t.len() > 120 {
        return false;
    }
    let low = t.to_ascii_lowercase();
    if low == "aed" || low == "dhs" || low == "dh" {
        return false;
    }
    t.chars().filter(|c| c.is_ascii_alphabetic()).count() >= 2
}

fn careem_date(raw: &str) -> String {
    let raw = raw.trim().trim_end_matches(',');
    parse_with(
        raw,
        &[
            "%d %b %Y", "%d %B %Y", "%d-%b-%Y", "%d-%B-%Y", "%d/%m/%Y", "%b %d, %Y", "%B %d, %Y",
            "%b %d %Y", "%B %d %Y", "%Y-%m-%d",
        ],
    )
}

fn careem_money_line(line: &str) -> Option<Vec<f64>> {
    let s = line.trim();
    let stripped = Regex::new(r"(?i)AED").unwrap().replace_all(s, "");
    if stripped.chars().any(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    if !Regex::new(r"(?i)AED|\d+\.\d{2}").unwrap().is_match(s) {
        let amount = Regex::new(r"(?i)^(?:AED\s*)?(\d{1,6}(?:\.\d{1,2})?)(?:\s*AED)?$").unwrap();
        return amount.captures(s).and_then(|c| c.get(1)?.as_str().parse().ok()).map(|n| vec![n]);
    }
    let re = Regex::new(r"\d{1,6}(?:\.\d{1,2})?").unwrap();
    let vals: Vec<f64> = re.find_iter(s).filter_map(|m| m.as_str().parse().ok()).collect();
    if vals.is_empty() { None } else { Some(vals) }
}

fn careem_following_price(lines: &[&str], i: usize) -> (Option<f64>, usize) {
    let mut j = i + 1;
    let mut last = None;
    while j < lines.len() {
        let Some(vals) = careem_money_line(lines[j]) else { break };
        last = vals.last().copied();
        j += 1;
    }
    (last, j)
}

fn careem_item(name: &str, qty: f64, total: f64) -> Option<LineItem> {
    let mut clean = name.split_whitespace().collect::<Vec<_>>().join(" ");
    clean = clean.trim_matches(|c: char| " -–—·|".contains(c)).to_string();
    let re = Regex::new(r"(?i)^AED\s+|\s+AED$").unwrap();
    clean = re.replace_all(&clean, "").trim().trim_matches(|c: char| " -–—·|".contains(c)).to_string();
    if !careem_named(&clean) || total <= 0.0 {
        return None;
    }
    let qty = if qty > 0.0 { qty } else { 1.0 };
    Some(item(&clean, qty, (total / qty * 100.0).round() / 100.0, total, ""))
}

pub fn parse_careem_text(text: &str, source: &str, date: &str) -> ParsedInvoice {
    let order_re = Regex::new(r"(?i)Order\s*(?:ID|No\.?|Number|#)\s*[:#]?\s*([A-Za-z0-9][A-Za-z0-9-]{3,})").unwrap();
    let inv_re = Regex::new(r"(?i)(?:Tax\s+)?Invoice\s*(?:No\.?|Number|#)\s*[:#]?\s*([A-Za-z0-9][A-Za-z0-9/-]{2,})").unwrap();
    let place_re = Regex::new(r"(?i)(?:Your order from|Order from|Delivered from|Restaurant|Merchant|Store)\s*[:\-]?\s*(.+)").unwrap();
    let date_re = Regex::new(
        r"(?i)(?:Invoice|Order|Delivery|Placed on|Date)\s*(?:Date)?\s*[:\-]?\s*(\d{1,2}[ /-][A-Za-z]{3,9}[ /-]\d{4}|[A-Za-z]{3,9} \d{1,2},? \d{4}|\d{4}-\d{2}-\d{2}|\d{1,2}/\d{1,2}/\d{4})",
    )
    .unwrap();
    let qty_first = Regex::new(
        r"(?i)^(?P<qty>\d{1,2})\s*(?:[xX×]\s*|[xX×]?\s+)(?P<name>[A-Za-z_][^\n]*?)\s+(?:AED\s*)?(?P<total>\d{1,6}(?:\.\d{1,2})?)(?:\s*AED)?$",
    )
    .unwrap();
    let qty_last = Regex::new(
        r"(?i)^(?P<name>.+?)\s*[xX×]\s*(?P<qty>\d{1,2})\s+(?:AED\s*)?(?P<total>\d{1,6}(?:\.\d{1,2})?)(?:\s*AED)?$",
    )
    .unwrap();
    let no_qty = Regex::new(
        r"(?i)^(?P<name>.+?)\s+(?:AED\s*)?(?P<total>\d{1,6}\.\d{2})(?:\s*AED)?$",
    )
    .unwrap();
    let qty_only_first = Regex::new(r"(?i)^(?P<qty>\d{1,2})\s*[xX×]\s*(?P<name>[A-Za-z_].*)$").unwrap();
    let qty_only_last = Regex::new(r"(?i)^(?P<name>.+?)\s*[xX×]\s*(?P<qty>\d{1,2})$").unwrap();

    let order = first(&order_re, text);
    let inv = first(&inv_re, text);
    let date_raw = first(&date_re, text);
    let mut place = first(&place_re, text).split_whitespace().collect::<Vec<_>>().join(" ");
    place = place.trim_matches(|c: char| " -–—:·|".contains(c)).to_string();
    let year = date.get(..4).and_then(|y| y.parse::<i32>().ok());
    let owned: Vec<String> = text
        .lines()
        .map(|ln| ln.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|ln| !ln.is_empty())
        .collect();
    let lines: Vec<&str> = owned.iter().map(String::as_str).collect();
    for (i, line) in lines.iter().enumerate() {
        if line.eq_ignore_ascii_case("delivery details") {
            if i + 1 < lines.len() {
                place = lines[i + 1].to_string();
            }
            break;
        }
    }
    let mut items = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if careem_money_line(line).is_some() {
            i += 1;
            continue;
        }
        if let Some(rest) = line.strip_prefix('+') {
            let mut name = rest.trim().to_string();
            let mut name_i = i;
            if name.is_empty() && i + 1 < lines.len() {
                name_i = i + 1;
                name = lines[name_i].to_string();
            }
            let (price, j) = careem_following_price(&lines, name_i);
            if let Some(price) = price {
                if let Some(hit) = careem_item(&name, 1.0, price) {
                    items.push(hit);
                    i = j;
                    continue;
                }
            }
            i += 1;
            continue;
        }
        let mut hit = None;
        for rx in [&qty_first, &qty_last, &no_qty] {
            if let Some(m) = rx.captures(line) {
                let qty = m.name("qty").and_then(|g| g.as_str().parse().ok()).unwrap_or(1.0);
                let total: f64 = m.name("total").unwrap().as_str().parse().unwrap_or(0.0);
                if let Some(row) = careem_item(m.name("name").unwrap().as_str(), qty, total) {
                    hit = Some(row);
                    break;
                }
            }
        }
        if let Some(hit) = hit {
            items.push(hit);
            i += 1;
            continue;
        }
        let (price, j) = careem_following_price(&lines, i);
        if price.is_some() {
            let mut paired = false;
            for rx in [&qty_only_first, &qty_only_last] {
                if let Some(m) = rx.captures(line) {
                    let qty: f64 = m.name("qty").unwrap().as_str().parse().unwrap_or(1.0);
                    if let Some(row) = careem_item(m.name("name").unwrap().as_str(), qty, price.unwrap()) {
                        items.push(row);
                        i = j;
                        paired = true;
                        break;
                    }
                }
            }
            if !paired {
                i += 1;
            }
            continue;
        }
        i += 1;
    }
    items.retain(|it| !skip_name(&it.name) && !careem_summary(&it.name));
    let mut invoice_date = if date_raw.is_empty() { String::new() } else { careem_date(&date_raw) };
    if invoice_date.is_empty() && !date.is_empty() {
        invoice_date = date.chars().take(10).collect();
    }
    let _ = year;
    ParsedInvoice {
        retailer: "careem".into(),
        store_name: if place.is_empty() { "Careem".into() } else { format!("Careem · {place}") },
        invoice_no: if !inv.is_empty() { inv } else if !order.is_empty() { format!("order-{order}") } else { String::new() },
        order_no: order,
        invoice_date,
        source: source.into(),
        items,
    }
}

pub fn parse_careem_html(html: &str, source: &str, date: &str) -> ParsedInvoice {
    parse_careem_text(&html_to_text(html, true), source, date)
}

pub fn parse_mcdonalds_text(text: &str, source: &str, date: &str) -> ParsedInvoice {
    let lines: Vec<String> = text
        .lines()
        .map(|ln| ln.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|ln| !ln.is_empty())
        .collect();
    let field = |label: &str| -> String {
        let want = label.trim_end_matches(':').to_ascii_lowercase();
        for (i, line) in lines.iter().enumerate() {
            if line.trim_end_matches(':').eq_ignore_ascii_case(&want) {
                return lines.get(i + 1).cloned().unwrap_or_default();
            }
        }
        String::new()
    };
    let rest_no = field("Restaurant Number");
    let place = field("Restaurant Name");
    let order = field("Order Id");
    let date_raw = {
        let d = field("Delivery Date");
        if d.is_empty() { field("Date") } else { d }
    };
    let invoice_date = mcd_date(&date_raw).or_else(|| {
        if date.len() >= 10 { Some(date[..10].to_string()) } else { None }
    }).unwrap_or_default();
    let mut start = 0usize;
    for (i, line) in lines.iter().enumerate() {
        if line.eq_ignore_ascii_case("item cost") {
            start = i + 1;
            break;
        }
    }
    let qty_re = Regex::new(r"^(\d{1,2})$").unwrap();
    let money_re = Regex::new(r"(?i)^(?:AED\s*)?(\d{1,6}\.\d{2})(?:\s*AED)?$").unwrap();
    let summary = Regex::new(r"(?i)^(sub\s*total|delivery fee|total\b)").unwrap();
    let mut items = Vec::new();
    let mut i = start;
    while i < lines.len() {
        let line = &lines[i];
        if summary.is_match(line) {
            break;
        }
        if qty_re.is_match(line) && i + 1 < lines.len() {
            let name = lines[i + 1].clone();
            let qty: f64 = line.parse().unwrap_or(1.0);
            let mut j = i + 2;
            let mut price: Option<f64> = None;
            if j < lines.len() {
                if let Some(c) = money_re.captures(&lines[j]) {
                    price = c.get(1).and_then(|m| m.as_str().parse().ok());
                    j += 1;
                }
            }
            if let Some(price) = price {
                if price > 0.0 && !summary.is_match(&name) && !skip_name(&name) {
                    items.push(item(&name, qty, (price / qty * 100.0).round() / 100.0, price, ""));
                }
            }
            i = j;
            continue;
        }
        i += 1;
    }
    let stamp = if invoice_date.is_empty() { "nodate".into() } else { invoice_date.replace('-', "") };
    let invoice_no = if order.is_empty() {
        String::new()
    } else {
        format!("{}-{order}-{stamp}", if rest_no.is_empty() { "mcd" } else { rest_no.as_str() })
    };
    ParsedInvoice {
        retailer: "mcdonalds".into(),
        store_name: if place.is_empty() { "McDonald's".into() } else { format!("McDonald's · {place}") },
        invoice_no,
        order_no: order,
        invoice_date,
        source: source.into(),
        items,
    }
}

fn mcd_date(raw: &str) -> Option<String> {
    let re = Regex::new(r"(\d{1,2})/(\d{1,2})/(\d{2,4})").unwrap();
    let c = re.captures(raw)?;
    let day: u32 = c.get(1)?.as_str().parse().ok()?;
    let month: u32 = c.get(2)?.as_str().parse().ok()?;
    let mut year: i32 = c.get(3)?.as_str().parse().ok()?;
    if year < 100 {
        year += 2000;
    }
    NaiveDate::from_ymd_opt(year, month, day).map(|d| d.format("%Y-%m-%d").to_string())
}

pub fn parse_mcdonalds_html(html: &str, source: &str, date: &str) -> ParsedInvoice {
    parse_mcdonalds_text(&html_to_text(html, true), source, date)
}

pub fn parse_text(text: &str, source: &str) -> Result<ParsedInvoice, String> {
    let low = text.to_ascii_lowercase();
    if low.contains("grandiose") {
        return Ok(parse_grandiose_text(text, source));
    }
    if low.contains("mmi home delivery") || low.contains("mmihomedelivery") {
        return Ok(parse_mmi_text(text, source));
    }
    if low.contains("african") && low.contains("eastern") {
        return Ok(parse_africaneastern_text(text, source));
    }
    if low.contains("careem") {
        return Ok(parse_careem_text(text, source, ""));
    }
    if low.contains("mcdonald") {
        return Ok(parse_mcdonalds_text(text, source, ""));
    }
    if low.contains("majid al futtaim") || low.contains("carrefour") || low.contains("tax invoice") {
        return Ok(parse_carrefour_text(text, source));
    }
    Err("unrecognized invoice text".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    const CF: &str = "
Invoice No. : 93084417
Invoice Date : 23-Aug-2026
Order No. : 784030019566483
City Center Meaisem
Coca-Cola Zero Sugar 2.0 2.0 5.99 5.71 11.41 5.00 0.57 0.00 11.98
Carbonated Soft Drink PET
Barcode: 5000112668209
Delivery Charge 1.0 1.0 0.00 0.00 0.00 5.00 0.00 0.00 0.00
Barcode: 2000019089384
Service Fee 1.0 1.0 3.42 3.26 3.26 5.00 0.16 0.00 3.42
Barcode: 2000019264163
";

    #[test]
    fn carrefour_skips_fees() {
        let out = parse_carrefour_text(CF, "");
        assert_eq!(out.invoice_no, "93084417");
        assert_eq!(out.invoice_date, "2026-08-23");
        assert_eq!(out.items.len(), 1);
        assert_eq!(out.items[0].barcode, "5000112668209");
        assert_eq!(out.items[0].qty, 2.0);
        assert_eq!(out.items[0].line_total, 11.98);
        let phone = parse_carrefour_text(
            "Invoice No. : 1\nInvoice Date : 10-May-2025\nHMD PULSE PRO 8 25 1 499.00 475.24 475.24 5.00 23.76 499.00\nBarcode: 6438409091819\n",
            "",
        );
        assert_eq!(phone.items[0].name, "HMD PULSE PRO");
        assert_eq!(phone.items[0].qty, 1.0);
        assert_eq!(phone.items[0].line_total, 499.0);
    }

    #[test]
    fn grandiose_and_slip() {
        let gr = "TAX INVOICE\nGRANDIOSE SUPERMARKET\nTax Invoice # 3325511433512 Tax invoice Date: May 18, 2026\nOrder # 110670729\nProducts SKU Qty Price Net Amount Discount VAT % VAT Amount Gross Amount\nVeroni Hot Salami 9814172000000 0.25 94.28 23.57 0.00 5.00 1.18 24.75\nNapoli (Non Halal)\nBelgium French 9816250000000 1 21.43 21.43 0.00 5.00 1.07 22.50\n";
        let out = parse_grandiose_text(gr, "");
        assert_eq!(out.invoice_no, "3325511433512");
        assert_eq!(out.invoice_date, "2026-05-18");
        let slip = "TAX INVOICE\nSlip: 000000ST11000167487\nDate: 06/08/2026 7:50 PM\n3000000003617 1 19.00 19.00\nVeroni Spianata Romana Slices\n8005573008356 1 19.00 19.00\nVeroni Salami Napoli Slices\n3000000003073 1 0.60 0.60\nPacific Printed Brown Paper Bag\nTotal 38.60\n";
        let slip_out = parse_grandiose_text(slip, "");
        assert_eq!(slip_out.invoice_no, "000000ST11000167487");
        assert_eq!(slip_out.invoice_date, "2026-08-06");
        assert_eq!(slip_out.items.len(), 3);
    }

    #[test]
    fn mmi_heineken_line() {
        let text = "Invoice No: ARInv-12869998\nInvoice Date: 01-Apr-2026\nCustomer Ref: 260401HVW9G0OR\nMMI Home Delivery\n02420 HEINEKEN CANS 50 CL 2.00 CS24 131.14 262.27 26.23 209.81 62.94 272.75 05 13.64 286.39\nExpress Delivery Charge 9.52 05 0.48 10\n";
        let out = parse_mmi_text(text, "");
        assert_eq!(out.invoice_no, "ARInv-12869998");
        assert_eq!(out.invoice_date, "2026-04-01");
        assert_eq!(out.items[0].name, "HEINEKEN CANS 50 CL");
        assert_eq!(out.items[0].line_total, 286.39);
    }

    #[test]
    fn careem_keeps_dishes() {
        let text = "Tax Invoice\nCareem Networks FZ LLC\nInvoice No: CRM-2026-884213\nOrder ID: 8842137755\nYour order from: Al Safadi Restaurant\nInvoice Date: 26 Aug 2026\n2 x Chicken Shawarma Wrap AED 36.00\n1 x Mixed Grill Platter AED 58.50\nHummus Beiruty AED 18.00\nDelivery fee AED 5.00\nService fee AED 2.50\nSubtotal AED 112.50\nVAT 5% AED 5.63\nTotal AED 123.13\n";
        let out = parse_careem_text(text, "careem.pdf", "");
        assert_eq!(out.invoice_no, "CRM-2026-884213");
        assert_eq!(out.invoice_date, "2026-08-26");
        assert_eq!(out.store_name, "Careem · Al Safadi Restaurant");
        assert_eq!(out.items.len(), 3);
        assert_eq!(out.items[0].qty, 2.0);
        assert_eq!(out.items[0].line_total, 36.0);
        assert_eq!(out.items[0].unit_price, Some(18.0));
    }

    #[test]
    fn mcdonalds_priced_rows_only() {
        let html = r#"<div>Restaurant Number:</div><div>1840105</div><div>Restaurant Name:</div><div>ENOC 1039 Dubai Production City</div><div>Order Id:</div><div>0786</div><div>Delivery Date:</div><div>05/04/26 17:06</div><table><tr><th>Qty</th><th>Item Detail</th><th>Item Cost</th></tr><tr><td>1</td><td>Double Cheeseburger Large Meal</td><td>AED 29.00</td></tr><tr><td>&nbsp;</td><td>1 Double Cheeseburger</td><td></td></tr><tr><td>1</td><td>Cheeseburger</td><td>AED 8.00</td></tr><tr><td colspan="2">Sub Total</td><td>AED 37.00</td></tr></table>"#;
        let out = parse_mcdonalds_html(html, "mail", "");
        assert_eq!(out.invoice_no, "1840105-0786-20260405");
        assert_eq!(out.store_name, "McDonald's · ENOC 1039 Dubai Production City");
        assert_eq!(out.items.len(), 2);
        assert_eq!(out.items[0].line_total, 29.0);
    }
}

use serde_json::Value;

const REMOVE_PREFIXES: &[&str] = &[
    "remove from the cart ",
    "remove from cart ",
    "take out the ",
    "take out ",
    "togli dal carrello ",
    "togli la ",
    "togli il ",
    "togli l'",
    "togli ",
    "remove the ",
    "remove ",
];

const STOP: &[&str] = &[
    "the", "la", "il", "lo", "a", "un", "una", "le", "gli", "and", "or", "di", "del", "dal",
];

pub fn fold_name(value: &str) -> String {
    let mut out = String::new();
    let mut gap = false;
    for c in value.chars() {
        if c.is_ascii_alphanumeric() {
            if gap && !out.is_empty() {
                out.push(' ');
            }
            gap = false;
            out.push(c.to_ascii_lowercase());
        } else {
            gap = true;
        }
    }
    out.trim().to_string()
}

pub fn fold_significant(value: &str) -> String {
    fold_name(value)
        .split_whitespace()
        .filter(|t| !STOP.contains(t) && t.len() > 1)
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn peel_remove_name(value: &str) -> Option<String> {
    let q = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if q.is_empty() {
        return None;
    }
    let low = q.to_ascii_lowercase();
    for prefix in REMOVE_PREFIXES {
        if low.starts_with(prefix) {
            let rest = q[prefix.len()..].trim().to_string();
            return if rest.is_empty() { None } else { Some(rest) };
        }
    }
    None
}

pub fn token_score(query: &str, name: &str) -> f64 {
    let q_fold = fold_significant(query);
    let n_fold = fold_significant(name);
    let q: std::collections::HashSet<&str> = q_fold.split_whitespace().filter(|t| t.len() > 2).collect();
    let n: std::collections::HashSet<&str> = n_fold.split_whitespace().filter(|t| t.len() > 2).collect();
    if q.is_empty() || n.is_empty() {
        return 0.0;
    }
    q.intersection(&n).count() as f64 / q.len() as f64
}

fn field<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(|x| x.as_str()).unwrap_or("")
}

/// Resolve a remove or set against lines already in the cart.
/// `item_id` is the numeric quote item id, not the EAN. `sku` is the catalog id.
pub fn match_cart_line(lines: &[Value], item_id: &str, sku: &str, name: &str) -> Option<Value> {
    let rows: Vec<&Value> = lines.iter().filter(|v| v.is_object()).collect();
    let want_item = item_id.trim();
    let want_sku = sku.trim();
    let mut want_name = name.trim().to_string();
    if let Some(peeled) = peel_remove_name(&want_name) {
        want_name = peeled;
    }
    if !want_item.is_empty() {
        for it in &rows {
            if want_item == field(it, "item_id").trim() || want_item == field(it, "uid").trim() {
                return Some((*it).clone().clone());
            }
        }
    }
    if !want_sku.is_empty() {
        for it in &rows {
            if field(it, "id").trim() == want_sku || field(it, "sku").trim() == want_sku {
                return Some((*it).clone().clone());
            }
        }
    }
    if want_name.is_empty() {
        return None;
    }
    let folded = fold_significant(&want_name);
    if folded.is_empty() {
        return None;
    }
    let exact: Vec<&&Value> = rows
        .iter()
        .filter(|it| fold_significant(field(it, "name")) == folded)
        .collect();
    if let Some(hit) = exact.first() {
        return Some((**hit).clone());
    }
    let mut contained: Vec<&&Value> = rows
        .iter()
        .filter(|it| {
            let n = fold_significant(field(it, "name"));
            !n.is_empty() && (n.contains(&folded) || folded.contains(&n))
        })
        .collect();
    if contained.len() == 1 {
        return Some((*contained[0]).clone().clone());
    }
    if contained.len() > 1 {
        contained.sort_by(|a, b| {
            let sa = token_score(&want_name, field(a, "name"));
            let sb = token_score(&want_name, field(b, "name"));
            sb.partial_cmp(&sa)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    fold_significant(field(a, "name"))
                        .len()
                        .cmp(&fold_significant(field(b, "name")).len())
                })
        });
        return Some((*contained[0]).clone().clone());
    }
    let mut ranked = rows.clone();
    ranked.sort_by(|a, b| {
        token_score(&want_name, field(b, "name"))
            .partial_cmp(&token_score(&want_name, field(a, "name")))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    if ranked.is_empty() {
        return None;
    }
    let best = token_score(&want_name, field(ranked[0], "name"));
    let second = if ranked.len() > 1 {
        token_score(&want_name, field(ranked[1], "name"))
    } else {
        0.0
    };
    if best >= 0.5 && best > second {
        return Some((*ranked[0]).clone());
    }
    None
}

pub fn missing_line_error(label: &str, lines: &[Value], store: &str) -> String {
    let names: Vec<&str> = lines
        .iter()
        .filter_map(|it| {
            let n = field(it, "name");
            let id = field(it, "id");
            let pick = if n.is_empty() { id } else { n };
            if pick.trim().is_empty() {
                None
            } else {
                Some(pick.trim())
            }
        })
        .collect();
    let what = {
        let t = label.trim();
        if t.is_empty() {
            "That product"
        } else {
            t
        }
    };
    if names.is_empty() {
        format!("{what} is not in the official {store} cart. The cart is empty.")
    } else {
        format!(
            "{what} is not in the official {store} cart. Cart has: {}.",
            names.join(", ")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn lines() -> Vec<Value> {
        vec![
            json!({"item_id": "12115690", "id": "5449000000996", "sku": "5449000000996", "name": "Coca-Cola Zero Sugar 330ml"}),
            json!({"item_id": "99", "id": "8002270015223", "sku": "8002270015223", "name": "S.Pellegrino Sparkling 500ml"}),
        ]
    }

    #[test]
    fn peel_and_match_by_name_or_item_id() {
        assert_eq!(
            peel_remove_name("togli la Coca-Cola Zero").as_deref(),
            Some("Coca-Cola Zero")
        );
        let hit = match_cart_line(&lines(), "", "", "take out the Coca-Cola Zero").unwrap();
        assert_eq!(hit["item_id"], "12115690");
        let by_id = match_cart_line(&lines(), "12115690", "", "").unwrap();
        assert_eq!(by_id["sku"], "5449000000996");
        assert!(match_cart_line(&lines(), "", "", "olive oil").is_none());
    }
}

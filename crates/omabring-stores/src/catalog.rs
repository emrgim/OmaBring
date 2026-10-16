//! Live UAE storefront search and sign-in.
//!
//! Each shop is called the way its own site does: Constructor.io for Carrefour,
//! Magento GraphQL for Grandiose and African & Eastern, Magento REST plus the
//! storefront Algolia config for Union Coop, the Retter catalog for MMI, and
//! the public search HTML for Waitrose and Spinneys.

use std::time::Duration;

use omabring_core::{
    BackendError, Cart, Offer, OfferSource, RemoteCheckout, StoreId, StoreSecrets,
};
use serde_json::{json, Value};

const UA: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36";

pub fn search(store: StoreId, query: &str) -> Result<Vec<Offer>, BackendError> {
    let query = query.trim();
    if query.is_empty() {
        return Err(BackendError::bad_response("enter a product to search"));
    }
    match store {
        StoreId::Carrefour => search_carrefour(query),
        StoreId::Grandiose => search_grandiose(query),
        StoreId::UnionCoop => search_unioncoop(query),
        StoreId::Waitrose => search_html(
            store,
            &format!("https://www.waitrose.ae/en/search/?q={}", urlencode(query)),
            "https://www.waitrose.ae",
            "/en/products/",
        ),
        StoreId::Spinneys => search_html(
            store,
            &format!(
                "https://www.spinneys.com/en-ae/search/?q={}",
                urlencode(query)
            ),
            "https://www.spinneys.com",
            "/en-ae/catalogue/",
        ),
        StoreId::Mmi => search_mmi(query),
        StoreId::AfricanEastern => search_african(query),
    }
}

pub fn sign_in(
    store: StoreId,
    username: &str,
    password: &str,
) -> Result<StoreSecrets, BackendError> {
    let username = username.trim();
    if username.is_empty() || password.is_empty() {
        return Err(BackendError::bad_response(
            "username and password are required to link a store",
        ));
    }
    let mut secrets = StoreSecrets {
        username: username.to_string(),
        password: password.to_string(),
        ..StoreSecrets::blank()
    };
    match store {
        StoreId::Waitrose | StoreId::Spinneys => Ok(secrets),
        StoreId::Carrefour => {
            let session = carrefour_login(username, password)?;
            secrets.token = Some(session.token);
            secrets.user_id = Some(session.user_id);
            Ok(secrets)
        }
        StoreId::Grandiose => {
            secrets.token = Some(magento_token(
                "https://www.grandiose.ae/rest/V1/integration/customer/token",
                username,
                password,
                "Grandiose",
            )?);
            Ok(secrets)
        }
        StoreId::UnionCoop => {
            secrets.token = Some(magento_token(
                "https://www.unioncoop.ae/rest/V1/integration/customer/token",
                username,
                password,
                "Union Coop",
            )?);
            Ok(secrets)
        }
        StoreId::Mmi => {
            secrets.token = Some(mmi_login(username, password)?);
            Ok(secrets)
        }
        StoreId::AfricanEastern => {
            secrets.token = Some(african_login(username, password)?);
            Ok(secrets)
        }
    }
}

pub fn push_cart(
    store: StoreId,
    secrets: &StoreSecrets,
    cart: &Cart,
) -> Result<RemoteCheckout, BackendError> {
    let token = secrets
        .token
        .as_deref()
        .map(str::trim)
        .filter(|token| !token.is_empty());
    match store {
        StoreId::Grandiose => {
            let token = token.ok_or_else(BackendError::not_configured)?;
            push_grandiose(token, cart)?;
            Ok(RemoteCheckout {
                reference: Some("grandiose-cart".into()),
                url: Some("https://www.grandiose.ae/checkout/".into()),
            })
        }
        StoreId::UnionCoop => {
            let token = token.ok_or_else(BackendError::not_configured)?;
            push_unioncoop(token, cart)?;
            Ok(RemoteCheckout {
                reference: Some("unioncoop-cart".into()),
                url: Some("https://www.unioncoop.ae/checkout/".into()),
            })
        }
        StoreId::Carrefour => Ok(RemoteCheckout {
            reference: None,
            url: Some("https://www.carrefouruae.com/mafuae/en/cart".into()),
        }),
        _ => Err(BackendError::unsupported(format!(
            "{} keeps the cart on its own website",
            store.name()
        ))),
    }
}

fn search_carrefour(query: &str) -> Result<Vec<Offer>, BackendError> {
    let url = format!(
        "https://ac.cnstrc.com/search/{}?key=key_UzmQuiABmYtLGFME&c=cio-python-bringfast-1.0&i=bringfast&s=1&page=1&num_results_per_page=8",
        urlencode(query)
    );
    let body = get_text(&url)?;
    offers_from_carrefour(&body)
}

pub(crate) fn offers_from_carrefour(body: &str) -> Result<Vec<Offer>, BackendError> {
    let value: Value = serde_json::from_str(body)
        .map_err(|_| BackendError::bad_response("Carrefour search was not JSON"))?;
    let results = value
        .pointer("/response/results")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut offers = Vec::new();
    for item in results {
        let data = item.get("data").cloned().unwrap_or(Value::Null);
        let name = item
            .get("value")
            .and_then(Value::as_str)
            .or_else(|| data.get("online_name_en").and_then(Value::as_str))
            .or_else(|| data.get("name").and_then(Value::as_str))
            .unwrap_or("")
            .trim()
            .to_string();
        let Some(price) = data.get("price").and_then(money) else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        let product_id = data
            .get("id")
            .or_else(|| data.get("code"))
            .map(id_string)
            .filter(|id| !id.is_empty())
            .unwrap_or_else(|| name.clone());
        offers.push(live_offer(
            StoreId::Carrefour,
            product_id,
            name,
            price,
            data.get("currency")
                .and_then(Value::as_str)
                .unwrap_or("AED"),
            first_image(&data),
        ));
        if offers.len() == 8 {
            break;
        }
    }
    Ok(offers)
}

fn search_grandiose(query: &str) -> Result<Vec<Offer>, BackendError> {
    let body = graphql(
        "https://www.grandiose.ae/graphql",
        "",
        r#"query Search($q: String!, $n: Int!) { products(search: $q, pageSize: $n) { items { sku name small_image { url } thumbnail { url } price_range { minimum_price { regular_price { value currency } } } } } }"#,
        json!({"q": query, "n": 20}),
        "Grandiose",
    )?;
    let mut offers = offers_from_magento_products(StoreId::Grandiose, &body)?;
    rank_offers(query, &mut offers);
    offers.truncate(8);
    Ok(offers)
}

fn search_african(query: &str) -> Result<Vec<Offer>, BackendError> {
    let body = graphql(
        "https://www.africaneasternonline.com/graphql",
        "",
        r#"query ($q: String!, $n: Int!) { products(search: $q, pageSize: $n) { items { sku name small_image { url } thumbnail { url } price_range { minimum_price { regular_price { value currency } } } } } }"#,
        json!({"q": query, "n": 20}),
        "African & Eastern",
    )?;
    let mut offers = offers_from_magento_products(StoreId::AfricanEastern, &body)?;
    offers.retain(|offer| query_matches(query, &offer.name));
    rank_offers(query, &mut offers);
    offers.truncate(8);
    Ok(offers)
}

pub(crate) fn offers_from_magento_products(
    store: StoreId,
    body: &str,
) -> Result<Vec<Offer>, BackendError> {
    let value: Value = serde_json::from_str(body)
        .map_err(|_| BackendError::bad_response(format!("{store} search was not JSON")))?;
    if let Some(message) = graphql_error(&value) {
        return Err(BackendError::bad_response(format!("{store}: {message}")));
    }
    let items = value
        .pointer("/data/products/items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut offers = Vec::new();
    for item in items {
        let name = item
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let price = item.pointer("/price_range/minimum_price/regular_price/value");
        let Some(price) = price.and_then(money) else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        let currency = item
            .pointer("/price_range/minimum_price/regular_price/currency")
            .and_then(Value::as_str)
            .unwrap_or("AED");
        let product_id = item
            .get("sku")
            .map(id_string)
            .filter(|id| !id.is_empty())
            .unwrap_or_else(|| name.clone());
        offers.push(live_offer(
            store,
            product_id,
            name,
            price,
            currency,
            first_image(&item),
        ));
    }
    Ok(offers)
}

fn search_mmi(query: &str) -> Result<Vec<Offer>, BackendError> {
    let payload = json!({"searchTerm": query, "sorting": "-stock", "pageSize": 8});
    let encoded = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        serde_json::to_vec(&payload).unwrap_or_default(),
    );
    let url = format!(
        "https://api.mmiprod.retter.io/o9pki8qf/CALL/ProductManager/searchProductsV2/default?data={}&__isbase64=true",
        urlencode(&encoded)
    );
    let body = send(
        "GET",
        &url,
        None,
        &[
            ("accept", "application/json"),
            ("channel-id", "mmiDubai"),
            ("client-type", "web"),
        ],
    )?;
    let mut offers = offers_from_mmi(&body)?;
    offers.retain(|offer| query_matches(query, &offer.name));
    rank_offers(query, &mut offers);
    offers.truncate(8);
    Ok(offers)
}

pub(crate) fn offers_from_mmi(body: &str) -> Result<Vec<Offer>, BackendError> {
    let value: Value = serde_json::from_str(body)
        .map_err(|_| BackendError::bad_response("MMI search was not JSON"))?;
    let hits = value
        .get("result")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut offers = Vec::new();
    for hit in hits {
        let product = hit.get("product").cloned().unwrap_or(Value::Null);
        let name = field(product.get("title"))
            .or_else(|| field(product.get("erpName")))
            .unwrap_or_default();
        let Some(price) = mmi_price(hit.get("price")) else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        let product_id = product
            .get("sku")
            .or_else(|| hit.get("_id"))
            .map(id_string)
            .filter(|id| !id.is_empty())
            .unwrap_or_else(|| name.clone());
        let image = {
            let from_product = first_image(&product);
            if from_product.is_empty() {
                first_image(&hit)
            } else {
                from_product
            }
        };
        offers.push(live_offer(
            StoreId::Mmi,
            product_id,
            name,
            price,
            "AED",
            image,
        ));
        if offers.len() == 8 {
            break;
        }
    }
    Ok(offers)
}

fn search_unioncoop(query: &str) -> Result<Vec<Offer>, BackendError> {
    let home = get_text("https://www.unioncoop.ae/")?;
    let cfg = json_object_after(&home, "algoliaConfig").ok_or_else(|| {
        BackendError::unavailable(
            "Union Coop did not publish its search config (the storefront blocked this network)",
        )
    })?;
    let app = cfg
        .get("applicationId")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let key = cfg
        .get("apiKey")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let index_name = cfg.get("indexName").and_then(Value::as_str).unwrap_or("");
    if app.is_empty() || key.is_empty() || index_name.is_empty() {
        return Err(BackendError::bad_response(
            "Union Coop search config is incomplete",
        ));
    }
    let filters = cfg
        .pointer("/attributeFilter/filters")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let mut payload = json!({"query": query, "hitsPerPage": 8});
    if !filters.is_empty() {
        payload["filters"] = json!(filters);
    }
    let url = format!("https://{app}-dsn.algolia.net/1/indexes/{index_name}_products/query");
    let body = send(
        "POST",
        &url,
        Some(payload.to_string()),
        &[
            ("content-type", "application/json"),
            ("x-algolia-application-id", app),
            ("x-algolia-api-key", key),
            ("origin", "https://www.unioncoop.ae"),
            ("referer", "https://www.unioncoop.ae/"),
        ],
    )?;
    offers_from_algolia(&body)
}

pub(crate) fn offers_from_algolia(body: &str) -> Result<Vec<Offer>, BackendError> {
    let value: Value = serde_json::from_str(body)
        .map_err(|_| BackendError::bad_response("Union Coop search was not JSON"))?;
    let hits = value
        .get("hits")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut offers = Vec::new();
    for hit in hits {
        let amount = hit
            .pointer("/price/AED/default")
            .and_then(money)
            .or_else(|| hit.get("regular_price").and_then(money));
        let Some(price) = amount else {
            continue;
        };
        let name = hit
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if name.is_empty() {
            continue;
        }
        let product_id = hit
            .get("sku")
            .or_else(|| hit.get("objectID"))
            .map(id_string)
            .filter(|id| !id.is_empty())
            .unwrap_or_else(|| name.clone());
        offers.push(live_offer(
            StoreId::UnionCoop,
            product_id,
            name,
            price,
            "AED",
            first_image(&hit),
        ));
    }
    Ok(offers)
}

fn search_html(
    store: StoreId,
    url: &str,
    origin: &str,
    href_prefix: &str,
) -> Result<Vec<Offer>, BackendError> {
    let html = get_text(url)?;
    Ok(offers_from_html(store, &html, origin, href_prefix))
}

pub(crate) fn offers_from_html(
    store: StoreId,
    html: &str,
    origin: &str,
    href_prefix: &str,
) -> Vec<Offer> {
    let mut offers = offers_from_split(store, html, origin, href_prefix, "data-product-id=\"");
    if offers.is_empty() {
        offers = offers_from_split(store, html, origin, href_prefix, "js-product-wrapper");
    }
    if offers.is_empty() {
        offers = offers_from_split(store, html, origin, href_prefix, "product-name");
    }
    offers
}

fn offers_from_split(
    store: StoreId,
    html: &str,
    origin: &str,
    href_prefix: &str,
    marker: &str,
) -> Vec<Offer> {
    let mut offers = Vec::new();
    let mut seen = Vec::new();
    for block in html.split(marker).skip(1) {
        let product_id = block
            .chars()
            .take_while(|ch| ch.is_ascii_digit())
            .collect::<String>();
        let Some(href) = find_href(block, href_prefix) else {
            continue;
        };
        if seen.iter().any(|item| item == &href) {
            continue;
        }
        let Some(name) = html_name(block, href_prefix) else {
            continue;
        };
        let Some(price) = html_price(block) else {
            continue;
        };
        seen.push(href.clone());
        let id = if product_id.is_empty() {
            href.trim_end_matches('/')
                .rsplit('_')
                .next()
                .unwrap_or(&name)
                .to_string()
        } else {
            product_id
        };
        offers.push(live_offer(
            store,
            id,
            name,
            price,
            "AED",
            html_image(block, origin),
        ));
        if offers.len() == 8 {
            break;
        }
    }
    offers
}

/// Product title, never the image markup inside the first link.
///
/// Waitrose puts `<div class="intrinsic-image">` in the photo link and the
/// title in a later plain-text link. Spinneys puts the title in
/// `class="product-name"`, and the photo link is not always the first `href`.
fn html_name(block: &str, href_prefix: &str) -> Option<String> {
    if let Some(name) = plain_anchor_near(block, "product-name", href_prefix) {
        return Some(name);
    }
    if let Some(name) = plain_product_anchor(block, href_prefix) {
        return Some(name);
    }
    if let Some(name) = plain_anchor_near(block, "<h5", href_prefix) {
        return Some(name);
    }
    if let Some(name) = image_alt(block) {
        return Some(name);
    }
    let href = find_href(block, href_prefix)?;
    let slug = href
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("")
        .split('_')
        .next()
        .unwrap_or("")
        .replace('-', " ");
    let slug = slug.split_whitespace().collect::<Vec<_>>().join(" ");
    if slug.len() >= 2 && !slug.contains('<') {
        Some(slug)
    } else {
        None
    }
}

fn plain_anchor_near(block: &str, marker: &str, href_prefix: &str) -> Option<String> {
    let at = block.find(marker)?;
    let window = &block[at..];
    let window = window.get(..window.len().min(700)).unwrap_or(window);
    plain_product_anchor(window, href_prefix).or_else(|| plain_product_anchor(window, ""))
}

fn plain_product_anchor(block: &str, href_prefix: &str) -> Option<String> {
    let mut rest = block;
    while let Some(at) = rest.find("href=") {
        let from = &rest[at..];
        let Some((href, after_href)) = read_attr(from, "href") else {
            rest = &rest[at + 5..];
            continue;
        };
        if !href_prefix.is_empty() && !href_matches(&href, href_prefix) {
            rest = after_href;
            continue;
        }
        let Some(gt) = from.find('>') else {
            break;
        };
        let inner_from = &from[gt + 1..];
        let Some(close) = inner_from.find("</a>") else {
            rest = &rest[at + 5..];
            continue;
        };
        let inner = &inner_from[..close];
        if !inner.contains('<') {
            let text = inner.split_whitespace().collect::<Vec<_>>().join(" ");
            if (2..=200).contains(&text.len()) && text != "#" && !text.contains('<') {
                return Some(text);
            }
        }
        rest = &rest[at + 5..];
    }
    None
}

fn image_alt(block: &str) -> Option<String> {
    let mut rest = block;
    while let Some(at) = rest.find("<img") {
        let tag = &rest[at..];
        let end = tag.find('>').unwrap_or(tag.len());
        if let Some((alt, _)) = read_attr(&tag[..end], "alt") {
            let alt = alt.trim();
            let junk = alt.is_empty()
                || alt == "#"
                || alt.eq_ignore_ascii_case("add")
                || alt.eq_ignore_ascii_case("remove")
                || alt.eq_ignore_ascii_case("cart")
                || alt.contains('<');
            if !junk && alt.len() >= 2 {
                return Some(alt.to_string());
            }
        }
        rest = &rest[at + 4..];
    }
    None
}

fn find_href(block: &str, prefix: &str) -> Option<String> {
    let mut rest = block;
    while let Some(at) = rest.find("href=") {
        let Some((href, next)) = read_attr(&rest[at..], "href") else {
            rest = &rest[at + 5..];
            continue;
        };
        if href_matches(&href, prefix) {
            return Some(href);
        }
        rest = next;
    }
    None
}

fn href_matches(href: &str, prefix: &str) -> bool {
    if prefix.is_empty() {
        return true;
    }
    href.starts_with(prefix) || href.contains(prefix)
}

fn read_attr<'a>(from: &'a str, name: &str) -> Option<(String, &'a str)> {
    let key = format!("{name}=");
    if !from.starts_with(&key) {
        return None;
    }
    let after = &from[key.len()..];
    let quote = after.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let body = &after[1..];
    let end = body.find(quote)?;
    Some((body[..end].to_string(), &body[end + 1..]))
}

fn html_price(block: &str) -> Option<f64> {
    if let Some(price) = number_after_class(block, "price") {
        return Some(price);
    }
    if let Some(at) = block.find("AED") {
        return first_number(&block[at + 3..]);
    }
    None
}

fn number_after_class(block: &str, class_name: &str) -> Option<f64> {
    let mut rest = block;
    while let Some(at) = rest.find("class=") {
        let Some((class, _)) = read_attr(&rest[at..], "class") else {
            rest = &rest[at + 6..];
            continue;
        };
        let hit = class.split_whitespace().any(|token| token == class_name);
        if hit {
            let from = &rest[at..];
            let gt = from.find('>')?;
            if let Some(price) = first_number(&from[gt + 1..]) {
                return Some(price);
            }
        }
        rest = &rest[at + 6..];
    }
    None
}

fn first_number(text: &str) -> Option<f64> {
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.peek().copied() {
        if ch.is_ascii_digit() {
            break;
        }
        chars.next();
    }
    let number: String = chars
        .take_while(|ch| ch.is_ascii_digit() || *ch == '.')
        .collect();
    let price = number.parse::<f64>().ok()?;
    if price > 0.0 {
        Some(price)
    } else {
        None
    }
}

struct Session {
    token: String,
    user_id: String,
}

fn carrefour_login(email: &str, password: &str) -> Result<Session, BackendError> {
    let body = send(
        "POST",
        "https://www.carrefouruae.com/v2/customers/login",
        Some(
            json!({
                "email": email,
                "password": password,
                "langCode": "en",
                "storeId": "mafuae"
            })
            .to_string(),
        ),
        &[
            ("content-type", "application/json"),
            ("accept", "application/json"),
            ("appid", "Android"),
            ("storeid", "mafuae"),
            ("langcode", "en"),
        ],
    )?;
    let value: Value = serde_json::from_str(&body)
        .map_err(|_| BackendError::bad_response("Carrefour login was not JSON"))?;
    let token = value
        .get("access_token")
        .or_else(|| value.get("token"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if token.is_empty() {
        let message = value
            .pointer("/meta/message")
            .or_else(|| value.get("message"))
            .and_then(Value::as_str)
            .unwrap_or("Carrefour did not return a session");
        return Err(BackendError::unavailable(message));
    }
    let user_id = value
        .get("user_id")
        .or_else(|| value.get("userId"))
        .map(id_string)
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| email.to_string());
    Ok(Session { token, user_id })
}

fn magento_token(
    url: &str,
    username: &str,
    password: &str,
    store: &str,
) -> Result<String, BackendError> {
    let body = send(
        "POST",
        url,
        Some(json!({"username": username, "password": password}).to_string()),
        &[
            ("content-type", "application/json"),
            ("accept", "application/json"),
        ],
    )?;
    let token = body.trim().trim_matches('"').to_string();
    if token.is_empty() || token.starts_with('{') || token.starts_with('<') {
        return Err(BackendError::unavailable(format!(
            "{store} did not return a session"
        )));
    }
    Ok(token)
}

fn mmi_login(ident: &str, password: &str) -> Result<String, BackendError> {
    let payload = if ident.contains('@') {
        json!({"email": ident, "password": password})
    } else {
        json!({"phone": ident, "password": password})
    };
    let body = send(
        "POST",
        "https://api.mmiprod.retter.io/o9pki8qf/CALL/User/loginWithDxbLicense",
        Some(payload.to_string()),
        &[
            ("content-type", "application/json"),
            ("accept", "application/json"),
            ("channel-id", "mmiDubai"),
            ("client-type", "web"),
        ],
    )?;
    let value: Value = serde_json::from_str(&body)
        .map_err(|_| BackendError::bad_response("MMI login was not JSON"))?;
    let custom = value
        .pointer("/data/token")
        .or_else(|| value.get("token"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if custom.is_empty() {
        return Err(BackendError::unavailable(
            value
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("MMI did not return a session"),
        ));
    }
    let exchanged = send(
        "POST",
        "https://api.mmiprod.retter.io/o9pki8qf/TOKEN/auth",
        Some(json!({"customToken": custom}).to_string()),
        &[
            ("content-type", "application/json"),
            ("accept", "application/json"),
            ("channel-id", "mmiDubai"),
            ("client-type", "web"),
        ],
    )?;
    let token_body: Value = serde_json::from_str(&exchanged)
        .map_err(|_| BackendError::bad_response("MMI session was not JSON"))?;
    let access = token_body
        .pointer("/data/accessToken")
        .or_else(|| token_body.get("accessToken"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if access.is_empty() {
        return Err(BackendError::unavailable(
            "MMI session token was not issued",
        ));
    }
    Ok(access)
}

fn african_login(email: &str, password: &str) -> Result<String, BackendError> {
    let body = graphql(
        "https://www.africaneasternonline.com/graphql",
        "",
        "mutation getAuthTokenDxb($email: String!, $password: String!) { customerTokenForLicenseDXB(email: $email, password: $password) { token } }",
        json!({"email": email, "password": password}),
        "African & Eastern",
    )?;
    let value: Value = serde_json::from_str(&body)
        .map_err(|_| BackendError::bad_response("African & Eastern login was not JSON"))?;
    let token = value
        .pointer("/data/customerTokenForLicenseDXB/token")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if token.is_empty() {
        return Err(BackendError::unavailable(
            graphql_error(&value).unwrap_or("African & Eastern login failed"),
        ));
    }
    Ok(token)
}

fn push_grandiose(token: &str, cart: &Cart) -> Result<(), BackendError> {
    let current = graphql(
        "https://www.grandiose.ae/graphql",
        token,
        "query { customerCart { id } }",
        json!({}),
        "Grandiose",
    )?;
    let value: Value = serde_json::from_str(&current)
        .map_err(|_| BackendError::bad_response("Grandiose cart was not JSON"))?;
    let cart_id = value
        .pointer("/data/customerCart/id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if cart_id.is_empty() {
        return Err(BackendError::unavailable("Grandiose cart is missing"));
    }
    for line in &cart.lines {
        let added = graphql(
            "https://www.grandiose.ae/graphql",
            token,
            "mutation Add($cartId: String!, $sku: String!, $qty: Float!) { addProductsToCart(cartId: $cartId, cartItems: [{ sku: $sku, quantity: $qty }]) { user_errors { message } } }",
            json!({"cartId": cart_id, "sku": line.product_id, "qty": line.qty}),
            "Grandiose",
        )?;
        let added: Value = serde_json::from_str(&added)
            .map_err(|_| BackendError::bad_response("Grandiose add was not JSON"))?;
        if let Some(message) = added
            .pointer("/data/addProductsToCart/user_errors/0/message")
            .and_then(Value::as_str)
        {
            return Err(BackendError::bad_response(message));
        }
    }
    Ok(())
}

fn push_unioncoop(token: &str, cart: &Cart) -> Result<(), BackendError> {
    let current = send(
        "GET",
        "https://www.unioncoop.ae/rest/V1/carts/mine",
        None,
        &[
            ("accept", "application/json"),
            ("authorization", &format!("Bearer {token}")),
        ],
    )?;
    let value: Value = serde_json::from_str(&current)
        .map_err(|_| BackendError::bad_response("Union Coop cart was not JSON"))?;
    let quote = value
        .get("id")
        .map(id_string)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| BackendError::unavailable("Union Coop cart is missing"))?;
    for line in &cart.lines {
        let _ = send(
            "POST",
            "https://www.unioncoop.ae/rest/V1/carts/mine/items",
            Some(
                json!({
                    "cartItem": {
                        "sku": line.product_id,
                        "qty": line.qty,
                        "quote_id": quote
                    }
                })
                .to_string(),
            ),
            &[
                ("content-type", "application/json"),
                ("accept", "application/json"),
                ("authorization", &format!("Bearer {token}")),
            ],
        )?;
    }
    Ok(())
}

fn graphql(
    url: &str,
    token: &str,
    query: &str,
    variables: Value,
    store: &str,
) -> Result<String, BackendError> {
    let mut headers = vec![
        ("content-type", "application/json"),
        ("accept", "application/json"),
    ];
    let auth;
    if !token.is_empty() {
        auth = format!("Bearer {token}");
        headers.push(("authorization", auth.as_str()));
    }
    let body = json!({"query": query, "variables": variables}).to_string();
    let text = send("POST", url, Some(body), &headers)?;
    let value: Value = serde_json::from_str(&text)
        .map_err(|_| BackendError::bad_response(format!("{store} response was not JSON")))?;
    if value.get("data").is_none() {
        if let Some(message) = graphql_error(&value) {
            return Err(BackendError::bad_response(format!("{store}: {message}")));
        }
    }
    Ok(text)
}

fn graphql_error(value: &Value) -> Option<&str> {
    value.pointer("/errors/0/message").and_then(Value::as_str)
}

fn get_text(url: &str) -> Result<String, BackendError> {
    send(
        "GET",
        url,
        None,
        &[("accept", "text/html,application/json")],
    )
}

fn send(
    method: &str,
    url: &str,
    body: Option<String>,
    headers: &[(&str, &str)],
) -> Result<String, BackendError> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(20))
        .user_agent(UA)
        .build()
        .map_err(|err| BackendError::unavailable(err.to_string()))?;
    let mut request = match method {
        "POST" => client.post(url),
        "GET" => client.get(url),
        "DELETE" => client.delete(url),
        _ => return Err(BackendError::unsupported("unsupported store method")),
    };
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    if let Some(body) = body {
        request = request.body(body);
    }
    let response = request
        .send()
        .map_err(|err| BackendError::unavailable(err.to_string()))?;
    let status = response.status().as_u16();
    let text = response
        .text()
        .map_err(|err| BackendError::unavailable(err.to_string()))?;
    if !(200..300).contains(&status) {
        return Err(blocked_or_status(status, &text));
    }
    if is_blocked_body(&text) {
        return Err(BackendError::unavailable(
            "the storefront blocked this network",
        ));
    }
    Ok(text)
}

fn blocked_or_status(status: u16, body: &str) -> BackendError {
    if status == 403 || status == 405 || is_blocked_body(body) {
        BackendError::unavailable(format!(
            "the storefront blocked this network (HTTP {status})"
        ))
    } else {
        BackendError::unavailable(format!("store returned HTTP {status}"))
    }
}

fn is_blocked_body(body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    lower.contains("403 forbidden")
        || lower.contains("access denied")
        || lower.contains("error 54113")
        || (lower.contains("varnish") && lower.contains("not allowed"))
}

fn live_offer(
    store: StoreId,
    product_id: String,
    name: String,
    price: f64,
    currency: &str,
    image_url: impl Into<String>,
) -> Offer {
    let unit = pack_unit(&name);
    Offer {
        store,
        product_id,
        name,
        price,
        currency: currency.to_uppercase(),
        unit,
        source: OfferSource::Live,
        image_url: image_url.into(),
    }
}

/// Best catalog picture for a receipt name. The picture has to belong to a
/// name that contains the query, so a snack that merely shares one word is skipped.
pub fn product_image(store: StoreId, query: &str) -> Result<Option<String>, BackendError> {
    let offers = search(store, query)?;
    Ok(pick_image(query, &offers))
}

pub(crate) fn pick_image(query: &str, offers: &[Offer]) -> Option<String> {
    let mut ranked = offers.to_vec();
    rank_offers(query, &mut ranked);
    ranked.into_iter().find_map(|offer| {
        if !image_name_ok(query, &offer.name) {
            return None;
        }
        let url = offer.image_url.trim();
        if url.is_empty() {
            None
        } else {
            Some(url.to_string())
        }
    })
}

fn image_name_ok(query: &str, name: &str) -> bool {
    let query_tokens = significant_tokens(query);
    let name_tokens = significant_tokens(name);
    if query_tokens.is_empty() || name_tokens.is_empty() {
        return false;
    }
    if !query_tokens
        .iter()
        .all(|token| name_tokens.iter().any(|word| word == token))
    {
        return false;
    }
    let query_is_snack = query_tokens.iter().any(|token| snack_token(token));
    if !query_is_snack && name_tokens.iter().any(|token| snack_token(token)) {
        return false;
    }
    true
}

fn snack_token(token: &str) -> bool {
    matches!(
        token,
        "oreo"
            | "cookie"
            | "cookies"
            | "chocolate"
            | "biscuit"
            | "biscuits"
            | "candy"
            | "wafer"
            | "wafers"
            | "cake"
            | "cakes"
            | "cocoa"
            | "doughnut"
            | "donut"
    )
}

fn first_image(value: &Value) -> String {
    const KEYS: &[&str] = &[
        "image_url",
        "imageUrl",
        "image",
        "thumbnail",
        "thumbnail_url",
        "small_image",
        "smallImage",
        "media_url",
    ];
    for key in KEYS {
        if let Some(child) = value.get(*key) {
            let url = http_url(child);
            if !url.is_empty() {
                return url;
            }
            if let Some(items) = child.as_array() {
                for item in items {
                    let url = http_url(item);
                    if !url.is_empty() {
                        return url;
                    }
                }
            }
        }
    }
    String::new()
}

fn http_url(value: &Value) -> String {
    if let Some(url) = value.as_str() {
        if url.starts_with("http://") || url.starts_with("https://") {
            return url.to_string();
        }
    }
    if let Some(url) = value.get("url").and_then(Value::as_str) {
        if url.starts_with("http://") || url.starts_with("https://") {
            return url.to_string();
        }
    }
    String::new()
}

fn html_image(block: &str, origin: &str) -> String {
    let mut rest = block;
    while let Some(at) = rest.find("<img") {
        let tag = &rest[at..];
        let end = tag.find('>').unwrap_or(tag.len());
        let tag = &tag[..end];
        for attr in ["src", "data-src"] {
            let key = format!("{attr}=");
            if let Some(pos) = tag.find(&key) {
                if let Some((src, _)) = read_attr(&tag[pos..], attr) {
                    if let Some(url) = absolute_image(src.trim(), origin) {
                        return url;
                    }
                }
            }
        }
        rest = &rest[at + 4..];
    }
    String::new()
}

fn absolute_image(src: &str, origin: &str) -> Option<String> {
    let lower = src.to_ascii_lowercase();
    if lower.starts_with("data:") || lower.contains("logo") || lower.ends_with(".svg") {
        return None;
    }
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return Some(src.to_string());
    }
    if let Some(rest) = src.strip_prefix("//") {
        return Some(format!("https://{rest}"));
    }
    if src.starts_with('/') {
        return Some(format!("{origin}{src}"));
    }
    None
}

/// Token overlap used by Bring Fast `_score_name`. A token has to be longer
/// than two characters, so "Baileys" does not match "milk".
pub(crate) fn query_matches(query: &str, name: &str) -> bool {
    let query_tokens = significant_tokens(query);
    let name_tokens = significant_tokens(name);
    if query_tokens.is_empty() {
        let needle = query.trim().to_ascii_lowercase();
        return !needle.is_empty() && name.to_ascii_lowercase().contains(&needle);
    }
    query_tokens
        .iter()
        .any(|token| name_tokens.iter().any(|word| word == token))
}

pub(crate) fn rank_offers(query: &str, offers: &mut [Offer]) {
    let scores: Vec<f64> = offers
        .iter()
        .map(|offer| relevance(query, &offer.name))
        .collect();
    let mut order: Vec<usize> = (0..offers.len()).collect();
    order.sort_by(|&a, &b| {
        scores[b]
            .partial_cmp(&scores[a])
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.cmp(&b))
    });
    let ranked: Vec<Offer> = order
        .into_iter()
        .map(|index| offers[index].clone())
        .collect();
    offers.clone_from_slice(&ranked);
}

fn relevance(query: &str, name: &str) -> f64 {
    let query_tokens = significant_tokens(query);
    let name_tokens = significant_tokens(name);
    if query_tokens.is_empty() || name_tokens.is_empty() {
        return 0.0;
    }
    let hit = query_tokens
        .iter()
        .filter(|token| name_tokens.iter().any(|word| word == *token))
        .count();
    if hit == 0 {
        return 0.0;
    }
    let mut score = hit as f64 / query_tokens.len() as f64;
    if hit == query_tokens.len() {
        score += 0.5;
    }
    if let Some(head) = head_noun(&name_tokens) {
        if query_tokens.iter().any(|token| token == &head) {
            score += 1.2;
        }
    }
    let snacks = [
        "oreo",
        "cookie",
        "cookies",
        "chocolate",
        "biscuit",
        "biscuits",
        "candy",
        "wafer",
        "wafers",
        "cake",
        "cakes",
        "cocoa",
        "doughnut",
        "donut",
    ];
    let query_is_snack = query_tokens
        .iter()
        .any(|token| snacks.contains(&token.as_str()));
    if !query_is_snack
        && name_tokens
            .iter()
            .any(|token| snacks.contains(&token.as_str()))
    {
        score -= 1.0;
    }
    if query_tokens.iter().any(|token| token == "milk") {
        for word in ["fresh", "full", "skimmed", "skim", "lactose", "uht"] {
            if name_tokens.iter().any(|token| token == word) {
                score += 0.15;
            }
        }
    }
    let extra = name_tokens.len().saturating_sub(hit);
    score -= extra as f64 * 0.05;
    score
}

fn head_noun(tokens: &[String]) -> Option<String> {
    tokens
        .iter()
        .rev()
        .find(|token| !matches!(token.as_str(), "ml" | "kg" | "pack" | "pcs" | "ltr"))
        .cloned()
}

fn significant_tokens(value: &str) -> Vec<String> {
    value
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .map(|token| token.to_ascii_lowercase())
        .filter(|token| token.len() > 2)
        .collect()
}

/// Pack size from the product title: `2 L`, `200 ml`, `1 kg`, `500 g`, or `pack`.
pub(crate) fn pack_unit(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index].is_ascii_digit() {
            let start = index;
            while index < bytes.len()
                && (bytes[index].is_ascii_digit() || bytes[index] == b'.' || bytes[index] == b',')
            {
                index += 1;
            }
            let number = lower[start..index].trim_end_matches('.').replace(',', "");
            while index < bytes.len() && bytes[index] == b' ' {
                index += 1;
            }
            if let Some((label, consumed, scale)) = unit_suffix(&lower[index..]) {
                index += consumed;
                if let Ok(value) = number.parse::<f64>() {
                    let value = value * scale;
                    return format_measure(value, label);
                }
            }
        } else {
            index += 1;
        }
    }
    if lower.contains("pack") {
        "pack".into()
    } else {
        "piece".into()
    }
}

fn unit_suffix(text: &str) -> Option<(&'static str, usize, f64)> {
    const UNITS: &[(&str, &str, f64)] = &[
        ("millilitre", "ml", 1.0),
        ("milliliter", "ml", 1.0),
        ("litres", "L", 1.0),
        ("liters", "L", 1.0),
        ("litre", "L", 1.0),
        ("liter", "L", 1.0),
        ("ltr", "L", 1.0),
        ("ml", "ml", 1.0),
        ("kg", "kg", 1.0),
        ("cl", "ml", 10.0),
        ("g", "g", 1.0),
        ("l", "L", 1.0),
    ];
    for (raw, label, scale) in UNITS {
        if let Some(rest) = text.strip_prefix(raw) {
            if rest
                .chars()
                .next()
                .is_some_and(|ch| ch.is_ascii_alphabetic())
            {
                continue;
            }
            return Some((label, raw.len(), *scale));
        }
    }
    None
}

fn format_measure(value: f64, label: &str) -> String {
    let number = if (value - value.round()).abs() < 0.001 {
        format!("{}", value.round() as i64)
    } else {
        let text = format!("{value:.2}");
        text.trim_end_matches('0').trim_end_matches('.').to_string()
    };
    format!("{number} {label}")
}

fn money(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.replace(',', "").trim().parse().ok(),
        _ => None,
    }
}

fn id_string(value: &Value) -> String {
    match value {
        Value::String(text) => text.trim().to_string(),
        Value::Number(number) => number.to_string(),
        _ => String::new(),
    }
}

fn field(value: Option<&Value>) -> Option<String> {
    let value = value?;
    match value {
        Value::String(text) => Some(text.trim().to_string()).filter(|text| !text.is_empty()),
        Value::Array(items) => field(items.first()),
        Value::Object(map) => field(map.get("value")),
        _ => None,
    }
}

fn mmi_price(value: Option<&Value>) -> Option<f64> {
    let price = value?.as_object()?;
    for key in [
        "salePrice",
        "listPrice",
        "officialSalePrice",
        "officialListPrice",
    ] {
        if let Some(raw) = price.get(key).and_then(money) {
            if raw > 0.0 {
                return Some((raw / 100_000.0 * 100.0).round() / 100.0);
            }
        }
    }
    None
}

fn json_object_after(text: &str, marker: &str) -> Option<Value> {
    let start = text.find(marker)?;
    let brace = text[start..].find('{')? + start;
    let mut depth = 0i32;
    for (index, ch) in text[brace..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return serde_json::from_str(&text[brace..=brace + index]).ok();
                }
            }
            _ => {}
        }
    }
    None
}

fn urlencode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            b' ' => out.push_str("%20"),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_carrefour_constructor_results() {
        let body = r#"{"response":{"results":[{"value":"Almarai Full Fat Fresh Milk, 2L","data":{"id":"123","price":12.49,"currency":"AED"}}]}}"#;
        let offers = offers_from_carrefour(body).unwrap();
        assert_eq!(offers[0].name, "Almarai Full Fat Fresh Milk, 2L");
        assert_eq!(offers[0].price, 12.49);
        assert_eq!(offers[0].unit, "2 L");
        assert_eq!(offers[0].store, StoreId::Carrefour);
        assert_eq!(offers[0].source, OfferSource::Live);
    }

    #[test]
    fn parses_magento_and_mmi_and_algolia() {
        let magento = r#"{"data":{"products":{"items":[{"sku":"7622201765231","name":"Oreo Cookies Milk","price_range":{"minimum_price":{"regular_price":{"value":1.65,"currency":"AED"}}}}]}}}"#;
        let offers = offers_from_magento_products(StoreId::Grandiose, magento).unwrap();
        assert_eq!(offers[0].product_id, "7622201765231");
        assert_eq!(offers[0].price, 1.65);
        let mmi = r#"{"result":[{"_id":"E0390","product":{"sku":"E0390","title":{"value":"Cloudy Bay"}},"price":{"salePrice":14432234}}]}"#;
        let offers = offers_from_mmi(mmi).unwrap();
        assert_eq!(offers[0].name, "Cloudy Bay");
        assert_eq!(offers[0].price, 144.32);
        let algolia =
            r#"{"hits":[{"sku":"UC1","name":"Almarai Milk 1L","price":{"AED":{"default":6.75}}}]}"#;
        let offers = offers_from_algolia(algolia).unwrap();
        assert_eq!(offers[0].price, 6.75);
        assert_eq!(offers[0].store, StoreId::UnionCoop);
    }

    #[test]
    fn parses_storefront_html() {
        let html = r#"<div data-product-id="55"><a href="/en/products/fresh-milk_55">Fresh Milk</a> AED 8.50</div>"#;
        let offers = offers_from_html(
            StoreId::Waitrose,
            html,
            "https://www.waitrose.ae",
            "/en/products/",
        );
        assert_eq!(offers[0].name, "Fresh Milk");
        assert_eq!(offers[0].price, 8.50);
        assert_eq!(offers[0].product_id, "55");
        assert_eq!(offers[0].unit, "piece");
    }

    #[test]
    fn waitrose_name_skips_the_image_markup() {
        let html = r##"
<div data-product-id="1135">
  <a href="/en/products/almarai-full-fat-milk-2ltr_1135/"><div class="intrinsic-image main"><img alt="#"></div></a>
  <h5><a href="/en/products/almarai-full-fat-milk-2ltr_1135/">Almarai full fat milk 2ltr</a></h5>
  <span>AED 15.75</span>
</div>"##;
        let offers = offers_from_html(
            StoreId::Waitrose,
            html,
            "https://www.waitrose.ae",
            "/en/products/",
        );
        assert_eq!(offers.len(), 1);
        assert_eq!(offers[0].name, "Almarai full fat milk 2ltr");
        assert!(!offers[0].name.contains('<'));
        assert_eq!(offers[0].price, 15.75);
        assert_eq!(offers[0].unit, "2 L");
    }

    #[test]
    fn spinneys_uses_product_name_not_the_first_link() {
        let html = r##"
<div class="js-product-wrapper" data-product-id="335">
  <a href="/en-ae/account/wishlist/">saved</a>
  <a href="/en-ae/catalogue/milupa-aptamil-growing-up-toddler-milk-stage-3-200ml_335/">
    <img alt="#">
  </a>
  <span class="price">12.00</span>
  <p class="product-name">
    <a href="/en-ae/catalogue/milupa-aptamil-growing-up-toddler-milk-stage-3-200ml_335/">Aptamil Growing Up Milk Formula 1-3 Years 200ml</a>
  </p>
</div>"##;
        let offers = offers_from_html(
            StoreId::Spinneys,
            html,
            "https://www.spinneys.com",
            "/en-ae/catalogue/",
        );
        assert_eq!(offers.len(), 1);
        assert!(offers[0].name.contains("Aptamil"));
        assert!(!offers[0].name.contains('<'));
        assert_eq!(offers[0].price, 12.0);
        assert_eq!(offers[0].product_id, "335");
        assert_eq!(offers[0].unit, "200 ml");
    }

    #[test]
    fn liquor_catalogues_have_to_match_the_query() {
        assert!(!query_matches("milk", "Baileys Original"));
        assert!(!query_matches("milk", "Chivas Regal 12"));
        assert!(query_matches("milk", "Almarai Full Fat Fresh Milk 2L"));
        assert!(query_matches("milk", "Oreo Cookies Milk"));
    }

    #[test]
    fn fresh_milk_ranks_ahead_of_oreo_and_chocolate() {
        let mut offers = vec![
            live_offer(
                StoreId::Grandiose,
                "1".into(),
                "Oreo Cookies Milk".into(),
                1.65,
                "AED",
                "https://cdn.example/oreo.jpg",
            ),
            live_offer(
                StoreId::Grandiose,
                "2".into(),
                "Galaxy Chocolate".into(),
                8.0,
                "AED",
                "",
            ),
            live_offer(
                StoreId::Grandiose,
                "3".into(),
                "Almarai Full Fat Fresh Milk 2L".into(),
                12.49,
                "AED",
                "https://cdn.example/milk.jpg",
            ),
        ];
        rank_offers("milk", &mut offers);
        assert_eq!(offers[0].name, "Almarai Full Fat Fresh Milk 2L");
        assert_eq!(offers[0].unit, "2 L");
        offers.retain(|offer| query_matches("milk", &offer.name));
        assert!(offers
            .iter()
            .all(|offer| offer.name.to_lowercase().contains("milk")));
        assert!(offers[0].name.contains("Fresh Milk"));
    }

    #[test]
    fn pack_units_cover_weight_and_packs() {
        assert_eq!(pack_unit("Rice 5kg"), "5 kg");
        assert_eq!(pack_unit("Coffee 250g"), "250 g");
        assert_eq!(pack_unit("Oasis Blu 1L Pack of 6"), "1 L");
        assert_eq!(pack_unit("Still water pack of 6"), "pack");
        assert_eq!(pack_unit("White bread"), "piece");
    }

    #[test]
    fn catalog_image_follows_the_matching_product() {
        let body = r#"{"response":{"results":[{"value":"Almarai Milk","data":{"id":"1","price":12.5,"currency":"AED","image_url":"https://cdn.example/milk.jpg"}}]}}"#;
        let offers = offers_from_carrefour(body).unwrap();
        assert_eq!(offers[0].image_url, "https://cdn.example/milk.jpg");
        let html = r#"<div data-product-id="335">
  <img src="/media/aptamil.jpg" alt="Aptamil">
  <span class="price">12.00</span>
  <a href="/en-ae/catalogue/milupa-aptamil-growing-up-toddler-milk-stage-3-200ml_335/">Aptamil Growing Up Milk Formula 1-3 Years 200ml</a>
</div>"#;
        let offers = offers_from_html(
            StoreId::Spinneys,
            html,
            "https://www.spinneys.com",
            "/en-ae/catalogue/",
        );
        assert_eq!(
            offers[0].image_url,
            "https://www.spinneys.com/media/aptamil.jpg"
        );
        let ranked = vec![
            live_offer(
                StoreId::Carrefour,
                "1".into(),
                "Oreo Cookies Milk".into(),
                1.0,
                "AED",
                "https://cdn.example/oreo.jpg",
            ),
            live_offer(
                StoreId::Carrefour,
                "2".into(),
                "Almarai Full Fat Fresh Milk 2L".into(),
                12.0,
                "AED",
                "https://cdn.example/milk.jpg",
            ),
        ];
        assert_eq!(
            pick_image("Almarai Full Fat Fresh Milk", &ranked).as_deref(),
            Some("https://cdn.example/milk.jpg")
        );
        assert!(pick_image("milk", &ranked[..1]).is_none());
    }
}

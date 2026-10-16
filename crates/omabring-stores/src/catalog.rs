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
        r#"query Search($q: String!, $n: Int!) { products(search: $q, pageSize: $n) { items { sku name price_range { minimum_price { regular_price { value currency } } } } } }"#,
        json!({"q": query, "n": 8}),
        "Grandiose",
    )?;
    offers_from_magento_products(StoreId::Grandiose, &body)
}

fn search_african(query: &str) -> Result<Vec<Offer>, BackendError> {
    let body = graphql(
        "https://www.africaneasternonline.com/graphql",
        "",
        r#"query ($q: String!, $n: Int!) { products(search: $q, pageSize: $n) { items { sku name price_range { minimum_price { regular_price { value currency } } } } } }"#,
        json!({"q": query, "n": 8}),
        "African & Eastern",
    )?;
    offers_from_magento_products(StoreId::AfricanEastern, &body)
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
        offers.push(live_offer(store, product_id, name, price, currency));
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
    offers_from_mmi(&body)
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
        offers.push(live_offer(StoreId::Mmi, product_id, name, price, "AED"));
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
    let mut offers = Vec::new();
    let mut seen = Vec::new();
    for block in html.split("data-product-id=\"").skip(1) {
        let product_id = block
            .chars()
            .take_while(|ch| ch.is_ascii_digit())
            .collect::<String>();
        let Some(href_at) = block.find("href=\"") else {
            continue;
        };
        let href_rest = &block[href_at + 6..];
        let Some(end) = href_rest.find('"') else {
            continue;
        };
        let href = &href_rest[..end];
        if !href.starts_with(href_prefix) || seen.iter().any(|item| item == href) {
            continue;
        }
        seen.push(href.to_string());
        let name = html_name(block).unwrap_or_else(|| {
            href.trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or(href)
                .to_string()
        });
        let price = html_price(block);
        let Some(price) = price else {
            continue;
        };
        if name.trim().is_empty() {
            continue;
        }
        let id = if product_id.is_empty() {
            href.trim_end_matches('/')
                .rsplit('_')
                .next()
                .unwrap_or(&name)
                .to_string()
        } else {
            product_id
        };
        offers.push(live_offer(store, id, name, price, "AED"));
        if offers.len() == 8 {
            break;
        }
    }
    let _ = origin;
    offers
}

fn html_name(block: &str) -> Option<String> {
    if let Some(anchor) = text_of_first_anchor(block) {
        return Some(anchor);
    }
    for marker in ["product-name\">", "<h5"] {
        if let Some(at) = block.find(marker) {
            if let Some(anchor) = text_of_first_anchor(&block[at..]) {
                return Some(anchor);
            }
        }
    }
    if let Some(at) = block.find("alt=\"") {
        let rest = &block[at + 5..];
        if let Some(end) = rest.find('"') {
            let name = rest[..end].trim();
            if name.len() >= 2 {
                return Some(name.to_string());
            }
        }
    }
    None
}

fn text_of_first_anchor(block: &str) -> Option<String> {
    let start = block.find("<a")?;
    let rest = &block[start..];
    let open_end = rest.find('>')?;
    let close = rest.find("</a>")?;
    if close <= open_end {
        return None;
    }
    let cleaned = rest[open_end + 1..close]
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if cleaned.len() >= 2 {
        Some(cleaned)
    } else {
        None
    }
}

fn html_price(block: &str) -> Option<f64> {
    if let Some(at) = block.find("AED") {
        let rest = block[at + 3..].trim_start();
        let number: String = rest
            .chars()
            .skip_while(|ch| !ch.is_ascii_digit())
            .take_while(|ch| ch.is_ascii_digit() || *ch == '.')
            .collect();
        if let Ok(price) = number.parse::<f64>() {
            return Some(price);
        }
    }
    if let Some(at) = block.find("class=\"price\"") {
        let rest = &block[at..];
        if let Some(start) = rest.find('>') {
            let number: String = rest[start + 1..]
                .chars()
                .skip_while(|ch| ch.is_whitespace())
                .take_while(|ch| ch.is_ascii_digit() || *ch == '.')
                .collect();
            if let Ok(price) = number.parse::<f64>() {
                return Some(price);
            }
        }
    }
    None
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
) -> Offer {
    Offer {
        store,
        product_id,
        name,
        price,
        currency: currency.to_uppercase(),
        unit: "piece".into(),
        source: OfferSource::Live,
    }
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
    }
}

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use chrono::Utc;
use omabring_core::{
    App, AuthzPoll, ClientAuth, Error, SpendRange, StoreId, StoreSecrets, VERSION,
};
use omabring_stores::Registry;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Clone)]
pub struct Server {
    pub app: Arc<App>,
    pub backend: Arc<Registry>,
    pub issuer: String,
}

pub fn router(server: Server) -> Router {
    Router::new()
        .route("/health", get(health))
        .route(
            "/.well-known/oauth-authorization-server",
            get(authorization_metadata),
        )
        .route(
            "/.well-known/oauth-protected-resource",
            get(resource_metadata),
        )
        .route("/oauth/register", post(register_client))
        .route("/oauth/authorize", get(authorize))
        .route("/oauth/token", post(token))
        .route("/oauth/revoke", post(revoke))
        .route("/mcp", post(mcp))
        .with_state(server)
}

async fn health() -> Json<Value> {
    Json(json!({ "service": "omabring", "version": VERSION }))
}

async fn authorization_metadata(State(server): State<Server>) -> Json<Value> {
    let issuer = server.issuer.trim_end_matches('/');
    Json(json!({
        "issuer": issuer,
        "authorization_endpoint": format!("{issuer}/oauth/authorize"),
        "token_endpoint": format!("{issuer}/oauth/token"),
        "registration_endpoint": format!("{issuer}/oauth/register"),
        "revocation_endpoint": format!("{issuer}/oauth/revoke"),
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["none", "client_secret_post", "client_secret_basic"],
        "scopes_supported": ["omabring"]
    }))
}

async fn resource_metadata(State(server): State<Server>) -> Json<Value> {
    let issuer = server.issuer.trim_end_matches('/');
    Json(json!({
        "resource": format!("{issuer}/mcp"),
        "authorization_servers": [issuer],
        "bearer_methods_supported": ["header"],
        "scopes_supported": ["omabring"]
    }))
}

#[derive(Deserialize)]
struct RegisterBody {
    client_name: String,
    redirect_uris: Vec<String>,
    #[serde(default)]
    token_endpoint_auth_method: String,
}

async fn register_client(State(server): State<Server>, Json(body): Json<RegisterBody>) -> Response {
    let app = server.app.clone();
    let result = tokio::task::spawn_blocking(move || {
        app.register_client(
            &body.client_name,
            &body.redirect_uris,
            &body.token_endpoint_auth_method,
        )
    })
    .await;
    match result {
        Ok(Ok(client)) => (
            StatusCode::CREATED,
            Json(json!({
                "client_id": client.client_id,
                "client_secret": client.client_secret,
                "client_name": client.client_name,
                "redirect_uris": client.redirect_uris,
                "token_endpoint_auth_method": client.token_endpoint_auth_method,
                "grant_types": ["authorization_code", "refresh_token"],
                "response_types": ["code"],
            })),
        )
            .into_response(),
        Ok(Err(err)) => oauth_error(&err),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

#[derive(Deserialize)]
struct AuthorizeQuery {
    client_id: String,
    redirect_uri: String,
    code_challenge: String,
    code_challenge_method: String,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    response_type: Option<String>,
}

async fn authorize(State(server): State<Server>, Query(query): Query<AuthorizeQuery>) -> Response {
    if query.response_type.as_deref().unwrap_or("code") != "code" {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let app = server.app.clone();
    let started = tokio::task::spawn_blocking({
        let app = app.clone();
        let query_client = query.client_id.clone();
        let redirect = query.redirect_uri.clone();
        let challenge = query.code_challenge.clone();
        let method = query.code_challenge_method.clone();
        let state = query.state.clone();
        let scope = query.scope.clone();
        move || {
            app.start_authorization(
                &query_client,
                &redirect,
                &challenge,
                &method,
                state.as_deref(),
                scope.as_deref(),
            )
        }
    })
    .await;
    let pending_id = match started {
        Ok(Ok(id)) => id,
        Ok(Err(err)) => return oauth_error(&err),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(120);
    loop {
        if tokio::time::Instant::now() > deadline {
            return (
                StatusCode::REQUEST_TIMEOUT,
                "Approve the request in OmaBring",
            )
                .into_response();
        }
        let app = app.clone();
        let id = pending_id.clone();
        let poll = tokio::task::spawn_blocking(move || app.poll_authorization(&id)).await;
        match poll {
            Ok(Ok(AuthzPoll::Pending)) => {
                tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            }
            Ok(Ok(AuthzPoll::Ready { location })) | Ok(Ok(AuthzPoll::Denied { location })) => {
                return axum::response::Redirect::temporary(&location).into_response();
            }
            Ok(Ok(AuthzPoll::Expired)) => {
                return (StatusCode::BAD_REQUEST, "Authorization request expired").into_response();
            }
            _ => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        }
    }
}

async fn token(State(server): State<Server>, headers: HeaderMap, body: Body) -> Response {
    let bytes = match axum::body::to_bytes(body, 64 * 1024).await {
        Ok(bytes) => bytes,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    let form = parse_form(&bytes);
    let mut client = client_from_form(&form);
    if let Some(basic) = basic_client(&headers) {
        client = basic;
    }
    let grant = form.get("grant_type").cloned().unwrap_or_default();
    let app = server.app.clone();
    let result = tokio::task::spawn_blocking(move || match grant.as_str() {
        "authorization_code" => app.exchange_code(
            form.get("code").map(String::as_str).unwrap_or(""),
            form.get("redirect_uri").map(String::as_str).unwrap_or(""),
            form.get("code_verifier").map(String::as_str).unwrap_or(""),
            &client,
        ),
        "refresh_token" => app.refresh_token(
            form.get("refresh_token").map(String::as_str).unwrap_or(""),
            &client,
        ),
        _ => Err(Error::oauth(
            "unsupported_grant_type",
            "grant_type is not supported",
        )),
    })
    .await;
    match result {
        Ok(Ok(tokens)) => Json(json!({
            "access_token": tokens.access_token,
            "refresh_token": tokens.refresh_token,
            "token_type": tokens.token_type,
            "expires_in": tokens.expires_in,
            "scope": tokens.scope,
        }))
        .into_response(),
        Ok(Err(err)) => oauth_error(&err),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn revoke(State(server): State<Server>, headers: HeaderMap, body: Body) -> Response {
    let bytes = match axum::body::to_bytes(body, 64 * 1024).await {
        Ok(bytes) => bytes,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    let form = parse_form(&bytes);
    let mut client = client_from_form(&form);
    if let Some(basic) = basic_client(&headers) {
        client = basic;
    }
    let token = form.get("token").cloned().unwrap_or_default();
    let app = server.app.clone();
    let result = tokio::task::spawn_blocking(move || app.revoke_token(&token, &client)).await;
    match result {
        Ok(Ok(())) => StatusCode::OK.into_response(),
        Ok(Err(err)) => oauth_error(&err),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn mcp(State(server): State<Server>, headers: HeaderMap, body: Body) -> Response {
    let bytes = match axum::body::to_bytes(body, 1024 * 1024).await {
        Ok(bytes) => bytes,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    let request: Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(_) => {
            return Json(rpc_error(Value::Null, -32700, "parse error")).into_response();
        }
    };
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let method = request.get("method").and_then(|v| v.as_str()).unwrap_or("");
    let is_notification = request.get("id").is_none();
    if method == "initialize" {
        return Json(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "protocolVersion": "2025-03-26",
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": "OmaBring", "version": VERSION }
            }
        }))
        .into_response();
    }
    if method == "notifications/initialized" || is_notification {
        return StatusCode::ACCEPTED.into_response();
    }
    let token = bearer(&headers);
    let Some(token) = token else {
        return unauthorized(&server);
    };
    let app = server.app.clone();
    let token_for_check = token.clone();
    match tokio::task::spawn_blocking(move || app.require_access_token(&token_for_check)).await {
        Ok(Ok(())) => {}
        _ => return unauthorized(&server),
    };
    if method == "tools/list" {
        return Json(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": { "tools": tool_defs() }
        }))
        .into_response();
    }
    if method != "tools/call" {
        return Json(rpc_error(id, -32601, "method not found")).into_response();
    }
    let params = request.get("params").cloned().unwrap_or(json!({}));
    let name = params
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
    let app = server.app.clone();
    let backend = server.backend.clone();
    let result =
        tokio::task::spawn_blocking(move || call_tool(&app, &backend, &name, &arguments)).await;
    match result {
        Ok(Ok(payload)) => Json(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "content": [{ "type": "text", "text": payload.to_string() }],
                "isError": false
            }
        }))
        .into_response(),
        Ok(Err(message)) => Json(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "content": [{ "type": "text", "text": message }],
                "isError": true
            }
        }))
        .into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

fn call_tool(
    app: &App,
    backend: &Registry,
    name: &str,
    arguments: &Value,
) -> std::result::Result<Value, String> {
    let text = |key: &str| {
        arguments
            .get(key)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    match name {
        "whoami" => {
            let links = app.list_store_links().map_err(|err| err.to_string())?;
            Ok(json!({
                "linked_stores": links.iter().filter(|link| link.linked).map(|link| link.id.as_str()).collect::<Vec<_>>()
            }))
        }
        "list_stores" => {
            let links = app.list_store_links().map_err(|err| err.to_string())?;
            Ok(serde_json::to_value(links).unwrap_or(json!([])))
        }
        "link_store" => {
            let store =
                StoreId::parse(&text("store")).ok_or_else(|| "unknown store".to_string())?;
            let base = text("base_url");
            let secrets = StoreSecrets {
                username: text("username"),
                password: text("password"),
                base_url: if base.trim().is_empty() {
                    None
                } else {
                    Some(base)
                },
            };
            app.link_store(store, &secrets)
                .map_err(|err| err.to_string())?;
            Ok(json!({ "linked": store.as_str() }))
        }
        "unlink_store" => {
            let store =
                StoreId::parse(&text("store")).ok_or_else(|| "unknown store".to_string())?;
            app.unlink_store(store).map_err(|err| err.to_string())?;
            Ok(json!({ "unlinked": store.as_str() }))
        }
        "search" | "compare" => {
            let comparison = app
                .search(&text("query"), backend)
                .map_err(|err| err.to_string())?;
            Ok(serde_json::to_value(comparison).unwrap_or(json!({})))
        }
        "cart_list" => {
            let carts = app.carts().map_err(|err| err.to_string())?;
            Ok(serde_json::to_value(carts).unwrap_or(json!([])))
        }
        "cart_add" => {
            let store =
                StoreId::parse(&text("store")).ok_or_else(|| "unknown store".to_string())?;
            let qty = arguments.get("qty").and_then(|v| v.as_f64()).unwrap_or(1.0);
            let price = arguments.get("unit_price").and_then(|v| v.as_f64());
            let cart = app
                .cart_add(
                    store,
                    &text("product_id"),
                    &text("name"),
                    qty,
                    &text("unit"),
                    price,
                    &{
                        let currency = text("currency");
                        if currency.is_empty() {
                            "AED".into()
                        } else {
                            currency
                        }
                    },
                )
                .map_err(|err| err.to_string())?;
            Ok(serde_json::to_value(cart).unwrap_or(json!({})))
        }
        "cart_update" => {
            let store =
                StoreId::parse(&text("store")).ok_or_else(|| "unknown store".to_string())?;
            let qty = arguments.get("qty").and_then(|v| v.as_f64()).unwrap_or(1.0);
            let cart = app
                .cart_set_qty(store, &text("line_id"), qty)
                .map_err(|err| err.to_string())?;
            Ok(serde_json::to_value(cart).unwrap_or(json!({})))
        }
        "cart_remove" => {
            let store =
                StoreId::parse(&text("store")).ok_or_else(|| "unknown store".to_string())?;
            let cart = app
                .cart_remove(store, &text("line_id"))
                .map_err(|err| err.to_string())?;
            Ok(serde_json::to_value(cart).unwrap_or(json!({})))
        }
        "cart_clear" => {
            let store =
                StoreId::parse(&text("store")).ok_or_else(|| "unknown store".to_string())?;
            let cart = app.cart_clear(store).map_err(|err| err.to_string())?;
            Ok(serde_json::to_value(cart).unwrap_or(json!({})))
        }
        "checkout" => {
            let store =
                StoreId::parse(&text("store")).ok_or_else(|| "unknown store".to_string())?;
            let record = app
                .checkout(store, backend)
                .map_err(|err| err.to_string())?;
            Ok(serde_json::to_value(record).unwrap_or(json!({})))
        }
        "import_invoice" => {
            let document = if let Some(value) = arguments.get("document").and_then(|v| v.as_str()) {
                value.to_string()
            } else {
                arguments.to_string()
            };
            let saved = app
                .import_document(&document)
                .map_err(|err| err.to_string())?;
            Ok(serde_json::to_value(saved).unwrap_or(json!([])))
        }
        "list_invoices" => {
            let invoices = app.list_invoices().map_err(|err| err.to_string())?;
            Ok(serde_json::to_value(invoices).unwrap_or(json!([])))
        }
        "spend" => {
            let range = SpendRange::parse(&text("range")).unwrap_or(SpendRange::ThisMonth);
            let report = app
                .spend(range, Utc::now().date_naive())
                .map_err(|err| err.to_string())?;
            Ok(serde_json::to_value(report).unwrap_or(json!({})))
        }
        "forecast" => {
            let items = app
                .forecast(Utc::now().date_naive())
                .map_err(|err| err.to_string())?;
            Ok(serde_json::to_value(items).unwrap_or(json!([])))
        }
        _ => Err(format!("unknown tool {name}")),
    }
}

fn tool_defs() -> Value {
    let tool = |name: &str, description: &str, required: &[&str], props: Value| {
        json!({
            "name": name,
            "description": description,
            "inputSchema": {
                "type": "object",
                "properties": props,
                "required": required,
            }
        })
    };
    json!([
        tool("whoami", "Linked stores on this machine.", &[], json!({})),
        tool(
            "list_stores",
            "Store accounts linked on this machine.",
            &[],
            json!({})
        ),
        tool(
            "link_store",
            "Save a store sign-in. Credentials stay encrypted on this machine.",
            &["store", "username", "password"],
            json!({
                "store": {"type": "string"},
                "username": {"type": "string"},
                "password": {"type": "string"},
                "base_url": {"type": "string", "description": "Optional store contract address."}
            })
        ),
        tool(
            "unlink_store",
            "Remove a saved store sign-in.",
            &["store"],
            json!({"store": {"type": "string"}})
        ),
        tool(
            "search",
            "Search linked store contracts and receipt history.",
            &["query"],
            json!({"query": {"type": "string"}})
        ),
        tool(
            "compare",
            "Compare prices for one product across stores and receipt history.",
            &["query"],
            json!({"query": {"type": "string"}})
        ),
        tool(
            "cart_list",
            "List local carts for every store.",
            &[],
            json!({})
        ),
        tool(
            "cart_add",
            "Add a line to a local cart.",
            &["store", "name"],
            json!({
                "store": {"type": "string"},
                "product_id": {"type": "string"},
                "name": {"type": "string"},
                "qty": {"type": "number"},
                "unit": {"type": "string"},
                "unit_price": {"type": "number"},
                "currency": {"type": "string"}
            })
        ),
        tool(
            "cart_update",
            "Change a cart line quantity. Zero removes the line.",
            &["store", "line_id", "qty"],
            json!({
                "store": {"type": "string"},
                "line_id": {"type": "string"},
                "qty": {"type": "number"}
            })
        ),
        tool(
            "cart_remove",
            "Remove a cart line.",
            &["store", "line_id"],
            json!({
                "store": {"type": "string"},
                "line_id": {"type": "string"}
            })
        ),
        tool(
            "cart_clear",
            "Empty one store cart.",
            &["store"],
            json!({"store": {"type": "string"}})
        ),
        tool(
            "checkout",
            "Prepare checkout for a local cart. Payment stays with the store.",
            &["store"],
            json!({"store": {"type": "string"}})
        ),
        tool(
            "import_invoice",
            "Import a receipt as JSON or CSV.",
            &["document"],
            json!({"document": {"type": "string"}})
        ),
        tool("list_invoices", "List imported receipts.", &[], json!({})),
        tool(
            "spend",
            "Spending totals for a date window.",
            &[],
            json!({"range": {"type": "string", "description": "this_month, last_month, 7, 30, or 365"}})
        ),
        tool(
            "forecast",
            "What to buy again, and when, from receipt history.",
            &[],
            json!({})
        )
    ])
}

fn unauthorized(server: &Server) -> Response {
    let metadata = format!(
        "{}/.well-known/oauth-protected-resource",
        server.issuer.trim_end_matches('/')
    );
    let mut response = (
        StatusCode::UNAUTHORIZED,
        Json(json!({"error": "unauthorized"})),
    )
        .into_response();
    response.headers_mut().insert(
        header::WWW_AUTHENTICATE,
        format!("Bearer resource_metadata=\"{metadata}\"")
            .parse()
            .unwrap_or_else(|_| header::HeaderValue::from_static("Bearer")),
    );
    response
}

fn oauth_error(err: &Error) -> Response {
    let (error, description) = match err {
        Error::Oauth { error, description } => (*error, description.clone()),
        other => ("invalid_request", other.to_string()),
    };
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "error": error, "error_description": description })),
    )
        .into_response()
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message }
    })
}

fn bearer(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let token = value.strip_prefix("Bearer ")?.trim();
    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}

fn parse_form(bytes: &[u8]) -> std::collections::HashMap<String, String> {
    if let Ok(value) = serde_json::from_slice::<Value>(bytes) {
        if let Some(object) = value.as_object() {
            return object
                .iter()
                .filter_map(|(key, item)| item.as_str().map(|text| (key.clone(), text.to_string())))
                .collect();
        }
    }
    let text = String::from_utf8_lossy(bytes);
    let mut map = std::collections::HashMap::new();
    for pair in text.split('&') {
        if pair.is_empty() {
            continue;
        }
        let mut parts = pair.splitn(2, '=');
        let key = decode_form(parts.next().unwrap_or(""));
        let value = decode_form(parts.next().unwrap_or(""));
        map.insert(key, value);
    }
    map
}

fn decode_form(value: &str) -> String {
    let replaced = value.replace('+', " ");
    let bytes = replaced.as_bytes();
    let mut out = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(
                std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or(""),
                16,
            ) {
                out.push(byte);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn client_from_form(form: &std::collections::HashMap<String, String>) -> ClientAuth {
    ClientAuth {
        client_id: form.get("client_id").cloned().unwrap_or_default(),
        client_secret: form.get("client_secret").cloned(),
    }
}

fn basic_client(headers: &HeaderMap) -> Option<ClientAuth> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let encoded = value.strip_prefix("Basic ")?;
    let decoded = STANDARD.decode(encoded).ok()?;
    let text = String::from_utf8(decoded).ok()?;
    let (id, secret) = text.split_once(':')?;
    Some(ClientAuth {
        client_id: id.to_string(),
        client_secret: Some(secret.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use omabring_core::{pkce_s256, BackendError};
    use omabring_stores::{StoreRequest, StoreResponse, StoreTransport};
    use tower::ServiceExt;

    struct QuietTransport;

    impl StoreTransport for QuietTransport {
        fn exchange(
            &self,
            _request: &StoreRequest,
        ) -> std::result::Result<StoreResponse, BackendError> {
            Err(BackendError::unavailable(
                "no contract endpoint in this test",
            ))
        }
    }

    fn server() -> (tempfile::TempDir, Server) {
        let dir = tempfile::tempdir().unwrap();
        let app = Arc::new(App::open(dir.path()).unwrap());
        let server = Server {
            app,
            backend: Arc::new(Registry::with_transport(Arc::new(QuietTransport))),
            issuer: "http://127.0.0.1:8787".into(),
        };
        (dir, server)
    }

    async fn body_json(response: Response) -> Value {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn oauth_and_mcp_roundtrip() {
        let (_dir, server) = server();
        let app = router(server.clone());
        let health = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(health.status(), StatusCode::OK);

        let metadata = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/.well-known/oauth-authorization-server")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let metadata = body_json(metadata).await;
        assert_eq!(metadata["code_challenge_methods_supported"][0], "S256");
        assert!(metadata["registration_endpoint"]
            .as_str()
            .unwrap()
            .ends_with("/oauth/register"));

        let registered = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/oauth/register")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"client_name":"Grok","redirect_uris":["http://127.0.0.1:9/cb"],"token_endpoint_auth_method":"none"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(registered.status(), StatusCode::CREATED);
        let registered = body_json(registered).await;
        let client_id = registered["client_id"].as_str().unwrap().to_string();

        let denied = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mcp")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);

        let verifier = "abcdefghijklmnopqrstuvwxyz0123456789abcdefg";
        let challenge = pkce_s256(verifier);
        let pending = server
            .app
            .start_authorization(
                &client_id,
                "http://127.0.0.1:9/cb",
                &challenge,
                "S256",
                Some("st"),
                None,
            )
            .unwrap();
        server.app.decide_authorization(&pending, true).unwrap();
        let location = match server.app.poll_authorization(&pending).unwrap() {
            AuthzPoll::Ready { location } => location,
            other => panic!("unexpected {other:?}"),
        };
        let code = location
            .split("code=")
            .nth(1)
            .unwrap()
            .split('&')
            .next()
            .unwrap();
        let token_body = format!(
            "grant_type=authorization_code&code={code}&redirect_uri=http%3A%2F%2F127.0.0.1%3A9%2Fcb&code_verifier={verifier}&client_id={client_id}"
        );
        let tokens = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/oauth/token")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from(token_body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(tokens.status(), StatusCode::OK);
        let tokens = body_json(tokens).await;
        let access = tokens["access_token"].as_str().unwrap().to_string();

        let init = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mcp")
                    .header("content-type", "application/json")
                    .header("authorization", format!("Bearer {access}"))
                    .body(Body::from(
                        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"grok","version":"1"}}}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let init = body_json(init).await;
        assert_eq!(init["result"]["serverInfo"]["name"], "OmaBring");

        let document = r#"{"store":"carrefour","invoice_no":"Z","date":"2026-01-01","currency":"AED","items":[{"name":"Rice","qty":1,"unit_price":4}]}"#;
        let call = json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": { "name": "import_invoice", "arguments": { "document": document } }
        });
        let imported = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mcp")
                    .header("content-type", "application/json")
                    .header("authorization", format!("Bearer {access}"))
                    .body(Body::from(call.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let imported = body_json(imported).await;
        assert_eq!(imported["result"]["isError"], false);
        let text = imported["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("Rice"));

        let spend = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mcp")
                    .header("content-type", "application/json")
                    .header("authorization", format!("Bearer {access}"))
                    .body(Body::from(
                        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"spend","arguments":{"range":"365"}}}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let spend = body_json(spend).await;
        let text = spend["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("4"));
    }
}

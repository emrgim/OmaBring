mod pages;

use std::sync::Arc;

use axum::extract::{Form, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use omabring_core::{Db, User};
use omabring_oauth::{self, token_body};
use serde::Deserialize;
use serde_json::{json, Value};
use tower_http::compression::CompressionLayer;
use tower_http::cors::CorsLayer;

#[derive(Clone)]
pub struct App {
    pub db: Db,
    pub boot_id: String,
}

pub fn router(db: Db) -> Router {
    let app = Arc::new(App { db, boot_id: omabring_core::crypto::random_token(8) });
    Router::new()
        .route("/", get(home))
        .route("/login", post(login))
        .route("/logout", get(logout))
        .route("/purchases", get(purchases))
        .route("/stores", get(stores))
        .route("/stores/:id", post(save_store))
        .route("/stores/:id/clear", post(clear_store))
        .route("/settings", get(settings))
        .route("/settings/password", post(set_password))
        .route("/forgot", get(forgot_form).post(forgot_post))
        .route("/reset", get(reset_form).post(reset_post))
        .route("/oauth/authorize", get(authorize))
        .route("/oauth/token", post(token))
        .route("/oauth/register", post(register))
        .route("/.well-known/oauth-authorization-server", get(metadata))
        .route("/.well-known/oauth-protected-resource", get(resource_meta))
        .route("/mcp", get(mcp_get).post(mcp_post).delete(mcp_delete))
        .route("/health", get(health))
        .route("/backup", post(backup))
        .route("/manifest.webmanifest", get(manifest))
        .route("/sw.js", get(sw))
        .route("/offline.html", get(offline))
        .route("/static/icon.svg", get(icon))
        .layer(CompressionLayer::new())
        .layer(CorsLayer::permissive())
        .with_state(app)
}

pub fn serve() -> Result<(), Box<dyn std::error::Error>> {
    let host = std::env::var("OMABRING_HOST").unwrap_or_else(|_| "127.0.0.1".into());
    let port: u16 = std::env::var("OMABRING_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(8877);
    let db = Db::open(omabring_core::db::default_data_dir())?;
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async move {
        let listener = tokio::net::TcpListener::bind((host.as_str(), port)).await?;
        eprintln!("OmaBring listening on http://{host}:{port}");
        axum::serve(listener, router(db)).await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })
}

fn page(title: &str, body: &str, signed_in: bool) -> Response {
    Html(pages::shell(title, body, signed_in)).into_response()
}

fn cookie(headers: &HeaderMap, name: &str) -> String {
    headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|raw| {
            raw.split(';').find_map(|part| {
                let mut bits = part.trim().splitn(2, '=');
                let key = bits.next()?.trim();
                let val = bits.next()?.trim();
                if key == name { Some(val.to_string()) } else { None }
            })
        })
        .unwrap_or_default()
}

fn session_cookie(token: &str) -> String {
    format!("omabring_session={token}; HttpOnly; Path=/; SameSite=Lax; Max-Age=2592000")
}

fn clear_cookie() -> String {
    "omabring_session=; HttpOnly; Path=/; SameSite=Lax; Max-Age=0".into()
}

fn user_from(state: &App, headers: &HeaderMap) -> Option<(User, String)> {
    let token = cookie(headers, "omabring_session");
    state.db.session_user(&token).ok().flatten()
}

fn bearer(headers: &HeaderMap) -> String {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("")
        .trim()
        .to_string()
}

fn issuer(headers: &HeaderMap) -> String {
    if let Ok(url) = std::env::var("OMABRING_PUBLIC_URL") {
        if !url.trim().is_empty() {
            return url.trim().trim_end_matches('/').to_string();
        }
    }
    let proto = headers.get("x-forwarded-proto").and_then(|v| v.to_str().ok()).unwrap_or("http");
    let host = headers
        .get("x-forwarded-host")
        .or_else(|| headers.get(header::HOST))
        .and_then(|v| v.to_str().ok())
        .unwrap_or("127.0.0.1:8877");
    format!("{proto}://{host}")
}

fn unauthorized(headers: &HeaderMap) -> Response {
    let meta = format!("{}/.well-known/oauth-protected-resource", issuer(headers));
    (
        StatusCode::UNAUTHORIZED,
        [(header::WWW_AUTHENTICATE, format!("Bearer realm=\"OmaBring\", resource_metadata=\"{meta}\""))],
        "unauthorized",
    )
        .into_response()
}

async fn home(State(state): State<Arc<App>>, headers: HeaderMap) -> Response {
    let Some((user, _)) = user_from(&state, &headers) else {
        return page("Sign in", &pages::login_form("", "/"), false);
    };
    let today = chrono::Utc::now().date_naive();
    let spend = state.db.spend(user.id, "this_month", today, "").unwrap_or_else(|_| empty_spend());
    let list = state.db.shopping_list(user.id, today, 7, 8, "").unwrap_or_default();
    let stores = state.db.list_accounts(user.id).unwrap_or_default();
    page("Home", &pages::dashboard(&user.email, &spend, &list, &stores), true)
}

fn empty_spend() -> omabring_core::purchases::SpendReport {
    omabring_core::purchases::SpendReport {
        range: "this_month".into(),
        since: String::new(),
        until: String::new(),
        total: 0.0,
        average: 0.0,
        period_count: 0,
        grain: "weekly".into(),
        bars: Vec::new(),
        by_retailer: Vec::new(),
        by_category: Vec::new(),
    }
}

#[derive(Deserialize)]
struct LoginForm {
    email: String,
    password: String,
    #[serde(default)]
    next: String,
}

async fn login(State(state): State<Arc<App>>, Form(form): Form<LoginForm>) -> Response {
    let email = form.email.trim().to_ascii_lowercase();
    let user = match state.db.user_by_email(&email) {
        Ok(Some(user)) => {
            if !state.db.verify_user(&user, &form.password) {
                return page("Sign in", &pages::login_form("Wrong password", &form.next), false);
            }
            user
        }
        Ok(None) => match state.db.create_user(&email, &form.password) {
            Ok(user) => user,
            Err(err) => return page("Sign in", &pages::login_form(&err.to_string(), &form.next), false),
        },
        Err(err) => return page("Sign in", &pages::login_form(&err.to_string(), &form.next), false),
    };
    match state.db.create_session(user.id, 30) {
        Ok((token, _)) => {
            let next = if form.next.starts_with('/') { form.next } else { "/".into() };
            ([(header::SET_COOKIE, session_cookie(&token))], Redirect::to(&next)).into_response()
        }
        Err(err) => page("Sign in", &pages::login_form(&err.to_string(), &form.next), false),
    }
}

async fn logout() -> Response {
    (
        [(header::SET_COOKIE, clear_cookie()), (header::CONTENT_TYPE, "text/html; charset=utf-8".into())],
        Html(format!(
            "{}<script>if (navigator.serviceWorker && navigator.serviceWorker.controller) navigator.serviceWorker.controller.postMessage('clear'); location.replace('/');</script>",
            pages::shell("Signed out", "<h1>Signed out</h1>", false)
        )),
    )
        .into_response()
}

#[derive(Deserialize)]
struct Offset {
    #[serde(default)]
    offset: i64,
}

async fn purchases(State(state): State<Arc<App>>, headers: HeaderMap, Query(q): Query<Offset>) -> Response {
    let Some((user, _)) = user_from(&state, &headers) else {
        return Redirect::to("/").into_response();
    };
    let rows = state.db.list_invoices(user.id, 24, q.offset.max(0)).unwrap_or_default();
    page("Purchases", &pages::purchases(&rows, q.offset.max(0)), true)
}

async fn stores(State(state): State<Arc<App>>, headers: HeaderMap) -> Response {
    let Some((user, csrf)) = user_from(&state, &headers) else {
        return Redirect::to("/").into_response();
    };
    let accounts = state.db.list_accounts(user.id).unwrap_or_default();
    page("Stores", &pages::stores_page(&accounts, &csrf, ""), true)
}

#[derive(Deserialize)]
struct StoreForm {
    csrf: String,
    #[serde(default)]
    email: String,
    #[serde(default)]
    password: String,
    #[serde(default)]
    address: String,
}

async fn save_store(
    State(state): State<Arc<App>>,
    headers: HeaderMap,
    axum::extract::Path(id): axum::extract::Path<String>,
    Form(form): Form<StoreForm>,
) -> Response {
    let Some((user, csrf)) = user_from(&state, &headers) else {
        return Redirect::to("/").into_response();
    };
    if form.csrf != csrf {
        return page("Stores", "<h1>Session check failed</h1>", true);
    }
    let notice = match state.db.set_retailer_account(user.id, &id, &form.email, &form.password, &form.address) {
        Ok(()) => "Saved",
        Err(err) => return page("Stores", &format!("<h1>{}</h1>", pages::esc(&err.to_string())), true),
    };
    let accounts = state.db.list_accounts(user.id).unwrap_or_default();
    page("Stores", &pages::stores_page(&accounts, &csrf, notice), true)
}

async fn clear_store(
    State(state): State<Arc<App>>,
    headers: HeaderMap,
    axum::extract::Path(id): axum::extract::Path<String>,
    Form(form): Form<StoreForm>,
) -> Response {
    let Some((user, csrf)) = user_from(&state, &headers) else {
        return Redirect::to("/").into_response();
    };
    if form.csrf != csrf {
        return page("Stores", "<h1>Session check failed</h1>", true);
    }
    let _ = state.db.clear_retailer(user.id, &id);
    let accounts = state.db.list_accounts(user.id).unwrap_or_default();
    page("Stores", &pages::stores_page(&accounts, &csrf, "Unlinked"), true)
}

async fn settings(State(state): State<Arc<App>>, headers: HeaderMap) -> Response {
    let Some((user, csrf)) = user_from(&state, &headers) else {
        return Redirect::to("/").into_response();
    };
    let (path, status, _) = state.db.backup_settings().unwrap_or_default();
    page(
        "Settings",
        &pages::settings(&user.email, &user.mcp_token, &path, &status, &csrf, &issuer(&headers)),
        true,
    )
}

#[derive(Deserialize)]
struct PasswordForm {
    csrf: String,
    password: String,
}

async fn set_password(State(state): State<Arc<App>>, headers: HeaderMap, Form(form): Form<PasswordForm>) -> Response {
    let Some((user, csrf)) = user_from(&state, &headers) else {
        return Redirect::to("/").into_response();
    };
    if form.csrf != csrf {
        return page("Settings", "<h1>Session check failed</h1>", true);
    }
    match state.db.set_password(user.id, &form.password) {
        Ok(()) => Redirect::to("/settings").into_response(),
        Err(err) => page("Settings", &format!("<h1>{}</h1>", pages::esc(&err.to_string())), true),
    }
}

async fn forgot_form() -> Response {
    page("Reset", &pages::forgot(""), false)
}

#[derive(Deserialize)]
struct EmailForm {
    email: String,
}

async fn forgot_post(State(state): State<Arc<App>>, Form(form): Form<EmailForm>) -> Response {
    let notice = match state.db.create_reset(&form.email) {
        Ok(Some(token)) => format!(r#"Reset link: <a href="/reset?token={token}">/reset?token={token}</a>"#),
        _ => "If that email exists, a reset link is ready.".into(),
    };
    page("Reset", &pages::forgot(&notice), false)
}

#[derive(Deserialize)]
struct TokenQuery {
    #[serde(default)]
    token: String,
}

async fn reset_form(Query(q): Query<TokenQuery>) -> Response {
    page("Reset", &pages::reset(&q.token, ""), false)
}

#[derive(Deserialize)]
struct ResetForm {
    token: String,
    password: String,
}

async fn reset_post(State(state): State<Arc<App>>, Form(form): Form<ResetForm>) -> Response {
    let Some(user) = state.db.consume_reset(&form.token, 3600).ok().flatten() else {
        return page("Reset", &pages::reset("", "That reset link is no longer valid"), false);
    };
    if let Err(err) = state.db.set_password(user.id, &form.password) {
        return page("Reset", &pages::reset(&form.token, &err.to_string()), false);
    }
    Redirect::to("/").into_response()
}

#[derive(Deserialize)]
struct AuthorizeQuery {
    #[serde(default)]
    redirect_uri: String,
    #[serde(default)]
    client_id: String,
    #[serde(default)]
    code_challenge: String,
    #[serde(default)]
    code_challenge_method: String,
    #[serde(default)]
    state: String,
}

async fn authorize(State(state): State<Arc<App>>, headers: HeaderMap, Query(q): Query<AuthorizeQuery>) -> Response {
    let Some((user, _)) = user_from(&state, &headers) else {
        let next = format!(
            "/oauth/authorize?redirect_uri={}&client_id={}&code_challenge={}&code_challenge_method={}&state={}",
            urlencode(&q.redirect_uri),
            urlencode(&q.client_id),
            urlencode(&q.code_challenge),
            urlencode(&q.code_challenge_method),
            urlencode(&q.state)
        );
        return page("Authorize", &pages::login_form("", &next), false);
    };
    let client_id = if q.client_id.is_empty() { omabring_oauth::BUILTIN_CLIENT } else { q.client_id.as_str() };
    let Some(client) = state.db.oauth_client(client_id).ok().flatten() else {
        return page("Authorize", "<h1>Unknown client</h1>", true);
    };
    if !omabring_oauth::redirect_matches(&client, &q.redirect_uri) {
        return page("Authorize", "<h1>Redirect is not registered</h1>", true);
    }
    let method = if q.code_challenge_method.is_empty() { "S256" } else { q.code_challenge_method.as_str() };
    match state.db.save_oauth_code(user.id, &q.redirect_uri, client_id, &q.code_challenge, method) {
        Ok(code) => {
            let mut loc = format!("{}{}code={}", q.redirect_uri, if q.redirect_uri.contains('?') { '&' } else { '?' }, urlencode(&code));
            if !q.state.is_empty() {
                loc.push_str("&state=");
                loc.push_str(&urlencode(&q.state));
            }
            Redirect::to(&loc).into_response()
        }
        Err(err) => page("Authorize", &format!("<h1>{}</h1>", pages::esc(&err.to_string())), true),
    }
}

async fn token(State(state): State<Arc<App>>, body: String) -> Response {
    let parsed = parse_token_body(&body);
    match parsed.grant_type.as_str() {
        "refresh_token" => {
            let Some(user) = state.db.user_by_token(&parsed.refresh_token).ok().flatten() else {
                return (StatusCode::BAD_REQUEST, Json(json!({"error": "invalid_grant"}))).into_response();
            };
            Json(token_body(&user.mcp_token)).into_response()
        }
        "authorization_code" => {
            let Some((user, _client, challenge, method)) = state
                .db
                .consume_oauth_code(&parsed.code, Some(&parsed.redirect_uri))
                .ok()
                .flatten()
            else {
                return (StatusCode::BAD_REQUEST, Json(json!({"error": "invalid_grant"}))).into_response();
            };
            if !omabring_oauth::pkce_ok(&challenge, &method, &parsed.code_verifier) {
                return (StatusCode::BAD_REQUEST, Json(json!({"error": "invalid_grant"}))).into_response();
            }
            Json(token_body(&user.mcp_token)).into_response()
        }
        _ => (StatusCode::BAD_REQUEST, Json(json!({"error": "unsupported_grant_type"}))).into_response(),
    }
}

struct TokenBits {
    grant_type: String,
    code: String,
    redirect_uri: String,
    code_verifier: String,
    refresh_token: String,
}

fn parse_token_body(body: &str) -> TokenBits {
    if let Ok(v) = serde_json::from_str::<Value>(body) {
        return TokenBits {
            grant_type: v.get("grant_type").and_then(|x| x.as_str()).unwrap_or("").into(),
            code: v.get("code").and_then(|x| x.as_str()).unwrap_or("").into(),
            redirect_uri: v.get("redirect_uri").and_then(|x| x.as_str()).unwrap_or("").into(),
            code_verifier: v.get("code_verifier").and_then(|x| x.as_str()).unwrap_or("").into(),
            refresh_token: v.get("refresh_token").and_then(|x| x.as_str()).unwrap_or("").into(),
        };
    }
    let mut bits = TokenBits {
        grant_type: String::new(),
        code: String::new(),
        redirect_uri: String::new(),
        code_verifier: String::new(),
        refresh_token: String::new(),
    };
    for pair in body.split('&') {
        let mut kv = pair.splitn(2, '=');
        let key = urldecode(kv.next().unwrap_or(""));
        let val = urldecode(kv.next().unwrap_or(""));
        match key.as_str() {
            "grant_type" => bits.grant_type = val,
            "code" => bits.code = val,
            "redirect_uri" => bits.redirect_uri = val,
            "code_verifier" => bits.code_verifier = val,
            "refresh_token" => bits.refresh_token = val,
            _ => {}
        }
    }
    bits
}

async fn register(State(state): State<Arc<App>>, Json(body): Json<Value>) -> Response {
    let uris: Vec<String> = body
        .get("redirect_uris")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_default();
    if uris.iter().any(|u| !omabring_oauth::redirect_allowed(u)) {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "invalid_redirect_uri"}))).into_response();
    }
    let method = body.get("token_endpoint_auth_method").and_then(|v| v.as_str()).unwrap_or("none");
    match state.db.register_oauth_client(&uris, method) {
        Ok(doc) => (StatusCode::CREATED, Json(doc)).into_response(),
        Err(err) => (StatusCode::BAD_REQUEST, Json(json!({"error": err.to_string()}))).into_response(),
    }
}

async fn metadata(headers: HeaderMap) -> Json<Value> {
    Json(omabring_oauth::metadata(&issuer(&headers)))
}

async fn resource_meta(headers: HeaderMap) -> Json<Value> {
    Json(omabring_oauth::protected_resource(&issuer(&headers)))
}

async fn mcp_get(State(state): State<Arc<App>>, headers: HeaderMap) -> Response {
    if mcp_user(&state, &headers).is_none() {
        return unauthorized(&headers);
    }
    StatusCode::METHOD_NOT_ALLOWED.into_response()
}

async fn mcp_delete(State(state): State<Arc<App>>, headers: HeaderMap) -> Response {
    if mcp_user(&state, &headers).is_none() {
        return unauthorized(&headers);
    }
    StatusCode::NO_CONTENT.into_response()
}

async fn mcp_post(State(state): State<Arc<App>>, headers: HeaderMap, body: String) -> Response {
    let Some(user) = mcp_user(&state, &headers) else {
        return unauthorized(&headers);
    };
    let Ok(payload) = serde_json::from_str::<Value>(&body) else {
        return (StatusCode::BAD_REQUEST, Json(json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"parse error"}}))).into_response();
    };
    match omabring_mcp::handle(&state.db, &user, &payload) {
        Some(reply) => Json(reply).into_response(),
        None => StatusCode::ACCEPTED.into_response(),
    }
}

fn mcp_user(state: &App, headers: &HeaderMap) -> Option<User> {
    let token = bearer(headers);
    if !token.is_empty() {
        return state.db.user_by_token(&token).ok().flatten();
    }
    user_from(state, headers).map(|(user, _)| user)
}

async fn health(State(state): State<Arc<App>>, headers: HeaderMap) -> Json<Value> {
    let public = issuer(&headers);
    Json(json!({
        "ok": true,
        "server": "OmaBring",
        "version": omabring_mcp::SERVER_VERSION,
        "boot_id": state.boot_id,
        "public_url": public,
        "mcp_url": format!("{public}/mcp"),
    }))
}

#[derive(Deserialize)]
struct BackupForm {
    csrf: String,
    #[serde(default)]
    path: String,
    #[serde(default)]
    op: String,
}

async fn backup(State(state): State<Arc<App>>, headers: HeaderMap, Form(form): Form<BackupForm>) -> Response {
    let Some((user, csrf)) = user_from(&state, &headers) else {
        return Redirect::to("/").into_response();
    };
    if form.csrf != csrf {
        return page("Settings", "<h1>Session check failed</h1>", true);
    }
    if !form.path.trim().is_empty() {
        let _ = state.db.set_backup_path(form.path.trim());
    }
    if form.op == "run" {
        if let Err(err) = state.db.backup_configured() {
            let _ = state.db.set_backup_status("error", &err.to_string());
        }
    }
    let _ = user;
    Redirect::to("/settings").into_response()
}

async fn manifest() -> Response {
    ([(header::CONTENT_TYPE, "application/manifest+json")], pages::MANIFEST).into_response()
}

async fn sw() -> Response {
    ([(header::CONTENT_TYPE, "text/javascript")], pages::SW).into_response()
}

async fn offline() -> Html<&'static str> {
    Html(pages::OFFLINE)
}

async fn icon() -> Response {
    ([(header::CONTENT_TYPE, "image/svg+xml")], pages::ICON).into_response()
}

fn urlencode(raw: &str) -> String {
    let mut out = String::new();
    for b in raw.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn urldecode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""), 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        if bytes[i] == b'+' {
            out.push(b' ');
        } else {
            out.push(bytes[i]);
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

use base64::Engine;
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha1::Sha1;

type HmacSha1 = Hmac<Sha1>;

pub struct XCreds {
    pub api_key: String,
    pub api_secret: String,
    pub access_token: String,
    pub access_secret: String,
    pub bearer: String,
}

pub fn creds_from_env() -> XCreds {
    XCreds {
        api_key: std::env::var("X_API_KEY").unwrap_or_default(),
        api_secret: std::env::var("X_API_SECRET").unwrap_or_default(),
        access_token: std::env::var("X_ACCESS_TOKEN").unwrap_or_default(),
        access_secret: std::env::var("X_ACCESS_TOKEN_SECRET").unwrap_or_default(),
        bearer: std::env::var("X_BEARER_TOKEN").unwrap_or_default(),
    }
}

/// RFC 3986 percent-encoding. Unreserved characters include `~`.
pub fn percent_encode(raw: &str) -> String {
    let mut out = String::new();
    for b in raw.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub fn signature_base(method: &str, url: &str, params: &[(String, String)]) -> String {
    let mut pairs = params.to_vec();
    pairs.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    let encoded = pairs
        .into_iter()
        .map(|(k, v)| format!("{}={}", percent_encode(&k), percent_encode(&v)))
        .collect::<Vec<_>>()
        .join("&");
    format!(
        "{}&{}&{}",
        method.to_ascii_uppercase(),
        percent_encode(url),
        percent_encode(&encoded)
    )
}

pub fn sign(method: &str, url: &str, params: &[(String, String)], creds: &XCreds) -> String {
    let base = signature_base(method, url, params);
    let key = format!("{}&{}", percent_encode(&creds.api_secret), percent_encode(&creds.access_secret));
    let mut mac = HmacSha1::new_from_slice(key.as_bytes()).expect("hmac key");
    mac.update(base.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes())
}

pub fn call(tool: &str, args: &Value) -> Value {
    let creds = creds_from_env();
    if creds.api_key.is_empty() || creds.api_secret.is_empty() {
        return json!({"success": false, "error_code": "x_credentials_missing", "error": "X credentials are not configured"});
    }
    let needs_user = matches!(tool, "x_me" | "x_user_posts" | "x_mentions" | "x_post");
    if needs_user && creds.access_token.is_empty() {
        return json!({"success": false, "error_code": "x_user_context_required", "error": "X user context is not configured"});
    }
    let username = args.get("username").and_then(|v| v.as_str()).unwrap_or("ilTrumpista");
    let username = if username == "me" { "me" } else { username };
    let url = match tool {
        "x_me" => "https://api.x.com/2/users/me".to_string(),
        "x_user_by_username" => format!("https://api.x.com/2/users/by/username/{username}"),
        "x_user_posts" => format!("https://api.x.com/2/users/{username}/tweets"),
        "x_mentions" => format!("https://api.x.com/2/users/{username}/mentions"),
        "x_search" => "https://api.x.com/2/tweets/search/recent".to_string(),
        "x_post" => "https://api.x.com/2/tweets".to_string(),
        _ => return json!({"success": false, "error": "unknown x tool"}),
    };
    let method = if tool == "x_post" { "POST" } else { "GET" };
    let mut params = vec![
        ("oauth_consumer_key".into(), creds.api_key.clone()),
        ("oauth_token".into(), creds.access_token.clone()),
        ("oauth_signature_method".into(), "HMAC-SHA1".into()),
        ("oauth_version".into(), "1.0".into()),
    ];
    if tool == "x_search" {
        if let Some(q) = args.get("query").and_then(|v| v.as_str()) {
            params.push(("query".into(), q.into()));
        }
    }
    let signature = if creds.access_secret.is_empty() {
        String::new()
    } else {
        sign(method, url.split('?').next().unwrap_or(&url), &params, &creds)
    };
    json!({
        "success": false,
        "error_code": "x_outbound_disabled",
        "error": "X requests are signed locally and are not sent",
        "request": {"method": method, "url": url, "signed": !signature.is_empty(), "bearer_configured": !creds.bearer.is_empty()},
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tilde_stays_unencoded() {
        assert_eq!(percent_encode("a b~"), "a%20b~");
    }

    #[test]
    fn signature_base_sorts_parameters() {
        let base = signature_base(
            "post",
            "https://api.x.com/2/tweets",
            &[("b".into(), "2".into()), ("a".into(), "1".into())],
        );
        assert!(base.starts_with("POST&"));
        assert!(base.contains(&percent_encode("a=1&b=2")));
    }
}

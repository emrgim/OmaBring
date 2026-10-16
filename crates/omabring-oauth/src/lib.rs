use base64::Engine;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub const ACCESS_TTL_SECS: u64 = 30 * 24 * 60 * 60;
pub const BUILTIN_CLIENT: &str = "omabring";

pub fn metadata(issuer: &str) -> Value {
    let issuer = issuer.trim_end_matches('/');
    json!({
        "issuer": issuer,
        "authorization_endpoint": format!("{issuer}/oauth/authorize"),
        "token_endpoint": format!("{issuer}/oauth/token"),
        "registration_endpoint": format!("{issuer}/oauth/register"),
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "code_challenge_methods_supported": ["S256", "plain"],
        "token_endpoint_auth_methods_supported": ["none", "client_secret_post"],
        "scopes_supported": ["mcp"],
    })
}

pub fn protected_resource(issuer: &str) -> Value {
    let issuer = issuer.trim_end_matches('/');
    json!({
        "resource": format!("{issuer}/mcp"),
        "authorization_servers": [issuer],
        "scopes_supported": ["mcp"],
        "bearer_methods_supported": ["header"],
    })
}

pub fn redirect_allowed(uri: &str) -> bool {
    let uri = uri.trim();
    if uri.starts_with("https://") {
        return true;
    }
    uri.starts_with("http://127.0.0.1") || uri.starts_with("http://localhost")
}

pub fn redirect_matches(client: &Value, uri: &str) -> bool {
    if !redirect_allowed(uri) {
        return false;
    }
    match client.get("redirect_uris") {
        None | Some(Value::Null) => true,
        Some(Value::Array(uris)) if uris.is_empty() => true,
        Some(Value::Array(uris)) => uris.iter().any(|u| u.as_str() == Some(uri)),
        Some(Value::String(raw)) => raw == uri,
        _ => false,
    }
}

pub fn pkce_s256(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

pub fn pkce_ok(challenge: &str, method: &str, verifier: &str) -> bool {
    if challenge.is_empty() {
        return true;
    }
    if verifier.is_empty() {
        return false;
    }
    match method {
        "plain" => challenge == verifier,
        _ => pkce_s256(verifier) == challenge,
    }
}

pub fn token_body(access_token: &str) -> Value {
    json!({
        "access_token": access_token,
        "token_type": "Bearer",
        "expires_in": ACCESS_TTL_SECS,
        "refresh_token": access_token,
        "scope": "mcp",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn localhost_and_https_redirects_are_accepted() {
        assert!(redirect_allowed("https://connector.example/cb"));
        assert!(redirect_allowed("http://127.0.0.1:9/cb"));
        assert!(redirect_allowed("http://localhost:8877/cb"));
        assert!(!redirect_allowed("http://evil.example/cb"));
    }

    #[test]
    fn builtin_client_has_no_redirect_restriction() {
        let client = json!({"client_id": "omabring", "redirect_uris": null});
        assert!(redirect_matches(&client, "https://x.example/cb"));
        let registered = json!({"redirect_uris": ["https://ok.example/cb"]});
        assert!(!redirect_matches(&registered, "https://other.example/cb"));
        assert!(redirect_matches(&registered, "https://ok.example/cb"));
    }

    #[test]
    fn s256_challenge_matches_the_verifier() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = pkce_s256(verifier);
        assert!(pkce_ok(&challenge, "S256", verifier));
        assert!(!pkce_ok(&challenge, "S256", "nope"));
    }
}

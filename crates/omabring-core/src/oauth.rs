use rusqlite::{params, OptionalExtension};

use crate::crypto::{pkce_s256, random_token, sha256_hex, urlencode};
use crate::error::{Error, Result};
use crate::model::{AuthzPoll, ClientAuth, PendingAuthz, RegisteredClient, TokenSet};
use crate::{now_rfc3339, parse_time, plus_seconds, App};

const PENDING_SECONDS: i64 = 180;
const CODE_SECONDS: i64 = 300;
const ACCESS_SECONDS: i64 = 3600;
const REFRESH_SECONDS: i64 = 60 * 60 * 24 * 30;
const SCOPE: &str = "omabring";

impl App {
    pub fn register_client(
        &self,
        client_name: &str,
        redirect_uris: &[String],
        token_endpoint_auth_method: &str,
    ) -> Result<RegisteredClient> {
        let name = client_name.trim();
        if name.is_empty() || name.chars().count() > 120 {
            return Err(Error::oauth(
                "invalid_client_metadata",
                "client_name is required",
            ));
        }
        if redirect_uris.is_empty() {
            return Err(Error::oauth(
                "invalid_client_metadata",
                "redirect_uris is required",
            ));
        }
        for uri in redirect_uris {
            if !redirect_allowed(uri) {
                return Err(Error::oauth(
                    "invalid_redirect_uri",
                    "redirect_uri must be https, an omabring address, or loopback http",
                ));
            }
        }
        let method = match token_endpoint_auth_method {
            "" | "none" => "none",
            "client_secret_post" | "client_secret_basic" => token_endpoint_auth_method,
            _ => {
                return Err(Error::oauth(
                    "invalid_client_metadata",
                    "unsupported token endpoint auth method",
                ))
            }
        };
        let client_id = random_token(18);
        let client_secret = if method == "none" {
            None
        } else {
            Some(random_token(32))
        };
        let secret_hash = client_secret.as_deref().map(sha256_hex);
        let uris =
            serde_json::to_string(redirect_uris).map_err(|err| Error::Message(err.to_string()))?;
        let created = now_rfc3339();
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO oauth_clients (client_id, client_name, secret_hash, redirect_uris, token_endpoint_auth_method, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![client_id, name, secret_hash, uris, method, created],
            )?;
            Ok(RegisteredClient {
                client_id,
                client_secret,
                client_name: name.to_string(),
                redirect_uris: redirect_uris.to_vec(),
                token_endpoint_auth_method: method.to_string(),
            })
        })
    }

    pub fn start_authorization(
        &self,
        client_id: &str,
        redirect_uri: &str,
        code_challenge: &str,
        code_challenge_method: &str,
        state: Option<&str>,
        scope: Option<&str>,
    ) -> Result<String> {
        if code_challenge_method != "S256" {
            return Err(Error::oauth(
                "invalid_request",
                "code_challenge_method must be S256",
            ));
        }
        if !valid_challenge(code_challenge) {
            return Err(Error::oauth(
                "invalid_request",
                "code_challenge is not valid",
            ));
        }
        let scope = scope.unwrap_or(SCOPE);
        if scope.split_whitespace().any(|part| part != SCOPE) {
            return Err(Error::oauth("invalid_scope", "scope must be omabring"));
        }
        self.with_conn(|conn| {
            let uris = client_redirects(conn, client_id)?;
            if !uris.iter().any(|uri| uri == redirect_uri) {
                return Err(Error::oauth(
                    "invalid_request",
                    "redirect_uri is not registered",
                ));
            }
            let id = random_token(16);
            let created = now_rfc3339();
            let expires = plus_seconds(PENDING_SECONDS);
            conn.execute(
                "INSERT INTO oauth_pending (id, client_id, redirect_uri, code_challenge, state, scope, status, auth_code, created_at, expires_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', NULL, ?7, ?8)",
                params![id, client_id, redirect_uri, code_challenge, state, scope, created, expires],
            )?;
            Ok(id)
        })
    }

    pub fn list_pending(&self) -> Result<Vec<PendingAuthz>> {
        let now = now_rfc3339();
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT p.id, c.client_name, p.scope, p.redirect_uri, p.created_at FROM oauth_pending p JOIN oauth_clients c ON c.client_id = p.client_id WHERE p.status = 'pending' AND p.expires_at > ?1 ORDER BY p.created_at",
            )?;
            let rows = stmt.query_map(params![now], |row| {
                Ok(PendingAuthz {
                    id: row.get(0)?,
                    client_name: row.get(1)?,
                    scope: row.get(2)?,
                    redirect_uri: row.get(3)?,
                    created_at: row.get(4)?,
                })
            })?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(Error::from)
        })
    }

    pub fn decide_authorization(&self, pending_id: &str, allow: bool) -> Result<()> {
        self.with_conn(|conn| {
            let row = conn
                .query_row(
                    "SELECT client_id, redirect_uri, code_challenge, scope, expires_at, status FROM oauth_pending WHERE id = ?1",
                    params![pending_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, String>(5)?,
                        ))
                    },
                )
                .optional()?;
            let Some((client_id, redirect_uri, challenge, scope, expires, status)) = row else {
                return Err(Error::oauth("invalid_request", "unknown authorization request"));
            };
            if status != "pending" || parse_time(&expires)? < chrono::Utc::now() {
                return Err(Error::oauth("invalid_request", "authorization request expired"));
            }
            if !allow {
                conn.execute(
                    "UPDATE oauth_pending SET status = 'denied' WHERE id = ?1",
                    params![pending_id],
                )?;
                return Ok(());
            }
            let code = random_token(32);
            let code_hash = sha256_hex(&code);
            let code_expires = plus_seconds(CODE_SECONDS);
            conn.execute(
                "INSERT INTO oauth_codes (code_hash, client_id, redirect_uri, code_challenge, scope, expires_at, used) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0)",
                params![code_hash, client_id, redirect_uri, challenge, scope, code_expires],
            )?;
            conn.execute(
                "UPDATE oauth_pending SET status = 'approved', auth_code = ?1, expires_at = ?2 WHERE id = ?3",
                params![code, code_expires, pending_id],
            )?;
            Ok(())
        })
    }

    pub fn poll_authorization(&self, pending_id: &str) -> Result<AuthzPoll> {
        self.with_conn(|conn| {
            let row = conn
                .query_row(
                    "SELECT status, redirect_uri, state, auth_code, expires_at FROM oauth_pending WHERE id = ?1",
                    params![pending_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, Option<String>>(2)?,
                            row.get::<_, Option<String>>(3)?,
                            row.get::<_, String>(4)?,
                        ))
                    },
                )
                .optional()?;
            let Some((status, redirect_uri, state, auth_code, expires)) = row else {
                return Ok(AuthzPoll::Expired);
            };
            if parse_time(&expires)? < chrono::Utc::now() && status == "pending" {
                conn.execute(
                    "UPDATE oauth_pending SET status = 'expired' WHERE id = ?1",
                    params![pending_id],
                )?;
                return Ok(AuthzPoll::Expired);
            }
            match status.as_str() {
                "pending" => Ok(AuthzPoll::Pending),
                "denied" => {
                    conn.execute(
                        "UPDATE oauth_pending SET status = 'consumed' WHERE id = ?1 AND status = 'denied'",
                        params![pending_id],
                    )?;
                    Ok(AuthzPoll::Denied {
                        location: redirect_with(
                            &redirect_uri,
                            &[("error", "access_denied"), ("state", state.as_deref().unwrap_or(""))],
                        ),
                    })
                }
                "approved" => {
                    let Some(code) = auth_code else {
                        return Ok(AuthzPoll::Expired);
                    };
                    let changed = conn.execute(
                        "UPDATE oauth_pending SET status = 'consumed', auth_code = NULL WHERE id = ?1 AND status = 'approved'",
                        params![pending_id],
                    )?;
                    if changed == 0 {
                        return Ok(AuthzPoll::Expired);
                    }
                    let mut pairs = vec![("code", code.as_str())];
                    if let Some(state) = state.as_deref() {
                        if !state.is_empty() {
                            pairs.push(("state", state));
                        }
                    }
                    Ok(AuthzPoll::Ready {
                        location: redirect_with(&redirect_uri, &pairs),
                    })
                }
                _ => Ok(AuthzPoll::Expired),
            }
        })
    }

    pub fn exchange_code(
        &self,
        code: &str,
        redirect_uri: &str,
        code_verifier: &str,
        client: &ClientAuth,
    ) -> Result<TokenSet> {
        if code_verifier.len() < 43 || code_verifier.len() > 128 {
            return Err(Error::oauth(
                "invalid_grant",
                "code_verifier length is invalid",
            ));
        }
        let code_hash = sha256_hex(code);
        self.with_conn(|conn| {
            authenticate_client(conn, client)?;
            let row = conn
                .query_row(
                    "SELECT client_id, redirect_uri, code_challenge, scope, expires_at, used FROM oauth_codes WHERE code_hash = ?1",
                    params![code_hash],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, i64>(5)?,
                        ))
                    },
                )
                .optional()?;
            let Some((client_id, saved_redirect, challenge, scope, expires, used)) = row else {
                return Err(Error::oauth("invalid_grant", "authorization code is unknown"));
            };
            if used != 0 || parse_time(&expires)? < chrono::Utc::now() {
                return Err(Error::oauth("invalid_grant", "authorization code is no longer valid"));
            }
            if client_id != client.client_id || saved_redirect != redirect_uri {
                return Err(Error::oauth("invalid_grant", "authorization code does not match this client"));
            }
            if pkce_s256(code_verifier) != challenge {
                return Err(Error::oauth("invalid_grant", "code_verifier does not match"));
            }
            conn.execute(
                "UPDATE oauth_codes SET used = 1 WHERE code_hash = ?1 AND used = 0",
                params![code_hash],
            )?;
            issue_token(conn, &client_id, &scope)
        })
    }

    pub fn refresh_token(&self, refresh_token: &str, client: &ClientAuth) -> Result<TokenSet> {
        let refresh_hash = sha256_hex(refresh_token);
        self.with_conn(|conn| {
            authenticate_client(conn, client)?;
            let row = conn
                .query_row(
                    "SELECT id, client_id, scope, refresh_expires, revoked FROM oauth_tokens WHERE refresh_hash = ?1",
                    params![refresh_hash],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, i64>(4)?,
                        ))
                    },
                )
                .optional()?;
            let Some((id, client_id, scope, expires, revoked)) = row else {
                return Err(Error::oauth("invalid_grant", "refresh token is unknown"));
            };
            if revoked != 0 || client_id != client.client_id || parse_time(&expires)? < chrono::Utc::now() {
                return Err(Error::oauth("invalid_grant", "refresh token is no longer valid"));
            }
            conn.execute("UPDATE oauth_tokens SET revoked = 1 WHERE id = ?1", params![id])?;
            issue_token(conn, &client_id, &scope)
        })
    }

    pub fn revoke_token(&self, token: &str, client: &ClientAuth) -> Result<()> {
        let hash = sha256_hex(token);
        self.with_conn(|conn| {
            authenticate_client(conn, client)?;
            conn.execute(
                "UPDATE oauth_tokens SET revoked = 1 WHERE client_id = ?1 AND (access_hash = ?2 OR refresh_hash = ?2)",
                params![client.client_id, hash],
            )?;
            Ok(())
        })
    }

    pub fn require_access_token(&self, access_token: &str) -> Result<()> {
        let hash = sha256_hex(access_token);
        self.with_conn(|conn| {
            let row = conn
                .query_row(
                    "SELECT access_expires, revoked FROM oauth_tokens WHERE access_hash = ?1",
                    params![hash],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
                )
                .optional()?;
            let Some((expires, revoked)) = row else {
                return Err(Error::Unauthorized);
            };
            if revoked != 0 || parse_time(&expires)? < chrono::Utc::now() {
                return Err(Error::Unauthorized);
            }
            Ok(())
        })
    }
}

fn issue_token(conn: &rusqlite::Connection, client_id: &str, scope: &str) -> Result<TokenSet> {
    let access = random_token(32);
    let refresh = random_token(32);
    conn.execute(
        "INSERT INTO oauth_tokens (client_id, access_hash, refresh_hash, scope, access_expires, refresh_expires, revoked) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0)",
        params![
            client_id,
            sha256_hex(&access),
            sha256_hex(&refresh),
            scope,
            plus_seconds(ACCESS_SECONDS),
            plus_seconds(REFRESH_SECONDS),
        ],
    )?;
    Ok(TokenSet {
        access_token: access,
        refresh_token: refresh,
        token_type: "Bearer".into(),
        expires_in: ACCESS_SECONDS,
        scope: scope.to_string(),
    })
}

fn client_redirects(conn: &rusqlite::Connection, client_id: &str) -> Result<Vec<String>> {
    let raw = conn
        .query_row(
            "SELECT redirect_uris FROM oauth_clients WHERE client_id = ?1",
            params![client_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    let Some(raw) = raw else {
        return Err(Error::oauth("invalid_client", "client is not registered"));
    };
    serde_json::from_str(&raw).map_err(|err| Error::Message(err.to_string()))
}

fn authenticate_client(conn: &rusqlite::Connection, client: &ClientAuth) -> Result<()> {
    let row = conn
        .query_row(
            "SELECT secret_hash, token_endpoint_auth_method FROM oauth_clients WHERE client_id = ?1",
            params![client.client_id],
            |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    let Some((secret_hash, method)) = row else {
        return Err(Error::oauth("invalid_client", "client is not registered"));
    };
    if method == "none" {
        return Ok(());
    }
    let Some(secret) = client.client_secret.as_deref() else {
        return Err(Error::oauth("invalid_client", "client secret is required"));
    };
    if secret_hash.as_deref() != Some(sha256_hex(secret).as_str()) {
        return Err(Error::oauth("invalid_client", "client secret is not valid"));
    }
    Ok(())
}

fn redirect_allowed(uri: &str) -> bool {
    if uri.starts_with("https://") || uri.starts_with("omabring://") {
        return uri.len() > 10;
    }
    let Some(rest) = uri.strip_prefix("http://") else {
        return false;
    };
    let host = rest.split(['/', ':', '?', '#']).next().unwrap_or("");
    matches!(host, "localhost" | "127.0.0.1" | "[::1]")
}

fn valid_challenge(value: &str) -> bool {
    let len = value.len();
    (43..=128).contains(&len)
        && value
            .bytes()
            .all(|b| matches!(b, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_'))
}

fn redirect_with(base: &str, pairs: &[(&str, &str)]) -> String {
    let mut out = base.to_string();
    let joiner = if base.contains('?') { '&' } else { '?' };
    out.push(joiner);
    for (index, (key, value)) in pairs.iter().enumerate() {
        if index > 0 {
            out.push('&');
        }
        out.push_str(&urlencode(key));
        out.push('=');
        out.push_str(&urlencode(value));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::App;

    fn setup() -> (tempfile::TempDir, App, RegisteredClient) {
        let dir = tempfile::tempdir().unwrap();
        let app = App::open(dir.path()).unwrap();
        let client = app
            .register_client("Grok", &["http://127.0.0.1:9/callback".into()], "none")
            .unwrap();
        (dir, app, client)
    }

    #[test]
    fn authorization_code_with_pkce_and_refresh_rotation() {
        let (_dir, app, client) = setup();
        let verifier = "abcdefghijklmnopqrstuvwxyz0123456789abcdefg";
        let challenge = pkce_s256(verifier);
        let pending = app
            .start_authorization(
                &client.client_id,
                "http://127.0.0.1:9/callback",
                &challenge,
                "S256",
                Some("xyz"),
                Some("omabring"),
            )
            .unwrap();
        assert_eq!(app.list_pending().unwrap().len(), 1);
        assert!(matches!(
            app.poll_authorization(&pending).unwrap(),
            AuthzPoll::Pending
        ));
        app.decide_authorization(&pending, true).unwrap();
        let AuthzPoll::Ready { location } = app.poll_authorization(&pending).unwrap() else {
            panic!("expected redirect");
        };
        assert!(location.contains("state=xyz"));
        let code = location
            .split("code=")
            .nth(1)
            .unwrap()
            .split('&')
            .next()
            .unwrap();
        let auth = ClientAuth {
            client_id: client.client_id.clone(),
            client_secret: None,
        };
        let tokens = app
            .exchange_code(code, "http://127.0.0.1:9/callback", verifier, &auth)
            .unwrap();
        assert_eq!(tokens.token_type, "Bearer");
        assert!(app.require_access_token(&tokens.access_token).is_ok());
        assert!(app
            .exchange_code(code, "http://127.0.0.1:9/callback", verifier, &auth)
            .is_err());
        let rotated = app.refresh_token(&tokens.refresh_token, &auth).unwrap();
        assert!(app.require_access_token(&rotated.access_token).is_ok());
        assert!(app.refresh_token(&tokens.refresh_token, &auth).is_err());
        app.revoke_token(&rotated.access_token, &auth).unwrap();
        assert!(app.require_access_token(&rotated.access_token).is_err());
    }

    #[test]
    fn rejects_missing_pkce_and_bad_redirect() {
        let (_dir, app, client) = setup();
        let err = app
            .start_authorization(
                &client.client_id,
                "http://127.0.0.1:9/callback",
                "abc",
                "plain",
                None,
                None,
            )
            .unwrap_err();
        assert!(err.to_string().contains("S256"));
        let err = app
            .register_client("Other", &["http://evil.example/cb".into()], "none")
            .unwrap_err();
        assert!(err.to_string().contains("redirect"));
        let verifier = "abcdefghijklmnopqrstuvwxyz0123456789abcdefg";
        let pending = app
            .start_authorization(
                &client.client_id,
                "http://127.0.0.1:9/callback",
                &pkce_s256(verifier),
                "S256",
                None,
                None,
            )
            .unwrap();
        app.decide_authorization(&pending, false).unwrap();
        let AuthzPoll::Denied { location } = app.poll_authorization(&pending).unwrap() else {
            panic!("expected denial");
        };
        assert!(location.contains("access_denied"));
    }

    #[test]
    fn confidential_client_requires_secret() {
        let dir = tempfile::tempdir().unwrap();
        let app = App::open(dir.path()).unwrap();
        let client = app
            .register_client(
                "Desk",
                &["https://client.example/cb".into()],
                "client_secret_post",
            )
            .unwrap();
        assert!(client.client_secret.is_some());
        let verifier = "abcdefghijklmnopqrstuvwxyz0123456789abcdefg";
        let pending = app
            .start_authorization(
                &client.client_id,
                "https://client.example/cb",
                &pkce_s256(verifier),
                "S256",
                None,
                None,
            )
            .unwrap();
        app.decide_authorization(&pending, true).unwrap();
        let AuthzPoll::Ready { location } = app.poll_authorization(&pending).unwrap() else {
            panic!("ready");
        };
        let code = location.split("code=").nth(1).unwrap();
        let missing = ClientAuth {
            client_id: client.client_id.clone(),
            client_secret: None,
        };
        assert!(app
            .exchange_code(code, "https://client.example/cb", verifier, &missing)
            .is_err());
        let ok = ClientAuth {
            client_id: client.client_id,
            client_secret: client.client_secret,
        };
        assert!(app
            .exchange_code(code, "https://client.example/cb", verifier, &ok)
            .is_ok());
    }
}

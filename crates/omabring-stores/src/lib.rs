use std::sync::Arc;
use std::time::Duration;

use omabring_core::{
    BackendError, Cart, Offer, OfferSource, RemoteCheckout, StoreBackend, StoreId, StoreSecrets,
};

/// One linked shop. Live calls use the OmaBring store contract when the
/// account has a contract address. Without one, the adapter stays quiet and
/// the app keeps using local carts and receipt history.
pub struct Registry {
    adapters: Vec<Adapter>,
}

struct Adapter {
    store: StoreId,
    transport: Arc<dyn StoreTransport>,
}

pub trait StoreTransport: Send + Sync {
    fn exchange(&self, request: &StoreRequest) -> std::result::Result<StoreResponse, BackendError>;
}

#[derive(Debug, Clone)]
pub struct StoreRequest {
    pub method: &'static str,
    pub url: String,
    pub body: Option<String>,
}

#[derive(Debug, Clone)]
pub struct StoreResponse {
    pub status: u16,
    pub body: String,
}

impl Registry {
    pub fn with_transport(transport: Arc<dyn StoreTransport>) -> Self {
        let adapters = StoreId::all()
            .into_iter()
            .map(|store| Adapter {
                store,
                transport: Arc::clone(&transport),
            })
            .collect();
        Self { adapters }
    }

    pub fn live() -> Self {
        Self::with_transport(Arc::new(HttpTransport::default()))
    }

    fn adapter(&self, store: StoreId) -> Option<&Adapter> {
        self.adapters.iter().find(|adapter| adapter.store == store)
    }
}

impl StoreBackend for Registry {
    fn search_store(
        &self,
        store: StoreId,
        secrets: &StoreSecrets,
        query: &str,
    ) -> std::result::Result<Vec<Offer>, BackendError> {
        let adapter = self
            .adapter(store)
            .ok_or_else(|| BackendError::unsupported("unknown store"))?;
        let base = contract_base(secrets)?;
        let url = format!("{base}/omabring/search?q={}", urlencode(query));
        let response = adapter.transport.exchange(&StoreRequest {
            method: "GET",
            url,
            body: None,
        })?;
        ensure_ok(&response)?;
        parse_offers(store, &response.body)
    }

    fn checkout_store(
        &self,
        store: StoreId,
        secrets: &StoreSecrets,
        cart: &Cart,
    ) -> std::result::Result<RemoteCheckout, BackendError> {
        if !store.remote_checkout() {
            return Err(BackendError::unsupported(format!(
                "{} keeps the cart locally",
                store.name()
            )));
        }
        let adapter = self
            .adapter(store)
            .ok_or_else(|| BackendError::unsupported("unknown store"))?;
        let base = contract_base(secrets)?;
        let body = serde_json::json!({
            "store": store.as_str(),
            "lines": cart.lines,
        });
        let response = adapter.transport.exchange(&StoreRequest {
            method: "POST",
            url: format!("{base}/omabring/checkout"),
            body: Some(body.to_string()),
        })?;
        ensure_ok(&response)?;
        parse_checkout(&response.body)
    }
}

fn contract_base(secrets: &StoreSecrets) -> std::result::Result<String, BackendError> {
    let Some(base) = secrets
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return Err(BackendError::not_configured());
    };
    if !(base.starts_with("https://")
        || base.starts_with("http://127.0.0.1")
        || base.starts_with("http://localhost"))
    {
        return Err(BackendError::bad_response(
            "contract address must be https, or loopback http",
        ));
    }
    Ok(base.trim_end_matches('/').to_string())
}

fn ensure_ok(response: &StoreResponse) -> std::result::Result<(), BackendError> {
    if (200..300).contains(&response.status) {
        Ok(())
    } else if response.status == 404 {
        Err(BackendError::unavailable(
            "store contract endpoint was not found",
        ))
    } else {
        Err(BackendError::unavailable(format!(
            "store contract returned HTTP {}",
            response.status
        )))
    }
}

pub fn parse_offers(store: StoreId, body: &str) -> std::result::Result<Vec<Offer>, BackendError> {
    let value: serde_json::Value = serde_json::from_str(body)
        .map_err(|_| BackendError::bad_response("search response was not JSON"))?;
    let products = value
        .get("products")
        .and_then(|v| v.as_array())
        .ok_or_else(|| BackendError::bad_response("search response has no products"))?;
    let mut offers = Vec::new();
    for product in products {
        let name = product
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let price = product.get("price").and_then(|v| v.as_f64());
        if name.is_empty() || price.is_none() {
            continue;
        }
        let product_id = product
            .get("id")
            .and_then(|v| match v {
                serde_json::Value::String(text) => Some(text.clone()),
                serde_json::Value::Number(number) => Some(number.to_string()),
                _ => None,
            })
            .unwrap_or_else(|| name.clone());
        offers.push(Offer {
            store,
            product_id,
            name,
            price: price.unwrap_or(0.0),
            currency: product
                .get("currency")
                .and_then(|v| v.as_str())
                .unwrap_or("AED")
                .to_uppercase(),
            unit: product
                .get("unit")
                .and_then(|v| v.as_str())
                .unwrap_or("piece")
                .to_string(),
            source: OfferSource::Live,
        });
    }
    Ok(offers)
}

pub fn parse_checkout(body: &str) -> std::result::Result<RemoteCheckout, BackendError> {
    let value: serde_json::Value = serde_json::from_str(body)
        .map_err(|_| BackendError::bad_response("checkout response was not JSON"))?;
    let status = value.get("status").and_then(|v| v.as_str()).unwrap_or("");
    if status != "ready" && status != "prepared" && status != "accepted" {
        return Err(BackendError::bad_response(
            "checkout response did not accept the cart",
        ));
    }
    Ok(RemoteCheckout {
        reference: value
            .get("reference")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        url: value
            .get("url")
            .and_then(|v| v.as_str())
            .map(str::to_string),
    })
}

fn urlencode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Blocking store client. A client is built for each call so it is not dropped
/// from the background service runtime.
#[derive(Clone, Default)]
pub struct HttpTransport;

impl StoreTransport for HttpTransport {
    fn exchange(&self, request: &StoreRequest) -> std::result::Result<StoreResponse, BackendError> {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(8))
            .build()
            .map_err(|err| BackendError::unavailable(err.to_string()))?;
        let builder = match request.method {
            "POST" => client.post(&request.url),
            "GET" => client.get(&request.url),
            _ => {
                return Err(BackendError::unsupported(
                    "method is not part of the store contract",
                ))
            }
        };
        let builder = if let Some(body) = &request.body {
            builder
                .header("content-type", "application/json")
                .body(body.clone())
        } else {
            builder
        };
        let response = builder
            .send()
            .map_err(|err| BackendError::unavailable(err.to_string()))?;
        let status = response.status().as_u16();
        let body = response
            .text()
            .map_err(|err| BackendError::unavailable(err.to_string()))?;
        Ok(StoreResponse { status, body })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use omabring_core::CartLine;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    struct MapTransport {
        status: u16,
        body: String,
    }

    impl StoreTransport for MapTransport {
        fn exchange(
            &self,
            _request: &StoreRequest,
        ) -> std::result::Result<StoreResponse, BackendError> {
            Ok(StoreResponse {
                status: self.status,
                body: self.body.clone(),
            })
        }
    }

    #[test]
    fn parses_contract_search_and_checkout() {
        let offers = parse_offers(
            StoreId::Carrefour,
            r#"{"products":[{"id":"9","name":"Labneh","price":12.5,"currency":"AED","unit":"tub"}]}"#,
        )
        .unwrap();
        assert_eq!(offers[0].name, "Labneh");
        assert_eq!(offers[0].source, OfferSource::Live);
        let checkout = parse_checkout(
            r#"{"status":"ready","url":"https://shop.example/pay","reference":"abc"}"#,
        )
        .unwrap();
        assert_eq!(checkout.reference.as_deref(), Some("abc"));
        assert!(parse_checkout(r#"{"status":"no"}"#).is_err());
    }

    #[test]
    fn registry_uses_contract_and_skips_local_only_stores() {
        let registry = Registry::with_transport(Arc::new(MapTransport {
            status: 200,
            body: r#"{"products":[{"id":"1","name":"Rice","price":9}]}"#.into(),
        }));
        let secrets = StoreSecrets {
            username: "u".into(),
            password: "p".into(),
            base_url: Some("https://contracts.example".into()),
        };
        let offers = registry
            .search_store(StoreId::UnionCoop, &secrets, "rice")
            .unwrap();
        assert_eq!(offers[0].store, StoreId::UnionCoop);
        let missing = StoreSecrets {
            username: "u".into(),
            password: "p".into(),
            base_url: None,
        };
        assert_eq!(
            registry
                .search_store(StoreId::Waitrose, &missing, "rice")
                .unwrap_err()
                .kind,
            omabring_core::BackendErrorKind::NotConfigured
        );
        let cart = Cart {
            store: StoreId::Mmi,
            lines: vec![CartLine {
                line_id: "1".into(),
                product_id: "1".into(),
                name: "Rice".into(),
                qty: 1.0,
                unit: "kg".into(),
                unit_price: Some(9.0),
                currency: "AED".into(),
            }],
        };
        assert_eq!(
            registry
                .checkout_store(StoreId::Mmi, &secrets, &cart)
                .unwrap_err()
                .kind,
            omabring_core::BackendErrorKind::Unsupported
        );
    }

    #[test]
    fn http_transport_reads_a_contract_response() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf);
            let body = r#"{"products":[{"id":"1","name":"Dates","price":18,"currency":"AED"}]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
        });
        let transport = HttpTransport::default();
        let response = transport
            .exchange(&StoreRequest {
                method: "GET",
                url: format!("http://{address}/omabring/search?q=dates"),
                body: None,
            })
            .unwrap();
        assert_eq!(response.status, 200);
        let offers = parse_offers(StoreId::Grandiose, &response.body).unwrap();
        assert_eq!(offers[0].name, "Dates");
    }
}

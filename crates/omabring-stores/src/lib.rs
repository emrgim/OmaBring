mod catalog;

use std::sync::Arc;
use std::time::Duration;

use omabring_core::{BackendError, Cart, RemoteCheckout, StoreBackend, StoreId, StoreSecrets};

/// Live UAE storefronts. Search hits each shop's own catalog. A saved sign-in
/// is only required when pushing a cart to Grandiose or Union Coop.
pub struct Registry {
    adapters: Vec<Adapter>,
}

struct Adapter {
    store: StoreId,
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
    pub fn with_transport(_transport: Arc<dyn StoreTransport>) -> Self {
        let adapters = StoreId::all()
            .into_iter()
            .map(|store| Adapter { store })
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

impl Registry {
    pub fn sign_in(
        &self,
        store: StoreId,
        username: &str,
        password: &str,
    ) -> std::result::Result<StoreSecrets, BackendError> {
        let _ = self;
        catalog::sign_in(store, username, password)
    }
}

impl StoreBackend for Registry {
    fn search_store(
        &self,
        store: StoreId,
        _secrets: &StoreSecrets,
        query: &str,
    ) -> std::result::Result<Vec<omabring_core::Offer>, BackendError> {
        if self.adapter(store).is_none() {
            return Err(BackendError::unsupported("unknown store"));
        }
        catalog::search(store, query)
    }

    fn checkout_store(
        &self,
        store: StoreId,
        secrets: &StoreSecrets,
        cart: &Cart,
    ) -> std::result::Result<RemoteCheckout, BackendError> {
        if self.adapter(store).is_none() {
            return Err(BackendError::unsupported("unknown store"));
        }
        catalog::push_cart(store, secrets, cart)
    }
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
            _ => return Err(BackendError::unsupported("method is not supported")),
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

    #[test]
    fn mmi_keeps_its_cart_on_the_store_website() {
        let registry = Registry::live();
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
                .checkout_store(StoreId::Mmi, &StoreSecrets::blank(), &cart)
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
                url: format!("http://{address}/dates"),
                body: None,
            })
            .unwrap();
        assert_eq!(response.status, 200);
        assert!(response.body.contains("Dates"));
    }
}

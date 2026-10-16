use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub struct Retailer {
    pub id: &'static str,
    pub name: &'static str,
    pub url: &'static str,
    pub color: &'static str,
    pub cart_url: &'static str,
    pub checkout_url: &'static str,
    pub domain: Option<&'static str>,
    pub enabled_default: bool,
    pub search: bool,
    pub receipts: bool,
    pub login: bool,
    pub shop: bool,
    pub checkout: bool,
}

pub const RETAILERS: &[Retailer] = &[
    Retailer {
        id: "grandiose",
        name: "Grandiose",
        url: "https://www.grandiose.ae/",
        color: "#c9a227",
        cart_url: "https://www.grandiose.ae/checkout/cart/",
        checkout_url: "https://www.grandiose.ae/checkout/",
        domain: Some("grandiose.ae"),
        enabled_default: true,
        search: true,
        receipts: true,
        login: true,
        shop: true,
        checkout: true,
    },
    Retailer {
        id: "unioncoop",
        name: "Union Coop",
        url: "https://www.unioncoop.ae/",
        color: "#0b7a3e",
        cart_url: "https://www.unioncoop.ae/checkout/cart/",
        checkout_url: "https://www.unioncoop.ae/checkout/",
        domain: Some("unioncoop.ae"),
        enabled_default: false,
        search: true,
        receipts: false,
        login: true,
        shop: true,
        checkout: true,
    },
    Retailer {
        id: "carrefour",
        name: "Carrefour UAE",
        url: "https://www.carrefouruae.com/mafuae/en",
        color: "#004e9f",
        cart_url: "https://www.carrefouruae.com/mafuae/en",
        checkout_url: "https://www.carrefouruae.com/mafuae/en/cart",
        domain: Some("carrefouruae.com"),
        enabled_default: true,
        search: true,
        receipts: true,
        login: true,
        shop: true,
        checkout: false,
    },
    Retailer {
        id: "waitrose",
        name: "Waitrose UAE",
        url: "https://www.waitrose.ae/en/",
        color: "#007a33",
        cart_url: "https://www.waitrose.ae/en/",
        checkout_url: "https://www.waitrose.ae/en/checkout/",
        domain: Some("waitrose.ae"),
        enabled_default: false,
        search: true,
        receipts: false,
        login: true,
        shop: false,
        checkout: false,
    },
    Retailer {
        id: "spinneys",
        name: "Spinneys UAE",
        url: "https://www.spinneys.com/en-ae/",
        color: "#8b1e3f",
        cart_url: "https://www.spinneys.com/en-ae/",
        checkout_url: "https://www.spinneys.com/en-ae/checkout/",
        domain: Some("spinneys.com"),
        enabled_default: false,
        search: true,
        receipts: false,
        login: true,
        shop: false,
        checkout: false,
    },
    Retailer {
        id: "mmi",
        name: "MMI",
        url: "https://www.mmihomedelivery.ae/",
        color: "#1a1a1a",
        cart_url: "https://www.mmihomedelivery.ae/",
        checkout_url: "https://www.mmihomedelivery.ae/",
        domain: Some("mmihomedelivery.ae"),
        enabled_default: false,
        search: true,
        receipts: true,
        login: true,
        shop: false,
        checkout: false,
    },
    Retailer {
        id: "africaneastern",
        name: "African + Eastern",
        url: "https://www.africaneasternonline.com/",
        color: "#b11226",
        cart_url: "https://www.africaneasternonline.com/",
        checkout_url: "https://www.africaneasternonline.com/",
        domain: Some("africaneasternonline.com"),
        enabled_default: false,
        search: true,
        receipts: true,
        login: true,
        shop: false,
        checkout: false,
    },
    Retailer {
        id: "careem",
        name: "Careem",
        url: "https://www.careem.com/",
        color: "#3bb54a",
        cart_url: "https://www.careem.com/",
        checkout_url: "https://www.careem.com/",
        domain: Some("careem.com"),
        enabled_default: false,
        search: false,
        receipts: true,
        login: false,
        shop: false,
        checkout: false,
    },
    Retailer {
        id: "mcdonalds",
        name: "McDonald's",
        url: "https://www.mcdonalds.com/ae/en-ae.html",
        color: "#da291c",
        cart_url: "https://www.mcdonalds.com/ae/en-ae.html",
        checkout_url: "https://www.mcdonalds.com/ae/en-ae.html",
        domain: Some("mcdonalds.com"),
        enabled_default: false,
        search: false,
        receipts: true,
        login: false,
        shop: false,
        checkout: false,
    },
    Retailer {
        id: "amazon_it",
        name: "Amazon.it",
        url: "https://www.amazon.it/",
        color: "#ff9900",
        cart_url: "https://www.amazon.it/",
        checkout_url: "https://www.amazon.it/",
        domain: Some("amazon.it"),
        enabled_default: false,
        search: false,
        receipts: false,
        login: false,
        shop: false,
        checkout: false,
    },
    Retailer {
        id: "amazon_ae",
        name: "Amazon.ae",
        url: "https://www.amazon.ae/",
        color: "#ff9900",
        cart_url: "https://www.amazon.ae/",
        checkout_url: "https://www.amazon.ae/",
        domain: Some("amazon.ae"),
        enabled_default: false,
        search: false,
        receipts: false,
        login: false,
        shop: false,
        checkout: false,
    },
];

pub const CAPABILITIES: &[(&str, &str)] = &[
    ("search", "Search"),
    ("compare", "Compare"),
    ("cart", "Cart"),
    ("checkout", "Checkout"),
    ("receipts", "Receipts"),
    ("login", "Login"),
];

/// Cart is filled only after a successful list or add. Compare follows search.
const LIVE_PILLS: &[&str] = &["cart"];

#[derive(Clone, Debug, Serialize)]
pub struct Capability {
    pub key: String,
    pub label: String,
    pub on: bool,
    pub declared: bool,
    pub hint: String,
}

#[derive(Clone, Debug)]
pub struct Probe {
    pub ok: bool,
    pub at: i64,
    pub error_code: Option<String>,
}

pub fn retailer(id: &str) -> Option<&'static Retailer> {
    RETAILERS.iter().find(|r| r.id == id)
}

pub fn known(id: &str) -> bool {
    retailer(id).is_some()
}

pub fn store_can_search(id: &str) -> bool {
    retailer(id).map(|r| r.search).unwrap_or(false)
}

pub fn store_can_shop(id: &str) -> bool {
    retailer(id).map(|r| r.shop).unwrap_or(false)
}

pub fn store_can_checkout(id: &str) -> bool {
    retailer(id).map(|r| r.checkout).unwrap_or(false)
}

pub fn searchable() -> impl Iterator<Item = &'static Retailer> {
    RETAILERS.iter().filter(|r| r.search)
}

pub fn retailer_for_domain(domain: &str) -> Option<&'static str> {
    let d = domain.trim().trim_start_matches('@').to_ascii_lowercase();
    RETAILERS.iter().find(|r| r.domain == Some(d.as_str())).map(|r| r.id)
}

pub fn capabilities(id: &str, probes: &[(String, String, Probe)]) -> Vec<Capability> {
    let Some(r) = retailer(id) else {
        return Vec::new();
    };
    let declared = |key: &str| match key {
        "search" | "compare" => r.search,
        "cart" => r.shop,
        "checkout" => r.checkout,
        "receipts" => r.receipts,
        "login" => r.login,
        _ => false,
    };
    CAPABILITIES
        .iter()
        .map(|(key, label)| {
            let wired = declared(key);
            let probe = probes.iter().find(|(rid, cap, _)| rid == id && cap == key);
            let on = if LIVE_PILLS.contains(key) {
                wired && probe.map(|(_, _, p)| p.ok).unwrap_or(false)
            } else {
                wired
            };
            let mut shown = (*label).to_string();
            if wired && !on {
                shown = format!("{label} · declared");
            }
            let hint = if let Some((_, _, p)) = probe {
                if p.error_code.is_some() && !on {
                    p.error_code.clone().unwrap_or_default()
                } else if wired && !on {
                    "declared, not proven".into()
                } else {
                    String::new()
                }
            } else if wired && !on {
                "declared, not proven".into()
            } else {
                String::new()
            };
            Capability {
                key: (*key).into(),
                label: shown,
                on,
                declared: wired,
                hint,
            }
        })
        .collect()
}

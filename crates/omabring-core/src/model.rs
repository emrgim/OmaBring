use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StoreId {
    Carrefour,
    UnionCoop,
    Grandiose,
    Spinneys,
    Waitrose,
    Mmi,
    AfricanEastern,
    /// Food delivery. Emailed invoices land here; there is no catalog.
    Careem,
    /// Counter food. Emailed order receipts land here; there is no catalog.
    Mcdonalds,
    /// E-commerce. Amazon.it order mail lands here; there is no catalog.
    AmazonIt,
    /// E-commerce. Amazon.ae order mail lands here; there is no catalog.
    AmazonAe,
}

impl StoreId {
    pub fn all() -> [StoreId; 11] {
        [
            Self::Grandiose,
            Self::UnionCoop,
            Self::Carrefour,
            Self::Waitrose,
            Self::Spinneys,
            Self::Mmi,
            Self::AfricanEastern,
            Self::Careem,
            Self::Mcdonalds,
            Self::AmazonIt,
            Self::AmazonAe,
        ]
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Carrefour => "carrefour",
            Self::UnionCoop => "union_coop",
            Self::Grandiose => "grandiose",
            Self::Spinneys => "spinneys",
            Self::Waitrose => "waitrose",
            Self::Mmi => "mmi",
            Self::AfricanEastern => "african_eastern",
            Self::Careem => "careem",
            Self::Mcdonalds => "mcdonalds",
            Self::AmazonIt => "amazon_it",
            Self::AmazonAe => "amazon_ae",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Carrefour => "Carrefour",
            Self::UnionCoop => "Union Coop",
            Self::Grandiose => "Grandiose",
            Self::Spinneys => "Spinneys",
            Self::Waitrose => "Waitrose",
            Self::Mmi => "MMI",
            Self::AfricanEastern => "African & Eastern",
            Self::Careem => "Careem",
            Self::Mcdonalds => "McDonald's",
            Self::AmazonIt => "Amazon.it",
            Self::AmazonAe => "Amazon.ae",
        }
    }

    pub fn homepage(self) -> &'static str {
        match self {
            Self::Carrefour => "https://www.carrefouruae.com/mafuae/en",
            Self::UnionCoop => "https://www.unioncoop.ae/",
            Self::Grandiose => "https://www.grandiose.ae/",
            Self::Spinneys => "https://www.spinneys.com/en-ae/",
            Self::Waitrose => "https://www.waitrose.ae/en/",
            Self::Mmi => "https://www.mmihomedelivery.ae/",
            Self::AfricanEastern => "https://www.africaneasternonline.com/",
            Self::Careem => "https://www.careem.com/",
            Self::Mcdonalds => "https://www.mcdonalds.com/ae/en-ae.html",
            Self::AmazonIt => "https://www.amazon.it/",
            Self::AmazonAe => "https://www.amazon.ae/",
        }
    }

    /// A supermarket catalog can be searched. Careem, McDonald's, and Amazon
    /// only keep emailed receipts.
    pub fn search(self) -> bool {
        !matches!(
            self,
            Self::Careem | Self::Mcdonalds | Self::AmazonIt | Self::AmazonAe
        )
    }

    /// Emailed or downloaded invoices belong to this store.
    pub fn receipts(self) -> bool {
        !matches!(self, Self::UnionCoop | Self::Waitrose | Self::Spinneys)
    }

    /// The store has its own sign-in. Receipt-only stores do not.
    pub fn login(self) -> bool {
        self.search()
    }

    pub fn mail_domain(self) -> Option<&'static str> {
        match self {
            Self::AmazonIt => Some("amazon.it"),
            Self::AmazonAe => Some("amazon.ae"),
            _ => None,
        }
    }

    /// Grandiose and Union Coop can receive a signed-in cart. Carrefour
    /// checkout names the store cart page. The others keep a local cart.
    /// OmaBring does not open an address.
    pub fn remote_checkout(self) -> bool {
        matches!(self, Self::Carrefour | Self::UnionCoop | Self::Grandiose)
    }

    pub fn parse(raw: &str) -> Option<Self> {
        let key: String = raw
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .flat_map(|c| c.to_lowercase())
            .collect();
        match key.as_str() {
            "carrefour" => Some(Self::Carrefour),
            "unioncoop" => Some(Self::UnionCoop),
            "grandiose" => Some(Self::Grandiose),
            "spinneys" => Some(Self::Spinneys),
            "waitrose" => Some(Self::Waitrose),
            "mmi" => Some(Self::Mmi),
            "africaneastern" | "africanandeastern" => Some(Self::AfricanEastern),
            "careem" => Some(Self::Careem),
            "mcdonalds" | "mcdonald" => Some(Self::Mcdonalds),
            "amazonit" | "amazonitaly" => Some(Self::AmazonIt),
            "amazonae" | "amazonuae" => Some(Self::AmazonAe),
            _ => None,
        }
    }
}

impl std::fmt::Display for StoreId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StoreSecrets {
    pub username: String,
    pub password: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Session token from the store's own sign-in. Catalog search does not need it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
}

impl StoreSecrets {
    pub fn blank() -> Self {
        Self {
            username: String::new(),
            password: String::new(),
            base_url: None,
            token: None,
            user_id: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoreLink {
    pub id: StoreId,
    pub name: &'static str,
    pub homepage: &'static str,
    pub linked: bool,
    pub username: Option<String>,
    pub base_url: Option<String>,
    pub remote_checkout: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OfferSource {
    Live,
    History,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Offer {
    pub store: StoreId,
    pub product_id: String,
    pub name: String,
    pub price: f64,
    pub currency: String,
    pub unit: String,
    pub source: OfferSource,
    /// Catalog picture for this offer. Empty when the store search had none.
    #[serde(default)]
    pub image_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceGroup {
    pub name: String,
    pub currency: String,
    pub cheapest_store: StoreId,
    pub cheapest_price: f64,
    pub offers: Vec<Offer>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveState {
    Off,
    NotConfigured,
    Ok,
    Unsupported,
    Unavailable,
    BadResponse,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoreNote {
    pub store: StoreId,
    pub linked: bool,
    pub live: LiveState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Comparison {
    pub query: String,
    pub offers: Vec<Offer>,
    pub groups: Vec<PriceGroup>,
    pub notes: Vec<StoreNote>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CartLine {
    pub line_id: String,
    pub product_id: String,
    pub name: String,
    pub qty: f64,
    pub unit: String,
    pub unit_price: Option<f64>,
    pub currency: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cart {
    pub store: StoreId,
    pub lines: Vec<CartLine>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckoutRecord {
    pub id: String,
    pub store: StoreId,
    pub status: String,
    pub url: Option<String>,
    pub note: String,
    pub created_at: String,
    pub lines: Vec<CartLine>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteCheckout {
    pub reference: Option<String>,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BackendErrorKind {
    NotConfigured,
    Unsupported,
    Unavailable,
    BadResponse,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendError {
    pub kind: BackendErrorKind,
    pub detail: String,
}

impl BackendError {
    pub fn not_configured() -> Self {
        Self {
            kind: BackendErrorKind::NotConfigured,
            detail: "this store is not signed in".into(),
        }
    }

    pub fn unsupported(detail: impl Into<String>) -> Self {
        Self {
            kind: BackendErrorKind::Unsupported,
            detail: detail.into(),
        }
    }

    pub fn unavailable(detail: impl Into<String>) -> Self {
        Self {
            kind: BackendErrorKind::Unavailable,
            detail: detail.into(),
        }
    }

    pub fn bad_response(detail: impl Into<String>) -> Self {
        Self {
            kind: BackendErrorKind::BadResponse,
            detail: detail.into(),
        }
    }
}

pub trait StoreBackend: Send + Sync {
    fn search_store(
        &self,
        store: StoreId,
        secrets: &StoreSecrets,
        query: &str,
    ) -> std::result::Result<Vec<Offer>, BackendError>;

    fn checkout_store(
        &self,
        store: StoreId,
        secrets: &StoreSecrets,
        cart: &Cart,
    ) -> std::result::Result<RemoteCheckout, BackendError>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvoiceLine {
    pub name: String,
    pub qty: f64,
    pub unit: String,
    pub unit_price: f64,
    pub line_total: f64,
    #[serde(default)]
    pub product_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvoiceSummary {
    pub id: i64,
    pub store: StoreId,
    pub invoice_no: String,
    pub purchased_on: String,
    pub currency: String,
    pub total: f64,
    pub lines: Vec<InvoiceLine>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpendRange {
    ThisMonth,
    LastMonth,
    LastDays(i64),
}

impl SpendRange {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            "this_month" | "this-month" | "month" => Some(Self::ThisMonth),
            "last_month" | "last-month" => Some(Self::LastMonth),
            "7" | "1w" | "week" => Some(Self::LastDays(7)),
            "30" | "1m" => Some(Self::LastDays(30)),
            "365" | "1y" | "year" => Some(Self::LastDays(365)),
            other => other
                .parse::<i64>()
                .ok()
                .filter(|n| *n > 0)
                .map(Self::LastDays),
        }
    }

    pub fn label(self) -> String {
        match self {
            Self::ThisMonth => "this month".into(),
            Self::LastMonth => "last month".into(),
            Self::LastDays(7) => "last 7 days".into(),
            Self::LastDays(30) => "last 30 days".into(),
            Self::LastDays(365) => "last 365 days".into(),
            Self::LastDays(n) => format!("last {n} days"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoneyTotal {
    pub currency: String,
    pub amount: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupSpend {
    pub key: String,
    pub currency: String,
    pub amount: f64,
    pub qty: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpendReport {
    pub range_label: String,
    pub start: String,
    pub end: String,
    pub invoice_count: u32,
    pub totals: Vec<MoneyTotal>,
    pub by_store: Vec<GroupSpend>,
    pub by_product: Vec<GroupSpend>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForecastStatus {
    Overdue,
    DueToday,
    DueSoon,
    Upcoming,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForecastItem {
    pub name: String,
    pub store: StoreId,
    pub currency: String,
    pub purchases: u32,
    pub median_interval_days: f64,
    pub last_purchase: String,
    pub next_purchase: String,
    pub days_until: i64,
    pub status: ForecastStatus,
    pub likely: u8,
    pub typical_unit_price: f64,
    pub unit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisteredClient {
    pub client_id: String,
    pub client_secret: Option<String>,
    pub client_name: String,
    pub redirect_uris: Vec<String>,
    pub token_endpoint_auth_method: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingAuthz {
    pub id: String,
    pub client_name: String,
    pub scope: String,
    pub redirect_uri: String,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub enum AuthzPoll {
    Pending,
    Ready { location: String },
    Denied { location: String },
    Expired,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenSet {
    pub access_token: String,
    pub refresh_token: String,
    pub token_type: String,
    pub expires_in: i64,
    pub scope: String,
}

#[derive(Debug, Clone)]
pub struct ClientAuth {
    pub client_id: String,
    pub client_secret: Option<String>,
}

pub fn normalize_name(name: &str) -> String {
    name.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// A receipt line that is only the store's own name.
/// A size upcharge such as "Large (13")" is a real line. So is "Carrefour Chicken".
pub fn junk_product_name(name: &str) -> bool {
    let folded = fold_product_name(name);
    if folded.is_empty() {
        return true;
    }
    const STORES: &[&str] = &[
        "grandiose",
        "carrefour",
        "union coop",
        "unioncoop",
        "waitrose",
        "spinneys",
        "mmi",
        "african eastern",
        "african and eastern",
        "careem",
        "mcdonalds",
        "mcdonald s",
        "mc donalds",
        "amazon",
        "amazon it",
        "amazon ae",
    ];
    if STORES.contains(&folded.as_str()) {
        return true;
    }
    // Letters only, so "GRANDIOSE!!!" and a hidden character inside the word
    // still match. "Grandiose Chicken" does not.
    const STORE_LETTERS: &[&str] = &[
        "grandiose",
        "carrefour",
        "unioncoop",
        "waitrose",
        "spinneys",
        "mmi",
        "africaneastern",
        "africanandeastern",
        "careem",
        "mcdonalds",
        "amazon",
        "amazonit",
        "amazonae",
    ];
    let letters: String = name
        .chars()
        .filter(|c| c.is_ascii_alphabetic())
        .flat_map(|c| c.to_lowercase())
        .collect();
    if STORE_LETTERS.contains(&letters.as_str()) {
        return true;
    }
    // "Grandiose Supermarket LLC" is the store banner. "Grandiose Chicken" stays.
    store_banner(&folded)
}

/// A Careem pizza size such as `Large (13")`. It is a real charge, not a product name.
pub fn is_size_upcharge(name: &str) -> bool {
    let folded = fold_product_name(name);
    let tokens: Vec<&str> = folded
        .split_whitespace()
        .filter(|token| !token.is_empty())
        .collect();
    if tokens.is_empty() {
        return false;
    }
    let mut adjective = false;
    for token in tokens {
        if matches!(
            token,
            "small" | "medium" | "large" | "xl" | "xxl" | "xs" | "regular" | "personal" | "family"
        ) {
            adjective = true;
            continue;
        }
        if token.chars().all(|c| c.is_ascii_digit())
            || matches!(
                token,
                "inch"
                    | "inches"
                    | "in"
                    | "cm"
                    | "mm"
                    | "ml"
                    | "cl"
                    | "l"
                    | "ltr"
                    | "litre"
                    | "liter"
                    | "g"
                    | "kg"
                    | "oz"
                    | "pcs"
                    | "pc"
                    | "pack"
                    | "size"
                    | "pizza"
            )
        {
            continue;
        }
        return false;
    }
    adjective
}

fn store_banner(folded: &str) -> bool {
    const FILLER: &[&str] = &[
        "supermarket",
        "market",
        "store",
        "llc",
        "ltd",
        "limited",
        "sole",
        "proprietorship",
        "uae",
        "dubai",
        "inc",
    ];
    let kept: Vec<&str> = folded
        .split_whitespace()
        .filter(|token| !FILLER.contains(token))
        .collect();
    let [token] = kept.as_slice() else {
        return false;
    };
    let letters: String = token.chars().filter(|c| c.is_ascii_alphabetic()).collect();
    matches!(
        letters.as_str(),
        "grandiose"
            | "carrefour"
            | "unioncoop"
            | "waitrose"
            | "spinneys"
            | "mmi"
            | "africaneastern"
            | "africanandeastern"
            | "careem"
            | "mcdonalds"
            | "amazon"
            | "amazonit"
            | "amazonae"
    )
}

fn fold_product_name(name: &str) -> String {
    let mut out = String::new();
    let mut gap = false;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            if gap && !out.is_empty() {
                out.push(' ');
            }
            gap = false;
            out.push(c.to_ascii_lowercase());
        } else {
            gap = true;
        }
    }
    out
}

pub fn round_money(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

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
}

impl StoreId {
    pub fn all() -> [StoreId; 7] {
        [
            Self::Carrefour,
            Self::UnionCoop,
            Self::Grandiose,
            Self::Spinneys,
            Self::Waitrose,
            Self::Mmi,
            Self::AfricanEastern,
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
            Self::AfricanEastern => "https://www.africanandeastern.com/",
        }
    }

    /// Remote cart and checkout are part of the store contract for the three
    /// shops that OmaBring can push a prepared cart to. The others keep a
    /// local cart and open the shop website at checkout.
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
            detail: "no store contract address is saved for this account".into(),
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
pub struct Session {
    pub token: String,
    pub user_id: i64,
    pub email: String,
    pub expires_at: String,
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

pub fn round_money(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

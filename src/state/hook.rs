//! Repository webhooks and the deliveries sent to them.

/// A repository webhook.
#[derive(Clone)]
pub struct Hook {
    pub id: u64,
    pub active: bool,
    /// Events delivered; `*` means all.
    pub events: Vec<String>,
    pub url: String,
    /// `json` or `form`.
    pub content_type: String,
    pub secret: Option<String>,
    pub insecure_ssl: String,
    pub created_at: String,
    pub updated_at: String,
}

impl Hook {
    pub fn wants(&self, event: &str) -> bool {
        self.active
            && (event == "ping"
                || self
                    .events
                    .iter()
                    .any(|wanted| wanted == event || wanted == "*"))
    }
}

/// One webhook delivery attempt, recorded for inspection.
#[derive(Clone, Debug)]
pub struct Delivery {
    pub id: u64,
    pub guid: String,
    pub hook_id: u64,
    pub repository_id: u64,
    pub event: String,
    pub url: String,
    pub headers: crate::webhooks::DeliveryHeaders,
    pub body: Vec<u8>,
    pub delivered_at: String,
    /// Set once the attempt finished with an HTTP response.
    pub status_code: Option<u16>,
    /// Set once the attempt failed without an HTTP response.
    pub error: Option<String>,
    pub duration_ms: u64,
}

impl Delivery {
    pub fn finished(&self) -> bool {
        self.status_code.is_some() || self.error.is_some()
    }

    /// The JSON payload, whether it was sent as JSON or as a form.
    pub fn payload(&self) -> serde_json::Value {
        let json = url::form_urlencoded::parse(&self.body)
            .find(|(key, _)| key == "payload")
            .map_or_else(
                || String::from_utf8_lossy(&self.body).into_owned(),
                |(_, value)| value.into_owned(),
            );
        serde_json::from_str(&json).unwrap_or(serde_json::Value::Null)
    }
}

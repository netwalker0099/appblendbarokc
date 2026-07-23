use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};

use super::{ContactPush, OrderPush, Squarespace, SyncError};

const BASE_URL: &str = "https://api.squarespace.com/1.0";
const USER_AGENT: &str = "blendbar-app/0.1 (+https://app.theblendbarokc.com)";

/// Live Squarespace client.
///
/// ⚠️ UNTESTED against the real service — no API key exists yet, so this path has
/// never made a real request. The request/response shapes below are a best-effort
/// match to the Squarespace API. Before trusting this in production, obtain a key
/// and verify, against current Squarespace API docs, the endpoint paths, the
/// request bodies, and which response field carries the created id. Today all sync
/// tests run through `MockSquarespace`; this exists so wiring a key in is the only
/// change needed to go live.
pub struct HttpSquarespace {
    client: Client,
    api_key: String,
}

impl HttpSquarespace {
    pub fn new(api_key: String) -> Self {
        let client = Client::builder()
            .user_agent(USER_AGENT)
            .build()
            .expect("failed to build reqwest client");
        Self { client, api_key }
    }

    async fn post(&self, path: &str, body: Value) -> Result<Value, SyncError> {
        let resp = self
            .client
            .post(format!("{BASE_URL}{path}"))
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| SyncError::Transport(e.to_string()))?;

        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if status.is_success() {
            Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
        } else {
            let code = status.as_u16();
            // 5xx and 429 are transient and worth retrying; other 4xx are our
            // fault (bad request, auth) and won't fix themselves on retry.
            let retryable = status.is_server_error() || code == 429;
            Err(SyncError::Api {
                status: code,
                body: text,
                retryable,
            })
        }
    }
}

fn read_id(resp: &Value) -> Result<String, SyncError> {
    resp.get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| SyncError::Config(format!("no `id` field in response: {resp}")))
}

#[async_trait]
impl Squarespace for HttpSquarespace {
    fn name(&self) -> &'static str {
        "squarespace"
    }

    async fn upsert_contact(&self, contact: &ContactPush) -> Result<String, SyncError> {
        let body = json!({
            "emailAddress": contact.email,
            "firstName": contact.name,
            "acceptsMarketing": contact.marketing_consent,
        });
        let resp = self.post("/profiles", body).await?;
        read_id(&resp)
    }

    async fn create_order(&self, order: &OrderPush) -> Result<String, SyncError> {
        let body = json!({
            "customerEmail": order.email,
            "lineItems": [{ "description": order.description, "quantity": 1 }],
            "grandTotal": order.amount,
            "externalOrderReference": order.id,
        });
        let resp = self.post("/commerce/orders", body).await?;
        read_id(&resp)
    }
}

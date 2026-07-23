//! The Squarespace sync boundary. Everything the app does to Squarespace goes
//! through the [`Squarespace`] trait, so the sync worker can be driven by an
//! in-process mock (when no API key is configured) or the real HTTP client
//! (when one is) without any other code changing.

mod http;
mod mock;

pub use http::HttpSquarespace;
pub use mock::MockSquarespace;

use std::sync::Arc;

use async_trait::async_trait;
use rust_decimal::Decimal;
use uuid::Uuid;

/// A customer to upsert into Squarespace Contacts. Deliberately a plain struct,
/// not the DB model, so the boundary doesn't drag persistence types along.
#[derive(Debug)]
pub struct ContactPush {
    pub id: Uuid,
    pub email: String,
    pub name: Option<String>,
    pub marketing_consent: bool,
}

/// Authoritative order state fetched back from Squarespace during reconciliation
/// (Milestone 6). A webhook only carries an order id, so we fetch the real status.
#[derive(Debug)]
pub struct RemoteOrder {
    /// Squarespace fulfilment status, e.g. "PENDING" / "FULFILLED" / "CANCELED".
    pub fulfillment_status: String,
    pub paid: bool,
    pub grand_total: Option<Decimal>,
}

/// An order to create in Squarespace Orders.
#[derive(Debug)]
pub struct OrderPush {
    pub id: Uuid,
    pub email: String,
    pub name: Option<String>,
    pub order_type: String,
    pub size: String,
    pub status: String,
    pub amount: Option<Decimal>,
    pub description: String,
}

#[derive(Debug)]
pub enum SyncError {
    /// Transport/connection problem — always worth retrying.
    Transport(String),
    /// Squarespace answered with an error status. `retryable` is true for 5xx and
    /// 429 (transient), false for other 4xx we can't fix by retrying.
    Api {
        status: u16,
        body: String,
        retryable: bool,
    },
    /// Misconfiguration or bad data (missing key, entity vanished) — retrying
    /// won't help until something is fixed upstream.
    Config(String),
}

impl SyncError {
    pub fn retryable(&self) -> bool {
        match self {
            SyncError::Transport(_) => true,
            SyncError::Api { retryable, .. } => *retryable,
            SyncError::Config(_) => false,
        }
    }
}

impl std::fmt::Display for SyncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SyncError::Transport(msg) => write!(f, "transport error: {msg}"),
            SyncError::Api { status, body, .. } => write!(f, "api error {status}: {body}"),
            SyncError::Config(msg) => write!(f, "config error: {msg}"),
        }
    }
}

#[async_trait]
pub trait Squarespace: Send + Sync {
    /// Short label for logs and the sync-status endpoint ("mock" or "squarespace").
    fn name(&self) -> &'static str;

    /// Create or update the contact, returning its Squarespace id.
    async fn upsert_contact(&self, contact: &ContactPush) -> Result<String, SyncError>;

    /// Create the order, returning its Squarespace id.
    async fn create_order(&self, order: &OrderPush) -> Result<String, SyncError>;

    /// Fetch authoritative state for an order by its Squarespace id — used by the
    /// webhook receiver to reconcile, since the webhook itself only carries the id.
    async fn get_order(&self, squarespace_order_id: &str) -> Result<RemoteOrder, SyncError>;
}

/// Pick the live HTTP client when `SQUARESPACE_API_KEY` is set, otherwise the
/// in-process mock. This is how the app runs today: no key on the box, so the
/// mock backs every sync and the outbox/worker path is fully exercised without
/// touching Squarespace.
pub fn from_env() -> Arc<dyn Squarespace> {
    match std::env::var("SQUARESPACE_API_KEY") {
        Ok(key) if !key.trim().is_empty() => {
            tracing::info!("squarespace: live HTTP client (API key present)");
            Arc::new(HttpSquarespace::new(key))
        }
        _ => {
            tracing::warn!("squarespace: SQUARESPACE_API_KEY unset — using in-process mock");
            Arc::new(MockSquarespace::default())
        }
    }
}

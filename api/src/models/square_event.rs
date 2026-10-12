use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

/// A recorded inbound Square webhook (payload column omitted — this is the
/// summary shape returned by `GET /api/square/events`).
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct SquareWebhookEvent {
    pub id: Uuid,
    pub square_event_id: String,
    pub event_type: String,
    pub square_order_id: Option<String>,
    pub square_payment_id: Option<String>,
    pub status: String,
    pub matched_cart_id: Option<Uuid>,
    pub error: Option<String>,
    pub received_at: DateTime<Utc>,
    pub processed_at: Option<DateTime<Utc>>,
}

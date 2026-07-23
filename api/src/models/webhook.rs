use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

/// A recorded inbound webhook (payload column omitted — this is the summary shape
/// returned by `GET /api/webhooks/recent`).
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct WebhookEvent {
    pub id: Uuid,
    pub squarespace_notification_id: String,
    pub topic: String,
    pub squarespace_order_id: Option<String>,
    pub status: String,
    pub matched_order_id: Option<Uuid>,
    pub error: Option<String>,
    pub received_at: DateTime<Utc>,
    pub processed_at: Option<DateTime<Utc>>,
}

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

/// A stored reconciliation snapshot (without the full report blob).
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct ReconciliationRun {
    pub id: Uuid,
    pub period_start: DateTime<Utc>,
    pub period_end: DateTime<Utc>,
    pub local_total_cents: i64,
    pub square_total_cents: i64,
    pub matched_count: i32,
    pub mismatched_count: i32,
    pub missing_in_square_count: i32,
    pub missing_locally_count: i32,
    pub run_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

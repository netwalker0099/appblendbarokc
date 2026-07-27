use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Where a cart sits between "operator built it" and "money landed".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum CartStatus {
    /// Built locally, editable, nothing sent to Square yet.
    Open,
    /// Pushed to Square, hosted link live, waiting on the customer.
    PendingPayment,
    /// Square reported a COMPLETED payment.
    Paid,
    /// Abandoned before payment.
    Canceled,
    /// Was paid, then refunded in Square.
    Refunded,
}

impl CartStatus {
    /// Only an open cart can be edited or checked out; anything further along has
    /// a Square order behind it and must not be mutated locally.
    pub fn is_open(&self) -> bool {
        matches!(self, CartStatus::Open)
    }
}

/// A cart: one checkout, one Square order, one payment.
///
/// Money is in integer cents throughout, matching Square. See
/// [`crate::square::money`] for the conversion from the operator-entered decimal.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct Cart {
    pub id: Uuid,
    pub customer_id: Uuid,
    pub status: CartStatus,
    pub currency: String,
    /// What we asked Square to charge.
    pub total_cents: i64,
    /// What Square reported collecting. `None` until a payment settles.
    pub paid_cents: Option<i64>,
    pub square_order_id: Option<String>,
    pub square_payment_link_id: Option<String>,
    pub square_payment_id: Option<String>,
    pub checkout_url: Option<String>,
    /// Reused on every Square create call for this cart so a retry after a
    /// timeout returns the original payment link. Internal — never sent to a
    /// browser, since replaying it would let a client steer Square's dedup.
    #[serde(skip_serializing)]
    pub idempotency_key: String,
    pub note: Option<String>,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub checkout_at: Option<DateTime<Utc>>,
    pub paid_at: Option<DateTime<Utc>>,
}

/// One line of a cart. `order_id` is set when the line sells a blend, and null
/// for money that isn't a bottle (event deposit, rush fee, hotel line).
///
/// `name` and `unit_amount_cents` are snapshots taken when the line was added:
/// repricing a scent next month must not rewrite what a customer was charged.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct CartItem {
    pub id: Uuid,
    pub cart_id: Uuid,
    pub order_id: Option<Uuid>,
    pub name: String,
    pub quantity: i32,
    pub unit_amount_cents: i64,
    /// `blend` | `event_deposit` | `fee` | `other`. Set explicitly rather than
    /// inferred from `name`: a settled `event_deposit` is what marks an event as
    /// booked, and matching that on free text would break on a retyped label.
    pub kind: String,
    pub created_at: DateTime<Utc>,
}

impl CartItem {
    pub fn line_total_cents(&self) -> i64 {
        self.unit_amount_cents * self.quantity as i64
    }
}

/// A cart plus its lines — the shape every cart endpoint returns.
#[derive(Debug, Serialize)]
pub struct CartDetail {
    #[serde(flatten)]
    pub cart: Cart,
    pub items: Vec<CartItem>,
}

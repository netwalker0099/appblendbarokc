use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::Serialize;
use uuid::Uuid;

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct Scent {
    pub id: Uuid,
    pub name: String,
    pub active: bool,
    // Per-size retail prices; null until an admin sets them.
    pub price_oz3_4: Option<Decimal>,
    pub price_oz1_7: Option<Decimal>,
    pub price_roller: Option<Decimal>,
    pub price_spray: Option<Decimal>,
    pub created_at: DateTime<Utc>,
}

/// One ingredient in a scent's house formula. `amount_ml` is the base 3.4oz
/// amount, same convention as `MixItem`.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct ScentItem {
    pub scent_id: Uuid,
    pub ingredient_id: Uuid,
    pub amount_ml: Decimal,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct CustomerScentPreference {
    pub customer_id: Uuid,
    pub scent_id: Uuid,
    pub created_at: DateTime<Utc>,
}

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::Serialize;
use uuid::Uuid;

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct Scent {
    pub id: Uuid,
    pub name: String,
    pub active: bool,
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

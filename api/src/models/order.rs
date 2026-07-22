use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum OrderType {
    SetPerfume,
    CustomMix,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum OrderStatus {
    Lead,
    Paid,
    Fulfilled,
}

/// The 1.7oz amount is half the base formula and the roller is a tenth of it;
/// this only records which bottle was ordered, not the derived amounts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text")]
pub enum BottleSize {
    #[sqlx(rename = "oz3_4")]
    #[serde(rename = "oz3_4")]
    Oz3_4,
    #[sqlx(rename = "oz1_7")]
    #[serde(rename = "oz1_7")]
    Oz1_7,
    #[sqlx(rename = "roller")]
    #[serde(rename = "roller")]
    Roller,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct Order {
    pub id: Uuid,
    pub customer_id: Uuid,
    #[sqlx(rename = "type")]
    pub order_type: OrderType,
    pub size: BottleSize,
    pub mix_id: Option<Uuid>,
    pub scent_id: Option<Uuid>,
    pub status: OrderStatus,
    pub squarespace_order_id: Option<String>,
    pub external_ref: Option<String>,
    pub amount: Option<Decimal>,
    pub idempotency_key: Option<String>,
    pub created_at: DateTime<Utc>,
}

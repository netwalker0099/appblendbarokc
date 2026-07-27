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

/// The 1.7oz amount is half the base formula; the roller and the spray are a
/// tenth of it. This only records which bottle was ordered, not the derived
/// amounts.
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
    /// 10 ml with a spray top. Physically the same volume as the roller and so
    /// the same pour — only the closure and the price differ.
    #[sqlx(rename = "spray")]
    #[serde(rename = "spray")]
    Spray,
}

impl OrderType {
    pub fn label(&self) -> &'static str {
        match self {
            OrderType::SetPerfume => "Set perfume",
            OrderType::CustomMix => "Custom mix",
        }
    }
}

impl BottleSize {
    pub fn label(&self) -> &'static str {
        match self {
            BottleSize::Oz3_4 => "3.4 oz",
            BottleSize::Oz1_7 => "1.7 oz",
            BottleSize::Roller => "Roller",
            BottleSize::Spray => "Spray (10 ml)",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_size_has_a_distinct_human_label() {
        // Roller and Spray share a pour but must never share a label — an
        // operator reading "Roller" on a spray order would build the wrong thing.
        let labels = [
            BottleSize::Oz3_4.label(),
            BottleSize::Oz1_7.label(),
            BottleSize::Roller.label(),
            BottleSize::Spray.label(),
        ];
        let mut unique = labels.to_vec();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), labels.len(), "duplicate size label: {labels:?}");
        assert_eq!(BottleSize::Spray.label(), "Spray (10 ml)");
    }

    #[test]
    fn size_wire_format_is_stable() {
        // These strings are persisted in orders.size and sent by the public
        // share page; changing one silently breaks stored rows and live clients.
        for (size, wire) in [
            (BottleSize::Oz3_4, "\"oz3_4\""),
            (BottleSize::Oz1_7, "\"oz1_7\""),
            (BottleSize::Roller, "\"roller\""),
            (BottleSize::Spray, "\"spray\""),
        ] {
            assert_eq!(serde_json::to_string(&size).unwrap(), wire);
        }
    }
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
    /// How many bottles of this blend in this size. Two of the same thing is one
    /// order of quantity 2 — the blend was mixed once.
    pub quantity: i32,
    pub external_ref: Option<String>,
    pub amount: Option<Decimal>,
    /// The submission that produced this order; idempotency lives there, since
    /// one submission can legitimately create several orders.
    pub intake_id: Option<Uuid>,
    /// Set when the order came from a package deal.
    pub bundle_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

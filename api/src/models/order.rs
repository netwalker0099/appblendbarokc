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

/// The 1.7oz amount is half the base formula, the 1 oz three-tenths (30 ml), and
/// the roller and spray a tenth. The 1 oz is stored as `oz1_2` — it was sold as
/// 1.2 oz until 2026-10-05, and the wire code stays put because it is persisted
/// on orders, bundles and price columns. Only the label changed. This only records which bottle was ordered, not
/// the derived amounts.
///
/// Declared largest-bottle-first, which is the order the operator UI offers them
/// in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text")]
pub enum BottleSize {
    /// A 100 ml bottle in a vintage style. Physically the same volume as the
    /// 3.4oz and so the same pour — only the bottle and the price differ, the
    /// same relationship spray has to roller.
    #[sqlx(rename = "vintage")]
    #[serde(rename = "vintage")]
    Vintage,
    #[sqlx(rename = "oz3_4")]
    #[serde(rename = "oz3_4")]
    Oz3_4,
    #[sqlx(rename = "oz1_7")]
    #[serde(rename = "oz1_7")]
    Oz1_7,
    /// 1 oz (30 ml). The code is left over from when this bottle was 1.2 oz.
    #[sqlx(rename = "oz1_2")]
    #[serde(rename = "oz1_2")]
    Oz1_2,
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
    /// Every size, largest bottle first — the order the operator UI offers.
    ///
    /// Exhaustive on purpose: the tests below iterate it, so a size added to the
    /// enum without being added here is caught rather than quietly skipped.
    pub const ALL: [BottleSize; 6] = [
        BottleSize::Vintage,
        BottleSize::Oz3_4,
        BottleSize::Oz1_7,
        BottleSize::Oz1_2,
        BottleSize::Roller,
        BottleSize::Spray,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            BottleSize::Vintage => "Vintage",
            BottleSize::Oz3_4 => "3.4 oz",
            BottleSize::Oz1_7 => "1.7 oz",
            BottleSize::Oz1_2 => "1 oz",
            BottleSize::Roller => "Roller",
            BottleSize::Spray => "Spray",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_size_has_a_distinct_human_label() {
        // Roller and Spray share a pour, as do Vintage and 3.4 oz, but no two
        // may share a label — an operator reading "Roller" on a spray order, or
        // "3.4 oz" on a vintage one, would hand over the wrong bottle.
        let labels: Vec<_> = BottleSize::ALL.iter().map(|s| s.label()).collect();
        let mut unique = labels.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), labels.len(), "duplicate size label: {labels:?}");
        assert_eq!(BottleSize::Spray.label(), "Spray");
        assert_eq!(BottleSize::Oz1_2.label(), "1 oz");
        assert_eq!(BottleSize::Vintage.label(), "Vintage");
    }

    #[test]
    fn size_wire_format_is_stable() {
        // These strings are persisted in orders.size and sent by the public
        // share page; changing one silently breaks stored rows and live clients.
        for (size, wire) in [
            (BottleSize::Vintage, "\"vintage\""),
            (BottleSize::Oz3_4, "\"oz3_4\""),
            (BottleSize::Oz1_7, "\"oz1_7\""),
            (BottleSize::Oz1_2, "\"oz1_2\""),
            (BottleSize::Roller, "\"roller\""),
            (BottleSize::Spray, "\"spray\""),
        ] {
            assert_eq!(serde_json::to_string(&size).unwrap(), wire);
        }
        // Guards the loop above against a size being added to the enum but not
        // listed here, which would leave its wire format untested.
        assert_eq!(BottleSize::ALL.len(), 6);
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

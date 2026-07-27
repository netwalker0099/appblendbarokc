use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::Serialize;
use uuid::Uuid;

/// The maximum number of distinct ingredients a mix may contain, enforced in the
/// intake/mix-write handlers rather than in the database.
pub const MAX_MIX_INGREDIENTS: usize = 8;

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct Mix {
    pub id: Uuid,
    pub customer_id: Uuid,
    pub name: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// amount_ml is the ingredient's amount in the base 3.4oz formula. The 1.7oz
/// bottle is half of this; the roller and the 10 ml spray are a tenth. All are
/// derived at read/order time, never stored per size.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct MixItem {
    pub mix_id: Uuid,
    pub ingredient_id: Uuid,
    pub amount_ml: Decimal,
}

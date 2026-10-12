use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::Serialize;
use uuid::Uuid;

use crate::error::AppError;

/// The maximum number of distinct ingredients a mix may contain, enforced in the
/// intake/mix-write handlers rather than in the database.
pub const MAX_MIX_INGREDIENTS: usize = 8;

/// What a formula must total, in ml. Amounts are always entered at the 3.4 oz
/// base (a 100 ml bottle) and every other size is scaled from it — so a base
/// that totals exactly 100 ml fills every bottle exactly: 1.7 oz 50 ml, 1 oz
/// 30 ml, roller and spray 10 ml. Anything else under- or over-fills all of
/// them at once.
pub const BASE_FORMULA_ML: i64 = 100;

/// Refuse a formula whose amounts don't total [`BASE_FORMULA_ML`]. Exact, not
/// approximately: amounts are decimals, so there is no float rounding to
/// forgive, and a blend 0.5 ml short is a blend 0.5 ml short.
///
/// Amounts finer than 0.01 ml are refused too: the column stores two decimal
/// places, so 33.333 + 33.333 + 33.334 would pass here and then be saved as a
/// 99.99 ml formula.
pub fn check_formula_total(amounts: impl IntoIterator<Item = Decimal>) -> Result<(), AppError> {
    let amounts: Vec<Decimal> = amounts.into_iter().collect();
    if amounts.iter().any(|a| a.normalize().scale() > 2) {
        return Err(AppError::BadRequest(
            "amounts can be measured to 0.01 ml at most".into(),
        ));
    }
    let total: Decimal = amounts.into_iter().sum();
    if total == Decimal::from(BASE_FORMULA_ML) {
        return Ok(());
    }
    Err(AppError::BadRequest(format!(
        "the amounts add up to {} ml — a blend must total exactly {BASE_FORMULA_ML} ml \
         at the 3.4 oz base, which every bottle size is scaled from",
        total.normalize()
    )))
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct Mix {
    pub id: Uuid,
    pub customer_id: Uuid,
    pub name: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// amount_ml is the ingredient's amount in the base 3.4oz formula. The 1.7oz
/// bottle is half of this, the 1 oz three-tenths, and the roller and spray (both
/// 10 ml) a tenth. All are
/// derived at read/order time, never stored per size.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct MixItem {
    pub mix_id: Uuid,
    pub ingredient_id: Uuid,
    pub amount_ml: Decimal,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn dec(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    #[test]
    fn exactly_100ml_passes() {
        assert!(check_formula_total([dec("60"), dec("25.5"), dec("14.5")]).is_ok());
        assert!(check_formula_total([dec("100.00")]).is_ok());
    }

    #[test]
    fn anything_else_is_refused_and_says_the_total() {
        let err = check_formula_total([dec("60"), dec("39.5")]).unwrap_err();
        assert!(matches!(&err, AppError::BadRequest(m) if m.contains("99.5 ml")));
        assert!(check_formula_total([dec("60"), dec("40.01")]).is_err());
        assert!(check_formula_total(Vec::<Decimal>::new()).is_err());
    }

    #[test]
    fn amounts_finer_than_the_column_are_refused() {
        // Totals 100 as sent, but would be stored as 33.33 × 3 = 99.99.
        assert!(check_formula_total([dec("33.333"), dec("33.333"), dec("33.334")]).is_err());
        assert!(check_formula_total([dec("33.33"), dec("33.33"), dec("33.34")]).is_ok());
    }
}

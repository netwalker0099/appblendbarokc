use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::Serialize;
use uuid::Uuid;

use crate::error::AppError;

/// The maximum number of distinct ingredients a mix may contain, enforced in the
/// intake/mix-write handlers rather than in the database.
pub const MAX_MIX_INGREDIENTS: usize = 8;

/// What a formula must total, in ml, at the 3.4 oz base (a 100 ml bottle).
/// Every other size is scaled from it, so a base that totals 100 ml fills every
/// bottle exactly: 1.7 oz 50 ml, 1 oz 30 ml, roller and spray 10 ml.
///
/// Staff enter a blend in the ml of the bottle they are filling; the screen
/// scales that to the base before sending it, so this check sees base amounts.
pub const BASE_FORMULA_ML: i64 = 100;

/// Decimal places formula amounts are stored at (`numeric(8,4)`).
const AMOUNT_DP: u32 = 4;

/// Refuse a formula whose base amounts don't total [`BASE_FORMULA_ML`].
///
/// Exact apart from one allowance: a blend entered for the 1 oz bottle scales
/// to the base by 100/30, which repeats (10 ml -> 33.3333... ml), so each
/// amount arrives rounded to four places and may be off by up to half of the
/// last place. The total may therefore differ from 100 ml by at most 0.00005 ml
/// per ingredient, and no more. A blend 0.01 ml short is still refused.
///
/// Amounts finer than four decimal places are refused: the column can't hold
/// them, so they would be stored as something else.
pub fn check_formula_total(amounts: impl IntoIterator<Item = Decimal>) -> Result<(), AppError> {
    let amounts: Vec<Decimal> = amounts.into_iter().collect();
    if amounts.iter().any(|a| a.normalize().scale() > AMOUNT_DP) {
        return Err(AppError::BadRequest(
            "amounts can be stored to 0.0001 ml at most".into(),
        ));
    }
    if amounts.is_empty() {
        return Err(AppError::BadRequest(format!(
            "a blend needs ingredients totalling {BASE_FORMULA_ML} ml at the 3.4 oz base"
        )));
    }
    let total: Decimal = amounts.iter().copied().sum();
    let allowance = Decimal::new(5, AMOUNT_DP + 1) * Decimal::from(amounts.len() as i64);
    if (total - Decimal::from(BASE_FORMULA_ML)).abs() <= allowance {
        return Ok(());
    }
    Err(AppError::BadRequest(format!(
        "the amounts add up to {} ml at the 3.4 oz base — a blend must total exactly \
         {BASE_FORMULA_ML} ml there, which is 50 ml in a 1.7 oz bottle, 30 ml in a 1 oz \
         and 10 ml in a roller or spray",
        total.round_dp(2).normalize()
    )))
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct Mix {
    pub id: Uuid,
    pub customer_id: Uuid,
    pub name: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// amount_ml is the ingredient's amount in the base 3.4oz formula, to four
/// decimal places. The 1.7oz bottle is half of this, the 1 oz three-tenths, and
/// the roller and spray (both 10 ml) a tenth. Staff may enter a blend in any of
/// those sizes; it is scaled to the base before it is stored, and every size is
/// derived from the base at read/order time.
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
        assert!(check_formula_total([dec("33.33333"), dec("33.33333"), dec("33.33334")]).is_err());
        assert!(check_formula_total([dec("33.333"), dec("33.333"), dec("33.334")]).is_ok());
    }

    #[test]
    fn a_1oz_blend_scaled_to_the_base_passes() {
        // 10 + 10 + 10 ml in a 1 oz (30 ml) bottle is 33.3333... each at the
        // base, which arrives rounded to four places: 99.9999 in total.
        assert!(check_formula_total([dec("33.3333"), dec("33.3333"), dec("33.3333")]).is_ok());
        // 9.99 + 10 + 10.01 ml -> 33.3 + 33.3333 + 33.3667 = 100.0000.
        assert!(check_formula_total([dec("33.3"), dec("33.3333"), dec("33.3667")]).is_ok());
    }

    #[test]
    fn the_rounding_allowance_is_tiny() {
        // Three ingredients may be off by 0.00015 ml in total, not 0.0002.
        assert!(check_formula_total([dec("33.3333"), dec("33.3333"), dec("33.3332")]).is_err());
        // A whole hundredth short is a real shortfall.
        assert!(check_formula_total([dec("33.33"), dec("33.33"), dec("33.33")]).is_err());
    }

    #[test]
    fn roller_and_half_size_blends_scale_exactly() {
        // Roller: 3.33 + 3.33 + 3.34 ml -> x10 at the base.
        assert!(check_formula_total([dec("33.3"), dec("33.3"), dec("33.4")]).is_ok());
        // 1.7 oz: 25 + 15.5 + 9.5 ml -> x2.
        assert!(check_formula_total([dec("50"), dec("31"), dec("19")]).is_ok());
    }
}

//! What does this cost?
//!
//! Prices live in the catalogue — per-size on each scent, and a global per-size
//! price for bespoke blends in `settings`. Before this module existed, nothing
//! consulted them: intake stored whatever amount the operator typed into an
//! "Amount (optional)" box, so an order taken without manually retyping the price
//! was saved with `amount = null` and could not be sold. The prices configured in
//! Admin were decoration.
//!
//! One function answers the question, and intake, cart building and the public
//! share-page checkout all defer to it.

use rust_decimal::Decimal;
use uuid::Uuid;

use crate::models::order::{BottleSize, OrderType};

/// The catalogue price for one bottle, or `None` when that combination is not
/// priced (which is how a size is marked "not sold").
pub async fn catalog_price<'e, E>(
    exec: E,
    order_type: OrderType,
    size: BottleSize,
    scent_id: Option<Uuid>,
) -> Result<Option<Decimal>, sqlx::Error>
where
    E: sqlx::PgExecutor<'e>,
{
    match order_type {
        // A set perfume is priced per scent: two house scents can cost different
        // amounts in the same bottle.
        OrderType::SetPerfume => {
            let Some(scent_id) = scent_id else {
                return Ok(None);
            };
            let column = match size {
                BottleSize::Oz3_4 => "price_oz3_4",
                BottleSize::Oz1_7 => "price_oz1_7",
                BottleSize::Roller => "price_roller",
                BottleSize::Spray => "price_spray",
            };
            sqlx::query_scalar(&format!("select {column} from scents where id = $1"))
                .bind(scent_id)
                .fetch_optional(exec)
                .await
                .map(Option::flatten)
        }
        // Bespoke blends are priced uniformly by size — the work is the same
        // whatever goes in the bottle — so the price is global, not per mix.
        OrderType::CustomMix => {
            let column = match size {
                BottleSize::Oz3_4 => "custom_price_oz3_4",
                BottleSize::Oz1_7 => "custom_price_oz1_7",
                BottleSize::Roller => "custom_price_roller",
                BottleSize::Spray => "custom_price_spray",
            };
            sqlx::query_scalar(&format!("select {column} from settings where id = true"))
                .fetch_optional(exec)
                .await
                .map(Option::flatten)
        }
    }
}

/// Split a bundle's fixed price across its lines.
///
/// A package deal is sold at one headline price, but each bottle still needs its
/// own amount so the order history and the cart add up. The split is weighted by
/// each line's catalogue price, so a 3.4oz in the bundle carries more of the cost
/// than a roller does.
///
/// Works in whole cents and puts every leftover cent on the largest line, so the
/// parts always sum to exactly the bundle price — never a cent over or under.
pub fn split_bundle_price(total_cents: i64, weights: &[i64]) -> Vec<i64> {
    let n = weights.len();
    if n == 0 {
        return Vec::new();
    }
    let weight_sum: i64 = weights.iter().sum();

    // No catalogue prices to weight by (every component unpriced): split evenly.
    if weight_sum <= 0 {
        let base = total_cents / n as i64;
        let mut out = vec![base; n];
        out[0] += total_cents - base * n as i64;
        return out;
    }

    let mut out: Vec<i64> = weights
        .iter()
        .map(|w| total_cents * w / weight_sum)
        .collect();

    // Integer division always rounds down, so there is a remainder to place.
    let placed: i64 = out.iter().sum();
    let remainder = total_cents - placed;
    if remainder != 0 {
        let biggest = weights
            .iter()
            .enumerate()
            .max_by_key(|(_, w)| **w)
            .map(|(i, _)| i)
            .unwrap_or(0);
        out[biggest] += remainder;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_always_sums_to_the_bundle_price() {
        // The property that matters: a customer is charged the advertised price,
        // whatever the rounding does to the individual lines.
        for total in [10_000, 9_999, 1, 0, 7_777, 12_345] {
            for weights in [
                vec![6000, 1800],
                vec![6000, 6000, 1800],
                vec![1],
                vec![3300, 3300, 3300],
                vec![0, 0],
            ] {
                let parts = split_bundle_price(total, &weights);
                assert_eq!(
                    parts.iter().sum::<i64>(),
                    total,
                    "total {total} weights {weights:?} split to {parts:?}"
                );
                assert_eq!(parts.len(), weights.len());
            }
        }
    }

    #[test]
    fn split_is_weighted_not_even() {
        // A 3.4oz should carry more of a bundle than a roller.
        let parts = split_bundle_price(10_000, &[9500, 1000]);
        assert!(parts[0] > parts[1], "got {parts:?}");
    }

    #[test]
    fn split_handles_unpriced_components_evenly() {
        let parts = split_bundle_price(9_000, &[0, 0, 0]);
        assert_eq!(parts, vec![3000, 3000, 3000]);
    }

    #[test]
    fn split_of_nothing_is_nothing() {
        assert!(split_bundle_price(1000, &[]).is_empty());
    }

    #[test]
    fn remainder_lands_on_the_largest_line() {
        // 100 split 2:1 is 66.67/33.33; the stray cent goes to the bigger line.
        let parts = split_bundle_price(100, &[2, 1]);
        assert_eq!(parts.iter().sum::<i64>(), 100);
        assert_eq!(parts[0], 67);
    }
}

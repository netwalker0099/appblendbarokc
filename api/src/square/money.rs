//! Conversion between the `numeric(10,2)` amounts operators type and the integer
//! minor units Square requires.
//!
//! This is small enough to look unnecessary and important enough to be the one
//! part of the Square integration with real test coverage. Getting it wrong does
//! not throw — it silently charges the customer 100x too little (or too much).

use rust_decimal::prelude::ToPrimitive;
use rust_decimal::{Decimal, RoundingStrategy};

/// Currencies whose minor unit is not 1/100 of the major unit are not supported;
/// the shop prices in USD and Square is told so explicitly.
pub const DEFAULT_CURRENCY: &str = "USD";

/// `60.00` -> `6000`. Rounds half-to-even at 2dp, which is what you want for
/// money: half-up biases every borderline cent in the same direction, and over a
/// season of blends that bias is a real (small) number.
///
/// Returns `None` for negatives and for values too large to be a sane price —
/// both indicate a bug or bad input upstream, and neither should reach Square.
pub fn to_cents(amount: Decimal) -> Option<i64> {
    if amount.is_sign_negative() {
        return None;
    }
    let scaled = (amount * Decimal::from(100))
        .round_dp_with_strategy(0, RoundingStrategy::MidpointNearestEven);
    let cents = scaled.to_i64()?;
    // A single line over $1,000,000 is not a perfume order; refuse rather than
    // hand Square something absurd.
    if cents > 100_000_000 {
        return None;
    }
    Some(cents)
}

/// Render cents as a plain `$60.00` for logs, reports, and the admin UI.
pub fn format_cents(cents: i64, currency: &str) -> String {
    let sign = if cents < 0 { "-" } else { "" };
    let abs = cents.abs();
    let symbol = if currency.eq_ignore_ascii_case("USD") {
        "$"
    } else {
        ""
    };
    let rendered = format!("{sign}{symbol}{}.{:02}", abs / 100, abs % 100);
    if symbol.is_empty() {
        format!("{rendered} {currency}")
    } else {
        rendered
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn dec(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    #[test]
    fn converts_typical_prices() {
        assert_eq!(to_cents(dec("60.00")), Some(6000));
        assert_eq!(to_cents(dec("60")), Some(6000));
        assert_eq!(to_cents(dec("0.01")), Some(1));
        assert_eq!(to_cents(dec("0")), Some(0));
        assert_eq!(to_cents(dec("1234.56")), Some(123456));
    }

    #[test]
    fn does_not_lose_the_trailing_cent() {
        // The classic float bug: 0.1 + 0.2 style drift turning 19.99 into 1998.
        assert_eq!(to_cents(dec("19.99")), Some(1999));
        assert_eq!(to_cents(dec("0.29")), Some(29));
        assert_eq!(to_cents(dec("8.15")), Some(815));
    }

    #[test]
    fn rounds_half_to_even_at_the_sub_cent() {
        // A 50% deposit on an odd invoice is exactly how thirds of a cent appear.
        assert_eq!(to_cents(dec("0.125")), Some(12));
        assert_eq!(to_cents(dec("0.135")), Some(14));
        assert_eq!(to_cents(dec("2.005")), Some(200));
        assert_eq!(to_cents(dec("2.015")), Some(202));
    }

    #[test]
    fn rejects_negative_and_absurd_amounts() {
        assert_eq!(to_cents(dec("-1.00")), None);
        assert_eq!(to_cents(dec("-0.01")), None);
        assert_eq!(to_cents(dec("2000000.00")), None);
    }

    #[test]
    fn trailing_zeros_and_scale_do_not_change_the_value() {
        // The same price can arrive from Postgres at different scales; all of
        // these are $60 and must produce identical cents.
        for s in ["60", "60.0", "60.00", "60.000"] {
            assert_eq!(to_cents(dec(s)), Some(6000), "failed for {s}");
        }
    }

    #[test]
    fn formats_for_humans() {
        assert_eq!(format_cents(6000, "USD"), "$60.00");
        assert_eq!(format_cents(5, "USD"), "$0.05");
        assert_eq!(format_cents(0, "USD"), "$0.00");
        assert_eq!(format_cents(-2550, "USD"), "-$25.50");
        assert_eq!(format_cents(1999, "CAD"), "19.99 CAD");
    }
}

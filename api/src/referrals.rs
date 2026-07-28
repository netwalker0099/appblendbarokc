//! Referral discounts and the coupons they earn.
//!
//! Buy through someone's shared link and you get money off. Once that purchase
//! actually **pays**, the person whose link you used earns a coupon for the same
//! kind of amount.
//!
//! The timing matters: the buyer's discount is applied at checkout, but the
//! sharer's reward is only issued on settlement. Issuing it at checkout would
//! mint coupons for abandoned baskets, and abandoning a basket is free.

use rand::Rng;
use rust_decimal::Decimal;
use sqlx::PgPool;
use uuid::Uuid;

/// Unambiguous alphabet: no 0/O, no 1/I/L. These get read aloud and typed by
/// hand, so the pairs people confuse are simply absent.
const CODE_ALPHABET: &[u8] = b"23456789ABCDEFGHJKMNPQRSTUVWXYZ";
const REFERRAL_CODE_LEN: usize = 6;
const COUPON_CODE_LEN: usize = 8;

fn random_code(len: usize) -> String {
    let mut rng = rand::thread_rng();
    (0..len)
        .map(|_| CODE_ALPHABET[rng.gen_range(0..CODE_ALPHABET.len())] as char)
        .collect()
}

/// The configured referral amounts.
#[derive(Debug, Clone, Copy)]
pub struct ReferralSettings {
    pub enabled: bool,
    pub discount_cents: i64,
    pub reward_cents: i64,
    pub coupon_expiry_days: i32,
}

pub async fn settings(db: &PgPool) -> Result<ReferralSettings, sqlx::Error> {
    let row: (bool, i64, i64, i32) = sqlx::query_as(
        "select referral_enabled, referral_discount_cents, referral_reward_cents, \
         coupon_expiry_days from settings where id = true",
    )
    .fetch_one(db)
    .await?;
    Ok(ReferralSettings {
        enabled: row.0,
        discount_cents: row.1,
        reward_cents: row.2,
        coupon_expiry_days: row.3,
    })
}

/// This customer's referral code, creating one if they don't have it yet.
///
/// Lazy rather than issued to everyone at signup: most customers never share,
/// and an unused code is just a row that has to stay unique forever.
pub async fn code_for(db: &PgPool, customer_id: Uuid) -> Result<String, sqlx::Error> {
    if let Some(existing) =
        sqlx::query_scalar::<_, Option<String>>("select referral_code from customers where id = $1")
            .bind(customer_id)
            .fetch_optional(db)
            .await?
            .flatten()
    {
        return Ok(existing);
    }

    // Retry on collision rather than trusting one draw. 31^6 is large, but
    // "large" is not "never" and a unique violation here would surface as a 500
    // on someone pressing Share.
    for _ in 0..8 {
        let candidate = random_code(REFERRAL_CODE_LEN);
        let updated = sqlx::query_scalar::<_, String>(
            "update customers set referral_code = $2 where id = $1 and referral_code is null \
             returning referral_code",
        )
        .bind(customer_id)
        .bind(&candidate)
        .fetch_optional(db)
        .await;

        match updated {
            Ok(Some(code)) => return Ok(code),
            // Someone else took this code, or this customer got one concurrently.
            Ok(None) | Err(sqlx::Error::Database(_)) => {
                if let Some(code) = sqlx::query_scalar::<_, Option<String>>(
                    "select referral_code from customers where id = $1",
                )
                .bind(customer_id)
                .fetch_optional(db)
                .await?
                .flatten()
                {
                    return Ok(code);
                }
                continue;
            }
            Err(e) => return Err(e),
        }
    }
    Err(sqlx::Error::Protocol(
        "could not allocate a unique referral code".into(),
    ))
}

/// Look up whose code this is. `None` for unknown or blank.
pub async fn owner_of(db: &PgPool, code: &str) -> Result<Option<Uuid>, sqlx::Error> {
    let code = code.trim().to_uppercase();
    if code.is_empty() {
        return Ok(None);
    }
    sqlx::query_scalar("select id from customers where referral_code = $1")
        .bind(code)
        .fetch_optional(db)
        .await
}

/// What a referral code is worth to this buyer, right now.
///
/// Returns 0 when referrals are off, the code is unknown, or the buyer is trying
/// to use their own — self-referral is the first thing anyone tries.
pub async fn discount_for(
    db: &PgPool,
    code: Option<&str>,
    buyer_id: Uuid,
) -> Result<(i64, Option<Uuid>), sqlx::Error> {
    let cfg = settings(db).await?;
    if !cfg.enabled || cfg.discount_cents == 0 {
        return Ok((0, None));
    }
    let Some(code) = code else {
        return Ok((0, None));
    };
    let Some(referrer) = owner_of(db, code).await? else {
        return Ok((0, None));
    };
    if referrer == buyer_id {
        return Ok((0, None));
    }

    // Already rewarded for introducing this person — the relationship pays once.
    // The buyer still gets nothing further, so a pair cannot ping-pong.
    let already: bool = sqlx::query_scalar(
        "select exists (select 1 from referrals where referrer_customer_id = $1 \
         and referred_customer_id = $2)",
    )
    .bind(referrer)
    .bind(buyer_id)
    .fetch_one(db)
    .await?;
    if already {
        return Ok((0, None));
    }

    Ok((cfg.discount_cents, Some(referrer)))
}

/// A coupon the customer can actually spend, by code.
pub struct RedeemableCoupon {
    pub id: Uuid,
    pub amount_cents: i64,
    pub customer_id: Uuid,
}

/// Find an active, unexpired coupon. Optionally require it to belong to a
/// particular customer — a coupon is personal, not a public promo code.
pub async fn find_coupon(
    db: &PgPool,
    code: &str,
    owner: Option<Uuid>,
) -> Result<Option<RedeemableCoupon>, sqlx::Error> {
    let code = code.trim().to_uppercase();
    if code.is_empty() {
        return Ok(None);
    }
    let row: Option<(Uuid, i64, Uuid)> = sqlx::query_as(
        "select id, amount_cents, customer_id from coupons \
         where code = $1 and status = 'active' \
           and (expires_at is null or expires_at > now())",
    )
    .bind(&code)
    .fetch_optional(db)
    .await?;

    Ok(row.and_then(|(id, amount_cents, customer_id)| {
        match owner {
            Some(expected) if expected != customer_id => None,
            _ => Some(RedeemableCoupon {
                id,
                amount_cents,
                customer_id,
            }),
        }
    }))
}

/// Record that a cart arrived through someone's link. The reward is not issued
/// here — see [`settle_referral`].
pub async fn attach_referral(
    conn: &mut sqlx::PgConnection,
    cart_id: Uuid,
    code: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("update carts set referral_code = $2 where id = $1")
        .bind(cart_id)
        .bind(code.trim().to_uppercase())
        .execute(conn)
        .await?;
    Ok(())
}

/// Issue the referrer's reward, now that the referred purchase has paid.
///
/// Idempotent: the unique constraint on (referrer, referred) means a second call
/// for the same pair does nothing, so re-applying a payment — which happens on
/// webhook redelivery and on "Check Square" — cannot mint a second coupon.
pub async fn settle_referral(
    conn: &mut sqlx::PgConnection,
    cart_id: Uuid,
    buyer_id: Uuid,
    code: &str,
) -> Result<Option<Uuid>, sqlx::Error> {
    let code = code.trim().to_uppercase();

    let cfg: (bool, i64, i32) = sqlx::query_as(
        "select referral_enabled, referral_reward_cents, coupon_expiry_days \
         from settings where id = true",
    )
    .fetch_one(&mut *conn)
    .await?;
    if !cfg.0 || cfg.1 == 0 {
        return Ok(None);
    }

    let referrer: Option<Uuid> =
        sqlx::query_scalar("select id from customers where referral_code = $1")
            .bind(&code)
            .fetch_optional(&mut *conn)
            .await?;
    let Some(referrer) = referrer else {
        return Ok(None);
    };
    if referrer == buyer_id {
        return Ok(None);
    }

    // Claim the pair first. If it is already taken this returns nothing and no
    // coupon is created — that is the idempotency.
    let claimed: Option<Uuid> = sqlx::query_scalar(
        "insert into referrals (referrer_customer_id, referred_customer_id, cart_id) \
         values ($1, $2, $3) on conflict do nothing returning id",
    )
    .bind(referrer)
    .bind(buyer_id)
    .bind(cart_id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(referral_id) = claimed else {
        return Ok(None);
    };

    let expires_at = if cfg.2 > 0 {
        Some(chrono::Utc::now() + chrono::Duration::days(cfg.2 as i64))
    } else {
        None
    };

    // Same retry-on-collision reasoning as referral codes.
    let mut coupon_id = None;
    for _ in 0..8 {
        let candidate = random_code(COUPON_CODE_LEN);
        let inserted: Option<Uuid> = sqlx::query_scalar(
            "insert into coupons (customer_id, code, amount_cents, source, expires_at, note) \
             values ($1, $2, $3, 'referral_reward', $4, $5) \
             on conflict (code) do nothing returning id",
        )
        .bind(referrer)
        .bind(&candidate)
        // Copied from settings now, so changing the reward later does not
        // re-value coupons already in someone's hands.
        .bind(cfg.1)
        .bind(expires_at)
        .bind("Thanks for sharing")
        .fetch_optional(&mut *conn)
        .await?;
        if inserted.is_some() {
            coupon_id = inserted;
            break;
        }
    }

    let Some(coupon_id) = coupon_id else {
        // The referral is recorded; the coupon is not. Better than failing the
        // whole payment settlement over a code collision.
        tracing::error!(%referral_id, "could not allocate a coupon code for a referral reward");
        return Ok(None);
    };

    sqlx::query("update referrals set reward_coupon_id = $2 where id = $1")
        .bind(referral_id)
        .bind(coupon_id)
        .execute(&mut *conn)
        .await?;

    tracing::info!(%referrer, %buyer_id, %coupon_id, "referral reward issued");
    Ok(Some(coupon_id))
}

/// Mark a coupon spent. Conditional on it still being active, so two carts
/// racing to redeem the same coupon cannot both succeed.
pub async fn redeem_coupon(
    conn: &mut sqlx::PgConnection,
    coupon_id: Uuid,
    cart_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let updated = sqlx::query(
        "update coupons set status = 'redeemed', redeemed_at = now(), redeemed_cart_id = $2 \
         where id = $1 and status = 'active'",
    )
    .bind(coupon_id)
    .bind(cart_id)
    .execute(conn)
    .await?
    .rows_affected();
    Ok(updated > 0)
}

/// Return a coupon to its owner when the cart it was on is cancelled.
pub async fn release_coupon(
    conn: &mut sqlx::PgConnection,
    cart_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "update coupons set status = 'active', redeemed_at = null, redeemed_cart_id = null \
         where redeemed_cart_id = $1 and status = 'redeemed'",
    )
    .bind(cart_id)
    .execute(conn)
    .await?;
    Ok(())
}

/// Apply a discount to a subtotal without ever going negative.
///
/// A $5 discount on a $3 roller must charge $0, not -$2: Square rejects a
/// negative order, and a customer being "owed" money by a checkout is not a
/// state this system should be able to reach.
pub fn apply_discount(subtotal_cents: i64, discount_cents: i64) -> (i64, i64) {
    let applied = discount_cents.clamp(0, subtotal_cents.max(0));
    (subtotal_cents - applied, applied)
}

/// Decimal form, for storing on an order row.
pub fn cents_to_decimal(cents: i64) -> Decimal {
    Decimal::new(cents, 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discount_never_makes_a_total_negative() {
        // Square rejects a negative order, and "the shop owes you $2" is not a
        // state a checkout should be able to produce.
        assert_eq!(apply_discount(300, 500), (0, 300));
        assert_eq!(apply_discount(0, 500), (0, 0));
    }

    #[test]
    fn discount_comes_off_the_total() {
        assert_eq!(apply_discount(6000, 500), (5500, 500));
        assert_eq!(apply_discount(500, 500), (0, 500));
    }

    #[test]
    fn a_zero_or_negative_discount_changes_nothing() {
        assert_eq!(apply_discount(6000, 0), (6000, 0));
        assert_eq!(apply_discount(6000, -100), (6000, 0));
    }

    #[test]
    fn codes_avoid_characters_people_confuse() {
        // These get read aloud and typed by hand.
        for _ in 0..200 {
            let code = random_code(8);
            assert_eq!(code.len(), 8);
            for bad in ['0', 'O', '1', 'I', 'L'] {
                assert!(!code.contains(bad), "{code} contains {bad}");
            }
            assert!(code.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()));
        }
    }

    #[test]
    fn codes_are_not_all_the_same() {
        let a: std::collections::HashSet<String> = (0..50).map(|_| random_code(6)).collect();
        assert!(a.len() > 45, "codes are not random enough: {} unique", a.len());
    }
}

//! Applying a settled Square payment to a local cart.
//!
//! Two paths reach this: the webhook (Square pushes) and the manual refresh
//! (we pull). Both must produce identical state, so the logic lives here once
//! rather than being written twice and drifting.
//!
//! Everything is idempotent. Square retries webhooks, an operator can mash
//! refresh, and both can race — so applying the same payment twice must be a
//! no-op, not a double-count.

use chrono::Utc;
use sqlx::PgPool;
use uuid::Uuid;

use crate::models::cart::{Cart, CartStatus};
use crate::models::order::OrderStatus;
use crate::square::RemotePayment;

/// What applying a payment did, for logging and for the caller's response.
#[derive(Debug, PartialEq, Eq)]
pub enum Applied {
    /// Cart moved to paid.
    Paid,
    /// Cart moved to refunded.
    Refunded,
    /// Payment seen but not terminal (still APPROVED/PENDING) — nothing changed.
    Pending,
    /// Already in this state; nothing to do.
    NoChange,
}

/// Apply a Square payment to the cart it belongs to.
///
/// The cart is located by `square_order_id`, which we set at checkout and Square
/// echoes on every payment for that order. Returns `None` when no local cart
/// matches — a legitimate case (a sale rung up directly in the Square POS with
/// no counterpart here), not an error.
pub async fn apply_payment(
    db: &PgPool,
    payment: &RemotePayment,
) -> Result<Option<(Uuid, Applied)>, sqlx::Error> {
    let Some(square_order_id) = payment.square_order_id.as_deref() else {
        return Ok(None);
    };

    let mut tx = db.begin().await?;

    // Lock the cart for the duration: a webhook and a refresh arriving together
    // would otherwise both read 'pending_payment' and both flip the orders.
    let cart = sqlx::query_as::<_, Cart>(
        "select * from carts where square_order_id = $1 for update",
    )
    .bind(square_order_id)
    .fetch_optional(&mut *tx)
    .await?;

    let Some(cart) = cart else {
        tx.rollback().await?;
        return Ok(None);
    };

    // A refund reverses a sale, so check it before completion — a payment that
    // was completed and then refunded carries both signals.
    let outcome = if payment.is_refunded() {
        if cart.status == CartStatus::Refunded {
            Applied::NoChange
        } else {
            Applied::Refunded
        }
    } else if payment.is_completed() {
        if cart.status == CartStatus::Paid {
            Applied::NoChange
        } else {
            Applied::Paid
        }
    } else {
        Applied::Pending
    };

    match outcome {
        Applied::Paid => {
            sqlx::query(
                r#"
                update carts set
                    status = 'paid',
                    paid_cents = $2,
                    square_payment_id = $3,
                    paid_at = coalesce(paid_at, $4),
                    updated_at = now()
                where id = $1
                "#,
            )
            .bind(cart.id)
            .bind(payment.amount_cents)
            .bind(&payment.payment_id)
            .bind(payment.created_at)
            .execute(&mut *tx)
            .await?;

            // The blends in this cart are now sold. Orders that were already
            // fulfilled stay fulfilled — 'paid' is not a step backwards.
            sqlx::query(
                r#"
                update orders set status = $2
                where id in (select order_id from cart_items where cart_id = $1 and order_id is not null)
                  and status = 'lead'
                "#,
            )
            .bind(cart.id)
            .bind(OrderStatus::Paid)
            .execute(&mut *tx)
            .await?;

            // Flag a short payment rather than silently accepting it. This is
            // the single most useful signal the integration produces: it means
            // the amount charged did not match the amount quoted.
            if payment.amount_cents != cart.total_cents {
                tracing::warn!(
                    cart_id = %cart.id,
                    expected_cents = cart.total_cents,
                    actual_cents = payment.amount_cents,
                    "cart paid for a different amount than it was quoted"
                );
            }
        }
        Applied::Refunded => {
            sqlx::query(
                r#"
                update carts set
                    status = 'refunded',
                    paid_cents = $2,
                    square_payment_id = $3,
                    updated_at = now()
                where id = $1
                "#,
            )
            .bind(cart.id)
            // Net of the refund, so a partial refund is visible as a partial amount.
            .bind(payment.amount_cents - payment.refunded_cents)
            .bind(&payment.payment_id)
            .execute(&mut *tx)
            .await?;

            // Orders go back to 'lead': the blend exists, but it is not sold.
            // Fulfilled orders are left alone — the bottle already went home
            // with someone, and that is a conversation, not a status change.
            sqlx::query(
                r#"
                update orders set status = 'lead'
                where id in (select order_id from cart_items where cart_id = $1 and order_id is not null)
                  and status = 'paid'
                "#,
            )
            .bind(cart.id)
            .execute(&mut *tx)
            .await?;

            tracing::warn!(cart_id = %cart.id, "cart refunded in Square");
        }
        Applied::Pending | Applied::NoChange => {
            // Still record which payment is attached, so the admin UI can link
            // out to it even before it settles.
            sqlx::query(
                "update carts set square_payment_id = coalesce(square_payment_id, $2), updated_at = now() where id = $1",
            )
            .bind(cart.id)
            .bind(&payment.payment_id)
            .execute(&mut *tx)
            .await?;
        }
    }

    tx.commit().await?;

    if outcome != Applied::NoChange {
        tracing::info!(
            cart_id = %cart.id,
            payment_id = %payment.payment_id,
            ?outcome,
            "applied square payment"
        );
    }

    Ok(Some((cart.id, outcome)))
}

/// Cancel a cart and release its hold on the orders it contained.
///
/// `cart_items.order_id` is uniquely indexed, so while a cart holds an order that
/// blend cannot be sold on any other cart — that is the double-billing guard. A
/// canceled cart must therefore let go, or an abandoned checkout would lock a
/// customer's blend out of ever being sold.
///
/// The release nulls `order_id` but keeps the line's `name` and price, so the
/// canceled cart still shows what was in it and for how much; only the claim is
/// dropped. Anything already paid is refused — money has moved, and unwinding
/// that is a refund in Square, not a local status edit.
pub async fn cancel_cart(
    db: &PgPool,
    square: &dyn crate::square::Square,
    cart_id: Uuid,
) -> Result<bool, sqlx::Error> {
    // Read the link id before the status change, so we still know what to void.
    let payment_link_id: Option<String> = sqlx::query_scalar(
        "select square_payment_link_id from carts where id = $1 and status in ('open', 'pending_payment')",
    )
    .bind(cart_id)
    .fetch_optional(db)
    .await?
    .flatten();

    let mut tx = db.begin().await?;

    let canceled = sqlx::query(
        r#"
        update carts set status = 'canceled', updated_at = now()
        where id = $1 and status in ('open', 'pending_payment')
        "#,
    )
    .bind(cart_id)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        > 0;

    if !canceled {
        tx.rollback().await?;
        return Ok(false);
    }

    // Remember what this cart held before releasing the links, so speculative
    // online orders can be cleaned up below.
    let released: Vec<Uuid> = sqlx::query_scalar(
        "select order_id from cart_items where cart_id = $1 and order_id is not null",
    )
    .bind(cart_id)
    .fetch_all(&mut *tx)
    .await?;

    sqlx::query("update cart_items set order_id = null where cart_id = $1")
        .bind(cart_id)
        .execute(&mut *tx)
        .await?;

    // An order raised by the public share page exists only because someone
    // clicked Buy; if they never paid, nothing was ever made and the row is
    // noise — worse, it would sit on that email's account as an order they
    // never placed. Orders taken at the bar are left alone: those blends
    // physically exist whether or not this particular cart was paid.
    if !released.is_empty() {
        sqlx::query(
            "delete from orders where id = any($1) and external_ref = 'public_share' \
             and status = 'lead'",
        )
        .bind(&released)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;

    // Void the link only after the local state is committed. Ordering matters: if
    // this call fails we have a dead link that still works, which reconciliation
    // will catch as an unrecorded payment. The reverse order could void a live
    // link and then fail to cancel, leaving a cart nobody can pay and nothing to
    // explain why.
    if let Some(link_id) = payment_link_id {
        if let Err(e) = square.void_checkout(&link_id).await {
            tracing::error!(
                %cart_id, %link_id,
                "cart canceled locally but its Square payment link could not be voided \
                 — it may still be payable: {e}"
            );
        }
    }

    Ok(true)
}

/// Expire payment links that were never used. Called by the background worker.
///
/// Without this, an abandoned checkout holds its orders indefinitely and the
/// reconciliation report accumulates permanent "awaiting payment" noise that
/// buries the discrepancies worth looking at.
pub async fn expire_stale_checkouts(
    db: &PgPool,
    square: &dyn crate::square::Square,
    older_than_hours: i64,
) -> Result<u64, sqlx::Error> {
    let cutoff = Utc::now() - chrono::Duration::hours(older_than_hours);

    let stale: Vec<Uuid> = sqlx::query_scalar(
        "select id from carts where status = 'pending_payment' and checkout_at < $1",
    )
    .bind(cutoff)
    .fetch_all(db)
    .await?;

    let mut n = 0;
    for cart_id in stale {
        // Reuse cancel_cart so expiry and manual cancellation cannot diverge — in
        // particular so expiry never forgets to release the orders or to void the
        // link, which is what would otherwise let a day-old link be paid against
        // blends that have since been sold on another cart.
        if cancel_cart(db, square, cart_id).await? {
            n += 1;
        }
    }

    if n > 0 {
        tracing::info!(count = n, "expired stale pending checkouts");
    }
    Ok(n)
}

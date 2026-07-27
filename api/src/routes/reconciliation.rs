//! Reconciliation: does what this app thinks it sold match what Square actually
//! collected?
//!
//! The two sides are joined on `carts.square_order_id`, which we set at checkout
//! and Square echoes on every payment for that order. Every cart and every Square
//! payment in the window lands in exactly one bucket:
//!
//! | Bucket              | Meaning                                          | Action |
//! |---------------------|--------------------------------------------------|--------|
//! | `matched`            | Both sides agree, to the cent                   | none |
//! | `amount_mismatch`    | Both sides have it, totals differ               | investigate — tip, discount, or an edit in the Square dashboard |
//! | `missing_in_square`  | We think it's paid, Square has no payment       | serious — usually a manual status edit here, or a payment outside the window |
//! | `unrecorded_payment` | Square was paid, our cart never moved off `pending_payment` | a lost webhook — press "Check Square" on the cart, then fix the subscription |
//! | `missing_locally`    | Square took money, no cart here at all          | usually a POS sale rung up outside this app; benign but should be explained |
//! | `awaiting_payment`   | Link issued, customer hasn't paid               | informational, not a discrepancy |
//!
//! `unrecorded_payment` is split out from `missing_locally` deliberately. Both are
//! "Square has money we didn't book", but they need opposite responses: a POS sale
//! is expected and needs no action here, while a lost webhook means this app's
//! records are wrong and one button fixes it. Lumping them together would bury the
//! actionable one under the routine one.
//!
//! Only `COMPLETED` Square payments count as revenue. Refunded carts are reported
//! net so the local total is comparable to Square's.
//!
//! ## Window boundaries
//!
//! A payment made at 23:59 can be recorded here a moment later, so filtering both
//! sides by timestamp alone would report the same sale as missing on one side and
//! orphaned on the other. Square's list is authoritative for the window; the local
//! side then pulls in any cart *either* paid in the window *or* referenced by a
//! payment Square returned. That makes the report stable at the edges.

use axum::extract::{Query, State};
use axum::Extension;
use axum::Json;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

use crate::employee_auth::{AdminEmployee, AuthedEmployee};
use crate::error::AppError;
use crate::models::cart::{Cart, CartStatus};
use crate::models::square_event::ReconciliationRun;
use crate::square::money;
use crate::square::RemotePayment;
use crate::AppState;

/// Refuse windows longer than this. Square is paginated and a year-wide request
/// would be slow and, past the page cap, wrong.
const MAX_WINDOW_DAYS: i64 = 92;

#[derive(Deserialize)]
pub struct ReconcileQuery {
    /// RFC3339. Defaults to 7 days ago.
    pub from: Option<DateTime<Utc>>,
    /// RFC3339. Defaults to now.
    pub to: Option<DateTime<Utc>>,
    /// Persist this run for the audit trail. Defaults to false so the admin
    /// screen can refresh freely without filling the table with noise.
    #[serde(default)]
    pub save: bool,
}

#[derive(Serialize)]
pub struct MatchedRow {
    pub cart_id: Uuid,
    pub square_order_id: String,
    pub square_payment_id: Option<String>,
    pub cents: i64,
    pub paid_at: Option<DateTime<Utc>>,
}

#[derive(Serialize)]
pub struct MismatchRow {
    pub cart_id: Uuid,
    pub square_order_id: String,
    pub square_payment_id: Option<String>,
    /// What the cart said it was charging.
    pub local_cents: i64,
    /// What Square actually collected.
    pub square_cents: i64,
    /// square - local. Positive means Square collected more (a tip, typically).
    pub difference_cents: i64,
    pub paid_at: Option<DateTime<Utc>>,
}

#[derive(Serialize)]
pub struct LocalOnlyRow {
    pub cart_id: Uuid,
    pub status: CartStatus,
    pub square_order_id: Option<String>,
    pub cents: i64,
    pub created_at: DateTime<Utc>,
    pub paid_at: Option<DateTime<Utc>>,
}

#[derive(Serialize)]
pub struct SquareOnlyRow {
    pub square_payment_id: String,
    pub square_order_id: Option<String>,
    pub cents: i64,
    pub currency: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Serialize)]
pub struct ReconciliationReport {
    pub period_start: DateTime<Utc>,
    pub period_end: DateTime<Utc>,
    pub backend: String,
    /// False when the mock produced these numbers — they are then a logic check,
    /// never a financial one.
    pub live: bool,

    /// Sum of local paid carts (net of refunds).
    pub local_total_cents: i64,
    /// Sum of completed Square payments in the window (net of refunds).
    pub square_total_cents: i64,
    /// square - local. Zero is the goal.
    pub difference_cents: i64,

    pub matched: Vec<MatchedRow>,
    pub amount_mismatch: Vec<MismatchRow>,
    pub missing_in_square: Vec<LocalOnlyRow>,
    /// Square collected, but our cart never left `pending_payment` — a lost
    /// webhook. Separate from `missing_locally` because it is actionable here
    /// ("Check Square" on the cart) while a POS sale is not.
    pub unrecorded_payment: Vec<MismatchRow>,
    pub missing_locally: Vec<SquareOnlyRow>,
    pub awaiting_payment: Vec<LocalOnlyRow>,

    /// True when every bucket that represents a discrepancy is empty.
    pub balanced: bool,
    /// One-line plain-English summary for the top of the admin screen.
    pub summary: String,
}

/// Net amount a cart represents locally: what Square reported if it reported
/// anything, else what we quoted. A refunded cart nets to its post-refund amount.
fn local_cents(cart: &Cart) -> i64 {
    cart.paid_cents.unwrap_or(cart.total_cents)
}

pub async fn reconcile(
    _admin: AdminEmployee,
    Extension(employee): Extension<AuthedEmployee>,
    State(state): State<AppState>,
    Query(q): Query<ReconcileQuery>,
) -> Result<Json<ReconciliationReport>, AppError> {
    let period_end = q.to.unwrap_or_else(Utc::now);
    let period_start = q.from.unwrap_or(period_end - Duration::days(7));

    if period_start >= period_end {
        return Err(AppError::BadRequest("`from` must be before `to`".into()));
    }
    if period_end - period_start > Duration::days(MAX_WINDOW_DAYS) {
        return Err(AppError::BadRequest(format!(
            "window is limited to {MAX_WINDOW_DAYS} days"
        )));
    }

    // --- Square's side ------------------------------------------------------
    let payments = state
        .square
        .list_payments(period_start, period_end)
        .await
        .map_err(|e| AppError::Unavailable(format!("could not list Square payments: {e}")))?;

    // Only completed money counts. Failed and canceled attempts are noise.
    let completed: Vec<&RemotePayment> = payments.iter().filter(|p| p.is_completed()).collect();

    let square_order_ids: Vec<String> = completed
        .iter()
        .filter_map(|p| p.square_order_id.clone())
        .collect();

    // --- Our side -----------------------------------------------------------
    // Paid/refunded in the window, plus anything Square named — see the module
    // note on window boundaries.
    let carts = sqlx::query_as::<_, Cart>(
        r#"
        select * from carts
        where (status in ('paid', 'refunded') and paid_at >= $1 and paid_at <= $2)
           or (square_order_id = any($3))
        order by paid_at nulls last, created_at
        "#,
    )
    .bind(period_start)
    .bind(period_end)
    .bind(&square_order_ids)
    .fetch_all(&state.db)
    .await?;

    // Carts with a live link that nobody has paid — reported separately so they
    // don't masquerade as missing revenue.
    let awaiting = sqlx::query_as::<_, Cart>(
        r#"
        select * from carts
        where status = 'pending_payment'
          and checkout_at >= $1 and checkout_at <= $2
        order by checkout_at
        "#,
    )
    .bind(period_start)
    .bind(period_end)
    .fetch_all(&state.db)
    .await?;

    // --- Bucket -------------------------------------------------------------
    let by_order: HashMap<&str, &RemotePayment> = completed
        .iter()
        .filter_map(|p| p.square_order_id.as_deref().map(|id| (id, *p)))
        .collect();

    let mut matched = Vec::new();
    let mut amount_mismatch = Vec::new();
    let mut missing_in_square = Vec::new();
    let mut unrecorded_payment = Vec::new();
    let mut seen_square_orders: HashSet<&str> = HashSet::new();

    let mut local_total: i64 = 0;

    for cart in &carts {
        // A cart still open or canceled has no revenue claim of its own — but if
        // Square collected against it, that is a lost webhook, not a POS sale, and
        // saying so is the difference between one button press and an afternoon in
        // the Square dashboard.
        if !matches!(cart.status, CartStatus::Paid | CartStatus::Refunded) {
            if let Some((order_id, p)) = cart
                .square_order_id
                .as_deref()
                .and_then(|id| by_order.get(id).map(|p| (id, *p)))
            {
                seen_square_orders.insert(order_id);
                let square_cents = p.amount_cents - p.refunded_cents;
                unrecorded_payment.push(MismatchRow {
                    cart_id: cart.id,
                    square_order_id: order_id.to_string(),
                    square_payment_id: Some(p.payment_id.clone()),
                    // Nothing was booked locally, hence zero.
                    local_cents: 0,
                    square_cents,
                    difference_cents: square_cents,
                    paid_at: Some(p.created_at),
                });
            }
            continue;
        }

        let cents = local_cents(cart);
        local_total += cents;

        let payment = cart
            .square_order_id
            .as_deref()
            .and_then(|id| by_order.get(id).copied());

        match (cart.square_order_id.as_deref(), payment) {
            (Some(order_id), Some(p)) => {
                seen_square_orders.insert(order_id);
                let square_cents = p.amount_cents - p.refunded_cents;
                if square_cents == cents {
                    matched.push(MatchedRow {
                        cart_id: cart.id,
                        square_order_id: order_id.to_string(),
                        square_payment_id: Some(p.payment_id.clone()),
                        cents,
                        paid_at: cart.paid_at,
                    });
                } else {
                    amount_mismatch.push(MismatchRow {
                        cart_id: cart.id,
                        square_order_id: order_id.to_string(),
                        square_payment_id: Some(p.payment_id.clone()),
                        local_cents: cents,
                        square_cents,
                        difference_cents: square_cents - cents,
                        paid_at: cart.paid_at,
                    });
                }
            }
            _ => {
                // Marked paid here, but Square has no completed payment for it in
                // this window. Either someone set the status by hand, or the
                // payment falls outside the window.
                missing_in_square.push(LocalOnlyRow {
                    cart_id: cart.id,
                    status: cart.status,
                    square_order_id: cart.square_order_id.clone(),
                    cents,
                    created_at: cart.created_at,
                    paid_at: cart.paid_at,
                });
            }
        }
    }

    let mut square_total: i64 = 0;
    let mut missing_locally = Vec::new();
    for p in &completed {
        let net = p.amount_cents - p.refunded_cents;
        square_total += net;

        let known = p
            .square_order_id
            .as_deref()
            .map(|id| seen_square_orders.contains(id))
            .unwrap_or(false);
        if !known {
            missing_locally.push(SquareOnlyRow {
                square_payment_id: p.payment_id.clone(),
                square_order_id: p.square_order_id.clone(),
                cents: net,
                currency: p.currency.clone(),
                created_at: p.created_at,
            });
        }
    }

    let awaiting_payment: Vec<LocalOnlyRow> = awaiting
        .iter()
        .map(|c| LocalOnlyRow {
            cart_id: c.id,
            status: c.status,
            square_order_id: c.square_order_id.clone(),
            cents: c.total_cents,
            created_at: c.created_at,
            paid_at: None,
        })
        .collect();

    let balanced = amount_mismatch.is_empty()
        && missing_in_square.is_empty()
        && unrecorded_payment.is_empty()
        && missing_locally.is_empty()
        && square_total == local_total;

    let difference = square_total - local_total;
    let summary = if balanced {
        format!(
            "Balanced — {} matched, {} on both sides.",
            matched.len(),
            money::format_cents(local_total, "USD")
        )
    } else {
        let mut parts = Vec::new();
        if !amount_mismatch.is_empty() {
            parts.push(format!("{} amount mismatch", amount_mismatch.len()));
        }
        if !missing_in_square.is_empty() {
            parts.push(format!("{} missing in Square", missing_in_square.len()));
        }
        if !unrecorded_payment.is_empty() {
            parts.push(format!("{} paid but unrecorded", unrecorded_payment.len()));
        }
        if !missing_locally.is_empty() {
            parts.push(format!("{} only in Square", missing_locally.len()));
        }
        format!(
            "{} — Square {} vs local {} (difference {}).",
            if parts.is_empty() {
                "Totals differ".to_string()
            } else {
                parts.join(", ")
            },
            money::format_cents(square_total, "USD"),
            money::format_cents(local_total, "USD"),
            money::format_cents(difference, "USD"),
        )
    };

    let report = ReconciliationReport {
        period_start,
        period_end,
        backend: state.square.name().to_string(),
        live: state.square.is_live(),
        local_total_cents: local_total,
        square_total_cents: square_total,
        difference_cents: difference,
        matched,
        amount_mismatch,
        missing_in_square,
        unrecorded_payment,
        missing_locally,
        awaiting_payment,
        balanced,
        summary,
    };

    if q.save {
        sqlx::query(
            r#"
            insert into reconciliation_runs
                (period_start, period_end, local_total_cents, square_total_cents,
                 matched_count, mismatched_count, missing_in_square_count,
                 missing_locally_count, report, run_by)
            values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            "#,
        )
        .bind(report.period_start)
        .bind(report.period_end)
        .bind(report.local_total_cents)
        .bind(report.square_total_cents)
        .bind(report.matched.len() as i32)
        // Unrecorded payments are amount mismatches of the starkest kind (booked
        // nothing, collected something), so they roll into the same headline
        // count. The full breakdown is preserved in the `report` blob.
        .bind((report.amount_mismatch.len() + report.unrecorded_payment.len()) as i32)
        .bind(report.missing_in_square.len() as i32)
        .bind(report.missing_locally.len() as i32)
        .bind(json!(&report))
        .bind(employee.id)
        .execute(&state.db)
        .await?;
    }

    Ok(Json(report))
}

/// Previously saved runs, newest first.
pub async fn history(
    _admin: AdminEmployee,
    State(state): State<AppState>,
) -> Result<Json<Vec<ReconciliationRun>>, AppError> {
    let runs = sqlx::query_as::<_, ReconciliationRun>(
        r#"
        select id, period_start, period_end, local_total_cents, square_total_cents,
               matched_count, mismatched_count, missing_in_square_count,
               missing_locally_count, run_by, created_at
        from reconciliation_runs
        order by created_at desc
        limit 50
        "#,
    )
    .fetch_all(&state.db)
    .await?;
    Ok(Json(runs))
}

/// Integration status for the admin panel: which backend, whether webhooks are
/// wired, and how much is currently sitting unpaid.
pub async fn status(
    _admin: AdminEmployee,
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, AppError> {
    let rows =
        sqlx::query_as::<_, (String, i64)>("select status, count(*) from carts group by status")
            .fetch_all(&state.db)
            .await?;

    let mut counts = serde_json::Map::new();
    for s in [
        "open",
        "pending_payment",
        "paid",
        "canceled",
        "refunded",
    ] {
        counts.insert(s.to_string(), json!(0));
    }
    for (status, count) in rows {
        counts.insert(status, json!(count));
    }

    // Postgres widens sum(bigint) to numeric, which will not decode into i64 —
    // hence the explicit cast back. The total is bounded by the cart table, so
    // the narrowing is safe.
    let pending_cents: i64 = sqlx::query_scalar(
        "select coalesce(sum(total_cents), 0)::bigint from carts where status = 'pending_payment'",
    )
    .fetch_one(&state.db)
    .await?;

    Ok(Json(json!({
        "backend": state.square.name(),
        "live": state.square.is_live(),
        "webhook_receiver_enabled": state.square_webhook_key.is_some()
            && state.square_webhook_url.is_some(),
        "cart_counts": counts,
        "pending_payment_cents": pending_cents,
    })))
}

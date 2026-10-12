//! Square integration status for the admin panel.
//!
//! This used to live in the reconciliation module, which was removed on
//! 2026-10-08 at the owner's request.

use axum::extract::State;
use axum::Json;
use serde_json::json;

use crate::employee_auth::AdminEmployee;
use crate::error::AppError;
use crate::AppState;

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

    let terminals_paired: i64 = sqlx::query_scalar(
        "select count(*) from square_terminals where status = 'paired'",
    )
    .fetch_one(&state.db)
    .await?;
    let default_terminal: Option<String> = sqlx::query_scalar(
        "select name from square_terminals where is_default and status = 'paired'",
    )
    .fetch_optional(&state.db)
    .await?;

    Ok(Json(json!({
        "backend": state.square.name(),
        "live": state.square.is_live(),
        "connection": state.square_connection.status(),
        "webhook_receiver_enabled": state.square_webhook_key.is_some()
            && state.square_webhook_url.is_some(),
        "cart_counts": counts,
        "pending_payment_cents": pending_cents,
        "terminals_paired": terminals_paired,
        "default_terminal": default_terminal,
    })))
}

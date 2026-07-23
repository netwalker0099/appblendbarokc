//! Inbound Squarespace webhook receiver + order reconciliation (Milestone 6).
//!
//! The receiver is public (Squarespace can't send our operator bearer token) but
//! every request is HMAC-verified against `SQUARESPACE_WEBHOOK_SECRET`. When that
//! secret is unset the endpoint is disabled (503) — it mutates order status, so
//! an unauthenticated open path is not acceptable. Verified notifications are
//! recorded for dedup/audit, then order.* topics are reconciled by fetching the
//! authoritative order state back from Squarespace and updating our matching row.

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use hmac::{Hmac, Mac};
use serde::Deserialize;
use serde_json::Value;
use sha2::Sha256;
use uuid::Uuid;

use crate::error::AppError;
use crate::models::order::OrderStatus;
use crate::models::webhook::WebhookEvent;
use crate::squarespace::RemoteOrder;
use crate::AppState;

type HmacSha256 = Hmac<Sha256>;

const SIGNATURE_HEADER: &str = "squarespace-signature";

#[derive(Deserialize)]
struct Notification {
    id: String,
    topic: String,
    #[serde(default)]
    data: NotificationData,
}

#[derive(Deserialize, Default)]
struct NotificationData {
    #[serde(rename = "orderId")]
    order_id: Option<String>,
}

/// HMAC-SHA256 of the raw body, compared constant-time against the hex header.
///
/// NOTE: the header name and encoding (hex vs base64) are a best-effort match to
/// Squarespace and are UNVERIFIED against the live service (no signing secret
/// exists yet). This function is internally consistent — the same routine signs
/// our self-tests — so the receiver/dedup/reconcile logic is fully exercised;
/// only the exact wire format must be confirmed when a real secret lands.
fn verify_signature(secret: &str, body: &[u8], provided_hex: &str) -> bool {
    let Ok(mut mac) = HmacSha256::new_from_slice(secret.as_bytes()) else {
        return false;
    };
    mac.update(body);
    let Ok(provided) = hex::decode(provided_hex.trim()) else {
        return false;
    };
    mac.verify_slice(&provided).is_ok()
}

fn map_status(remote: &RemoteOrder) -> OrderStatus {
    if remote.fulfillment_status.eq_ignore_ascii_case("FULFILLED") {
        OrderStatus::Fulfilled
    } else if remote.paid {
        OrderStatus::Paid
    } else {
        OrderStatus::Lead
    }
}

async fn settle(
    state: &AppState,
    notification_id: &str,
    status: &str,
    matched: Option<Uuid>,
    error: Option<&str>,
) -> Result<(), AppError> {
    sqlx::query(
        "update webhook_events set status = $2, matched_order_id = $3, error = $4, processed_at = now() where squarespace_notification_id = $1",
    )
    .bind(notification_id)
    .bind(status)
    .bind(matched)
    .bind(error)
    .execute(&state.db)
    .await?;
    Ok(())
}

pub async fn receive(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, AppError> {
    let Some(secret) = state.webhook_secret.as_deref() else {
        tracing::warn!("webhook received but SQUARESPACE_WEBHOOK_SECRET is unset — rejecting");
        return Err(AppError::Unavailable("webhook receiver not configured".into()));
    };

    let signature = headers
        .get(SIGNATURE_HEADER)
        .and_then(|v| v.to_str().ok())
        .ok_or(AppError::Unauthorized)?;
    if !verify_signature(secret, &body, signature) {
        return Err(AppError::Unauthorized);
    }

    let notification: Notification = serde_json::from_slice(&body)
        .map_err(|e| AppError::BadRequest(format!("invalid webhook body: {e}")))?;

    // Dedup: if we've already terminally handled this notification, just ack.
    // A prior 'received'/'failed' row is allowed through to (re)process.
    let existing: Option<String> = sqlx::query_scalar(
        "select status from webhook_events where squarespace_notification_id = $1",
    )
    .bind(&notification.id)
    .fetch_optional(&state.db)
    .await?;
    if matches!(
        existing.as_deref(),
        Some("processed" | "unmatched" | "ignored")
    ) {
        return Ok(StatusCode::OK);
    }

    // Record (or refresh) the audit row before doing any work.
    let payload: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    sqlx::query(
        r#"
        insert into webhook_events (squarespace_notification_id, topic, squarespace_order_id, payload)
        values ($1, $2, $3, $4)
        on conflict (squarespace_notification_id) do update set
            topic = excluded.topic,
            squarespace_order_id = excluded.squarespace_order_id,
            payload = excluded.payload,
            status = 'received',
            error = null,
            received_at = now(),
            processed_at = null
        "#,
    )
    .bind(&notification.id)
    .bind(&notification.topic)
    .bind(&notification.data.order_id)
    .bind(&payload)
    .execute(&state.db)
    .await?;

    // Only order.* topics reconcile; anything else is acknowledged and ignored.
    if !notification.topic.starts_with("order") {
        settle(&state, &notification.id, "ignored", None, None).await?;
        return Ok(StatusCode::OK);
    }

    let Some(order_id) = notification.data.order_id.as_deref() else {
        settle(
            &state,
            &notification.id,
            "failed",
            None,
            Some("order topic without an orderId"),
        )
        .await?;
        return Ok(StatusCode::OK); // malformed, but retrying won't help
    };

    // Fetch authoritative state (the mock today; the live API once keyed).
    let remote = match state.squarespace.get_order(order_id).await {
        Ok(remote) => remote,
        Err(err) => {
            settle(&state, &notification.id, "failed", None, Some(&err.to_string())).await?;
            if err.retryable() {
                // Transient — 500 so Squarespace redelivers and we try again.
                return Err(AppError::Internal(format!("get_order failed: {err}")));
            }
            return Ok(StatusCode::OK);
        }
    };

    let new_status = map_status(&remote);

    // Match to our order by the id we stored when we pushed it (Milestone 5).
    let matched: Option<Uuid> = sqlx::query_scalar(
        r#"
        update orders set
            status = $2,
            amount = coalesce($3, amount)
        where squarespace_order_id = $1
        returning id
        "#,
    )
    .bind(order_id)
    .bind(new_status)
    .bind(remote.grand_total)
    .fetch_optional(&state.db)
    .await?;

    match matched {
        Some(id) => {
            settle(&state, &notification.id, "processed", Some(id), None).await?;
            tracing::info!(order_id = %id, ?new_status, "reconciled order from webhook");
        }
        None => {
            // An order taken directly in the POS with no local counterpart — kept
            // for the record, not an error.
            settle(&state, &notification.id, "unmatched", None, None).await?;
            tracing::warn!(squarespace_order_id = %order_id, "webhook order has no local match");
        }
    }

    Ok(StatusCode::OK)
}

/// Recent webhook activity, for debugging reconciliation. Operator-authed.
pub async fn recent(State(state): State<AppState>) -> Result<Json<Vec<WebhookEvent>>, AppError> {
    let events = sqlx::query_as::<_, WebhookEvent>(
        r#"
        select id, squarespace_notification_id, topic, squarespace_order_id,
               status, matched_order_id, error, received_at, processed_at
        from webhook_events
        order by received_at desc
        limit 50
        "#,
    )
    .fetch_all(&state.db)
    .await?;
    Ok(Json(events))
}

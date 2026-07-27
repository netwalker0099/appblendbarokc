//! Inbound Square webhook receiver.
//!
//! This endpoint is public — Square cannot present an employee session cookie —
//! and it moves carts to `paid`. That combination means signature verification
//! is the entire security boundary, so when the signature key is unset the
//! endpoint refuses every request rather than trusting anything.
//!
//! ## Square's signature scheme
//!
//! Square sends `x-square-hmacsha256-signature`, which is:
//!
//! ```text
//! base64( HMAC-SHA256( key = signature_key, message = notification_url || raw_body ) )
//! ```
//!
//! The notification URL is concatenated *in front of* the body and must match
//! byte-for-byte the URL configured in the Square dashboard — scheme, host, path,
//! no trailing slash unless the dashboard has one. That is why
//! `SQUARE_WEBHOOK_URL` is configured explicitly rather than reconstructed from
//! request headers: `Host`/`X-Forwarded-Proto` are attacker-influenced, and
//! deriving the signed message from them would let a caller pick the string their
//! forged signature was computed over.
//!
//! Including the URL is what stops a signature captured from one endpoint being
//! replayed against another.

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2::Sha256;
use uuid::Uuid;

use crate::billing;
use crate::error::AppError;
use crate::models::square_event::SquareWebhookEvent;
use crate::AppState;

type HmacSha256 = Hmac<Sha256>;

const SIGNATURE_HEADER: &str = "x-square-hmacsha256-signature";

/// Verify a Square webhook signature.
///
/// Constant-time via `Mac::verify_slice`; a naive `==` on the base64 strings
/// would leak the expected signature a byte at a time under timing analysis.
pub fn verify_signature(
    signature_key: &str,
    notification_url: &str,
    body: &[u8],
    provided_b64: &str,
) -> bool {
    let Ok(mut mac) = HmacSha256::new_from_slice(signature_key.as_bytes()) else {
        return false;
    };
    mac.update(notification_url.as_bytes());
    mac.update(body);

    let Ok(provided) = BASE64.decode(provided_b64.trim()) else {
        return false;
    };
    mac.verify_slice(&provided).is_ok()
}

/// Pull the ids we care about out of a Square event envelope.
///
/// Square nests the changed object under `data.object`, keyed by type:
/// `{"type":"payment.updated","event_id":"…","data":{"type":"payment",
///   "id":"…","object":{"payment":{…}}}}`
struct EventIds {
    event_id: String,
    event_type: String,
    payment_id: Option<String>,
    order_id: Option<String>,
}

fn extract_ids(v: &Value) -> Option<EventIds> {
    let event_id = v.get("event_id").and_then(Value::as_str)?.to_string();
    let event_type = v.get("type").and_then(Value::as_str)?.to_string();

    let object = v.get("data").and_then(|d| d.get("object"));
    let payment = object.and_then(|o| o.get("payment"));
    let refund = object.and_then(|o| o.get("refund"));

    // A refund event carries the id of the payment it reverses, which is the
    // handle we actually need — the cart is found through the payment's order.
    let payment_id = payment
        .and_then(|p| p.get("id"))
        .or_else(|| refund.and_then(|r| r.get("payment_id")))
        .and_then(Value::as_str)
        .map(str::to_string);

    let order_id = payment
        .and_then(|p| p.get("order_id"))
        .or_else(|| object.and_then(|o| o.get("order")).and_then(|o| o.get("id")))
        .and_then(Value::as_str)
        .map(str::to_string);

    Some(EventIds {
        event_id,
        event_type,
        payment_id,
        order_id,
    })
}

async fn settle(
    state: &AppState,
    event_id: &str,
    status: &str,
    matched: Option<Uuid>,
    error: Option<&str>,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        update square_webhook_events set
            status = $2, matched_cart_id = $3, error = $4, processed_at = now()
        where square_event_id = $1
        "#,
    )
    .bind(event_id)
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
    let (Some(key), Some(url)) = (
        state.square_webhook_key.as_deref(),
        state.square_webhook_url.as_deref(),
    ) else {
        tracing::warn!(
            "square webhook received but SQUARE_WEBHOOK_SIGNATURE_KEY / \
             SQUARE_WEBHOOK_URL are not both set — rejecting"
        );
        return Err(AppError::Unavailable(
            "square webhook receiver not configured".into(),
        ));
    };

    let signature = headers
        .get(SIGNATURE_HEADER)
        .and_then(|v| v.to_str().ok())
        .ok_or(AppError::Unauthorized)?;

    if !verify_signature(key, url, &body, signature) {
        tracing::warn!("square webhook signature verification failed");
        return Err(AppError::Unauthorized);
    }

    let payload: Value = serde_json::from_slice(&body)
        .map_err(|e| AppError::BadRequest(format!("invalid webhook body: {e}")))?;

    let Some(ids) = extract_ids(&payload) else {
        return Err(AppError::BadRequest(
            "webhook body has no event_id/type".into(),
        ));
    };

    // Dedup. Square redelivers until it gets a 2xx, so an already-settled event
    // is acked without reprocessing. A prior 'received'/'failed' row is allowed
    // through to be retried.
    let existing: Option<String> =
        sqlx::query_scalar("select status from square_webhook_events where square_event_id = $1")
            .bind(&ids.event_id)
            .fetch_optional(&state.db)
            .await?;
    if matches!(
        existing.as_deref(),
        Some("processed" | "unmatched" | "ignored")
    ) {
        return Ok(StatusCode::OK);
    }

    // Record before doing any work, so a crash mid-processing still leaves a trail.
    sqlx::query(
        r#"
        insert into square_webhook_events
            (square_event_id, event_type, square_order_id, square_payment_id, payload)
        values ($1, $2, $3, $4, $5)
        on conflict (square_event_id) do update set
            event_type = excluded.event_type,
            square_order_id = excluded.square_order_id,
            square_payment_id = excluded.square_payment_id,
            payload = excluded.payload,
            status = 'received',
            error = null,
            received_at = now(),
            processed_at = null
        "#,
    )
    .bind(&ids.event_id)
    .bind(&ids.event_type)
    .bind(&ids.order_id)
    .bind(&ids.payment_id)
    .bind(&payload)
    .execute(&state.db)
    .await?;

    // Only payment and refund events move money. Everything else is acked and
    // filed — acking matters, or Square retries it forever.
    let relevant = ids.event_type.starts_with("payment.") || ids.event_type.starts_with("refund.");
    if !relevant {
        settle(&state, &ids.event_id, "ignored", None, None).await?;
        return Ok(StatusCode::OK);
    }

    let Some(payment_id) = ids.payment_id.as_deref() else {
        settle(
            &state,
            &ids.event_id,
            "failed",
            None,
            Some("payment/refund event without a payment id"),
        )
        .await?;
        // Malformed rather than transient — retrying will not help, so ack it.
        return Ok(StatusCode::OK);
    };

    // Re-fetch rather than trusting the payload. The signature proves the body
    // came from Square, but re-fetching also collapses out-of-order deliveries:
    // whatever order events arrive in, we apply Square's current truth.
    let payment = match state.square.get_payment(payment_id).await {
        Ok(p) => p,
        Err(err) => {
            settle(
                &state,
                &ids.event_id,
                "failed",
                None,
                Some(&err.to_string()),
            )
            .await?;
            if err.retryable() {
                // 500 so Square redelivers and we try again.
                return Err(AppError::Internal(format!("get_payment failed: {err}")));
            }
            return Ok(StatusCode::OK);
        }
    };

    match billing::apply_payment(&state.db, &payment).await? {
        Some((cart_id, outcome)) => {
            settle(&state, &ids.event_id, "processed", Some(cart_id), None).await?;
            tracing::info!(%cart_id, ?outcome, "square webhook reconciled a cart");
        }
        None => {
            // A sale rung up directly in the Square POS with no cart here. Kept
            // for the record and surfaced by reconciliation as "in Square only";
            // not an error.
            settle(&state, &ids.event_id, "unmatched", None, None).await?;
            tracing::info!(payment_id, "square payment has no local cart");
        }
    }

    Ok(StatusCode::OK)
}

/// Recent webhook activity, for debugging. Admin-only.
pub async fn recent(
    _admin: crate::employee_auth::AdminEmployee,
    State(state): State<AppState>,
) -> Result<Json<Vec<SquareWebhookEvent>>, AppError> {
    let events = sqlx::query_as::<_, SquareWebhookEvent>(
        r#"
        select id, square_event_id, event_type, square_order_id, square_payment_id,
               status, matched_cart_id, error, received_at, processed_at
        from square_webhook_events
        order by received_at desc
        limit 50
        "#,
    )
    .fetch_all(&state.db)
    .await?;
    Ok(Json(events))
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "test_signature_key";
    const URL: &str = "https://sandbox.theblendbarokc.com/api/webhooks/square";

    /// Sign the way Square does, so the tests exercise the real construction
    /// rather than whatever the verifier happens to do.
    fn sign(key: &str, url: &str, body: &[u8]) -> String {
        let mut mac = HmacSha256::new_from_slice(key.as_bytes()).unwrap();
        mac.update(url.as_bytes());
        mac.update(body);
        BASE64.encode(mac.finalize().into_bytes())
    }

    #[test]
    fn accepts_a_correctly_signed_body() {
        let body = br#"{"event_id":"evt_1","type":"payment.updated"}"#;
        let sig = sign(KEY, URL, body);
        assert!(verify_signature(KEY, URL, body, &sig));
    }

    #[test]
    fn rejects_a_tampered_body() {
        let body = br#"{"event_id":"evt_1","type":"payment.updated"}"#;
        let sig = sign(KEY, URL, body);
        let tampered = br#"{"event_id":"evt_1","type":"payment.deleted"}"#;
        assert!(!verify_signature(KEY, URL, tampered, &sig));
    }

    #[test]
    fn rejects_a_signature_for_a_different_url() {
        // The whole point of prefixing the URL: a signature captured against
        // another endpoint must not verify here.
        let body = br#"{"event_id":"evt_1"}"#;
        let sig = sign(KEY, "https://evil.example/api/webhooks/square", body);
        assert!(!verify_signature(KEY, URL, body, &sig));
    }

    #[test]
    fn rejects_a_signature_from_the_wrong_key() {
        let body = br#"{"event_id":"evt_1"}"#;
        let sig = sign("some_other_key", URL, body);
        assert!(!verify_signature(KEY, URL, body, &sig));
    }

    #[test]
    fn rejects_malformed_signatures() {
        let body = br#"{"event_id":"evt_1"}"#;
        assert!(!verify_signature(KEY, URL, body, "not base64 !!!"));
        assert!(!verify_signature(KEY, URL, body, ""));
        // Right encoding, wrong length — must not panic.
        assert!(!verify_signature(KEY, URL, body, &BASE64.encode([0u8; 8])));
    }

    #[test]
    fn tolerates_surrounding_whitespace() {
        let body = br#"{"event_id":"evt_1"}"#;
        let sig = sign(KEY, URL, body);
        assert!(verify_signature(KEY, URL, body, &format!("  {sig}\n")));
    }

    #[test]
    fn extracts_payment_event_ids() {
        let v = serde_json::json!({
            "event_id": "evt_1",
            "type": "payment.updated",
            "data": { "type": "payment", "id": "pay_1",
                      "object": { "payment": { "id": "pay_1", "order_id": "ord_1" } } }
        });
        let ids = extract_ids(&v).unwrap();
        assert_eq!(ids.event_id, "evt_1");
        assert_eq!(ids.event_type, "payment.updated");
        assert_eq!(ids.payment_id.as_deref(), Some("pay_1"));
        assert_eq!(ids.order_id.as_deref(), Some("ord_1"));
    }

    #[test]
    fn extracts_the_payment_id_from_a_refund_event() {
        let v = serde_json::json!({
            "event_id": "evt_2",
            "type": "refund.updated",
            "data": { "type": "refund",
                      "object": { "refund": { "id": "ref_1", "payment_id": "pay_9" } } }
        });
        let ids = extract_ids(&v).unwrap();
        assert_eq!(ids.payment_id.as_deref(), Some("pay_9"));
    }

    #[test]
    fn rejects_an_envelope_with_no_event_id() {
        assert!(extract_ids(&serde_json::json!({ "type": "payment.updated" })).is_none());
    }
}

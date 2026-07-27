//! Outbound chat notifications for customer-triggered events.
//!
//! Two events reach a channel, and only two:
//!
//! - **`sale.online`** — someone bought from a shared scent link. There was no
//!   staff member involved, and the blend does not exist yet.
//! - **`event.booked`** — a deposit settled. The published booking terms say
//!   "Without a deposit, your event is not booked", so that payment *is* the
//!   booking.
//!
//! Sales rung up at the bar deliberately do **not** notify. The staff member who
//! took the payment is standing right there; a message about it is noise, and a
//! channel full of noise is a channel nobody reads.
//!
//! Delivery runs on the background worker, never on the payment path — a Discord
//! outage must not be able to fail a customer's checkout.

pub mod format;

use std::time::Duration;

use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use crate::models::cart::Cart;
use crate::square::money;
use format::{EventKind, Message};

/// Give up after this many attempts.
const MAX_ATTEMPTS: i32 = 5;
const BATCH: i64 = 20;

/// Hosts allowed to receive a webhook, by platform.
///
/// An allowlist rather than a private-IP blocklist. This endpoint takes a URL
/// from an admin and has the server fetch it, which is the classic shape of a
/// server-side request forgery: a blocklist has to anticipate every way to spell
/// an internal address (decimal IPs, IPv6-mapped, DNS that resolves inward),
/// whereas an allowlist only has to name the four hosts that are ever correct.
const ALLOWED_HOSTS: &[(&str, &[&str])] = &[
    ("discord", &["discord.com", "discordapp.com"]),
    ("slack", &["hooks.slack.com"]),
    // Classic O365 connectors, plus Power Automate (Workflows) which is
    // replacing them.
    ("teams", &["office.com", "logic.azure.com"]),
];

/// Validate a webhook URL for the given platform.
///
/// Returns the reason it was rejected, phrased for an admin to act on.
pub fn validate_webhook_url(platform: &str, url: &str) -> Result<(), String> {
    let url = url.trim();

    let Some(rest) = url.strip_prefix("https://") else {
        return Err("webhook URL must start with https://".into());
    };
    // Credentials in the authority would let a URL point somewhere other than it
    // appears to (https://hooks.slack.com@evil.example/...).
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.contains('@') {
        return Err("webhook URL must not contain credentials".into());
    }
    let host = authority.split(':').next().unwrap_or("").to_ascii_lowercase();
    if host.is_empty() {
        return Err("webhook URL has no host".into());
    }

    let allowed = ALLOWED_HOSTS
        .iter()
        .find(|(p, _)| *p == platform)
        .map(|(_, hosts)| *hosts)
        .ok_or_else(|| format!("unknown platform '{platform}'"))?;

    let ok = allowed
        .iter()
        .any(|suffix| host == *suffix || host.ends_with(&format!(".{suffix}")));

    if ok {
        Ok(())
    } else {
        Err(format!(
            "a {platform} webhook URL must be on {} — got '{host}'",
            allowed.join(" or ")
        ))
    }
}

/// Queue notifications for a cart that has just been paid.
///
/// Runs inside the settlement transaction so a notification is never queued for
/// a payment that then rolls back. Insert conflicts are ignored: settlement is
/// idempotent and may run again (webhook redelivery, a staff member pressing
/// "Check Square"), and the customer should not be announced twice.
/// Takes the connection rather than a generic executor because it runs several
/// statements — a `&mut` handle isn't `Copy`, so it can't be reused across a
/// generic `PgExecutor` bound.
pub async fn enqueue_for_paid_cart(
    conn: &mut sqlx::PgConnection,
    cart: &Cart,
) -> Result<(), sqlx::Error> {
    // `created_by` is null exactly when the public share-page checkout built the
    // cart — that is the "no employee involved" test.
    let customer_initiated = cart.created_by.is_none();

    let has_deposit: bool = sqlx::query_scalar(
        "select exists (select 1 from cart_items where cart_id = $1 and kind = 'event_deposit')",
    )
    .bind(cart.id)
    .fetch_one(&mut *conn)
    .await?;

    let mut events: Vec<EventKind> = Vec::new();
    if customer_initiated {
        events.push(EventKind::OnlineSale);
    }
    // A deposit is worth announcing however the cart was built: staff raise the
    // invoice, but the event is not booked until the customer pays it, and that
    // moment is what the calendar needs to know about.
    if has_deposit {
        events.push(EventKind::EventBooked);
    }
    if events.is_empty() {
        return Ok(());
    }

    for event in events {
        let column = match event {
            EventKind::OnlineSale => "notify_online_sale",
            EventKind::EventBooked => "notify_event_booked",
        };
        sqlx::query(&format!(
            r#"
            insert into notification_deliveries (target_id, event_type, cart_id)
            select id, $1, $2 from notification_targets
            where active = true and {column} = true
            on conflict (target_id, cart_id, event_type) do nothing
            "#
        ))
        .bind(event.wire())
        .bind(cart.id)
        .execute(&mut *conn)
        .await?;
    }

    Ok(())
}

/// One row of work, joined with everything needed to send it.
#[derive(sqlx::FromRow)]
struct DueDelivery {
    id: Uuid,
    attempts: i32,
    event_type: String,
    cart_id: Uuid,
    platform: String,
    webhook_url: String,
    include_customer_email: bool,
    target_id: Uuid,
}

/// Drain due notification deliveries. Called from the background worker.
pub async fn drain(db: &PgPool, client: &reqwest::Client) -> Result<(), sqlx::Error> {
    let due = sqlx::query_as::<_, DueDelivery>(
        r#"
        select d.id, d.attempts, d.event_type, d.cart_id, d.target_id,
               t.platform, t.webhook_url, t.include_customer_email
        from notification_deliveries d
        join notification_targets t on t.id = d.target_id
        where d.status = 'pending' and d.next_attempt_at <= now()
        order by d.next_attempt_at
        limit $1
        "#,
    )
    .bind(BATCH)
    .fetch_all(db)
    .await?;

    for job in due {
        let outcome = deliver(db, client, &job).await;
        settle(db, &job, outcome).await?;
    }
    Ok(())
}

async fn deliver(
    db: &PgPool,
    client: &reqwest::Client,
    job: &DueDelivery,
) -> Result<(), (String, bool)> {
    let kind = EventKind::from_wire(&job.event_type)
        .ok_or_else(|| (format!("unknown event type '{}'", job.event_type), false))?;

    let message = build_message(db, job.cart_id, kind, job.include_customer_email)
        .await
        .map_err(|e| (e, false))?;

    let payload = format::render(&job.platform, &message).ok_or_else(|| {
        (
            format!("no payload shape for platform '{}'", job.platform),
            false,
        )
    })?;

    post(client, &job.webhook_url, &payload).await
}

/// Assemble the message from live data.
async fn build_message(
    db: &PgPool,
    cart_id: Uuid,
    kind: EventKind,
    include_email: bool,
) -> Result<Message, String> {
    let row: Option<(i64, String, Option<String>, String)> = sqlx::query_as(
        r#"
        select c.total_cents, c.currency, cu.name, cu.email
        from carts c join customers cu on cu.id = c.customer_id
        where c.id = $1
        "#,
    )
    .bind(cart_id)
    .fetch_optional(db)
    .await
    .map_err(|e| e.to_string())?;

    let (total_cents, currency, name, email) =
        row.ok_or_else(|| format!("cart {cart_id} no longer exists"))?;

    let items: Vec<(String, i32, i64)> = sqlx::query_as(
        "select name, quantity, unit_amount_cents from cart_items where cart_id = $1 order by created_at",
    )
    .bind(cart_id)
    .fetch_all(db)
    .await
    .map_err(|e| e.to_string())?;

    let lines = items
        .into_iter()
        .map(|(name, qty, cents)| {
            let money = money::format_cents(cents * qty as i64, &currency);
            if qty > 1 {
                format!("{name} ×{qty} — {money}")
            } else {
                format!("{name} — {money}")
            }
        })
        .collect();

    Ok(Message {
        kind,
        lines,
        total_cents,
        currency,
        customer_name: name,
        customer_email: if include_email { Some(email) } else { None },
        reference: format!("cart {}", &cart_id.simple().to_string()[..8]),
    })
}

/// POST the payload. `Err((reason, retryable))`.
async fn post(
    client: &reqwest::Client,
    url: &str,
    payload: &Value,
) -> Result<(), (String, bool)> {
    let resp = client
        .post(url)
        .json(payload)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| (format!("transport error: {e}"), true))?;

    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }

    let body = resp.text().await.unwrap_or_default();
    let body: String = body.chars().take(300).collect();
    // 5xx and 429 are worth another go; a 404 means the webhook was deleted in
    // the chat app and no amount of retrying will bring it back.
    let retryable = status.is_server_error() || status.as_u16() == 429;
    Err((format!("{status}: {body}"), retryable))
}

async fn settle(
    db: &PgPool,
    job: &DueDelivery,
    outcome: Result<(), (String, bool)>,
) -> Result<(), sqlx::Error> {
    match outcome {
        Ok(()) => {
            sqlx::query(
                "update notification_deliveries set status = 'sent', last_error = null, updated_at = now() where id = $1",
            )
            .bind(job.id)
            .execute(db)
            .await?;
            sqlx::query(
                "update notification_targets set last_success_at = now(), last_error = null, updated_at = now() where id = $1",
            )
            .bind(job.target_id)
            .execute(db)
            .await?;
        }
        Err((reason, retryable)) => {
            let attempts = job.attempts + 1;
            let give_up = !retryable || attempts >= MAX_ATTEMPTS;
            if give_up {
                sqlx::query(
                    "update notification_deliveries set status = 'failed', attempts = $2, last_error = $3, updated_at = now() where id = $1",
                )
                .bind(job.id)
                .bind(attempts)
                .bind(&reason)
                .execute(db)
                .await?;
                tracing::error!(delivery = %job.id, "notification failed permanently: {reason}");
            } else {
                let backoff = (2i64.pow(attempts as u32) * 15) as f64;
                sqlx::query(
                    "update notification_deliveries set attempts = $2, last_error = $3, next_attempt_at = now() + make_interval(secs => $4), updated_at = now() where id = $1",
                )
                .bind(job.id)
                .bind(attempts)
                .bind(&reason)
                .bind(backoff)
                .execute(db)
                .await?;
                tracing::warn!(delivery = %job.id, attempts, "notification failed, will retry: {reason}");
            }
            sqlx::query(
                "update notification_targets set last_error = $2, updated_at = now() where id = $1",
            )
            .bind(job.target_id)
            .bind(&reason)
            .execute(db)
            .await?;
        }
    }
    Ok(())
}

/// Send a sample message to a target, so an admin can prove the plumbing works
/// without waiting for a real customer.
pub async fn send_test(
    client: &reqwest::Client,
    platform: &str,
    webhook_url: &str,
) -> Result<(), String> {
    let message = Message {
        kind: EventKind::OnlineSale,
        lines: vec!["Test message — no order was placed".into()],
        total_cents: 0,
        currency: money::DEFAULT_CURRENCY.into(),
        customer_name: Some("The Blend Bar".into()),
        customer_email: None,
        reference: "connection test".into(),
    };
    let payload = format::render(platform, &message)
        .ok_or_else(|| format!("unknown platform '{platform}'"))?;
    post(client, webhook_url, &payload)
        .await
        .map_err(|(reason, _)| reason)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_the_real_webhook_hosts() {
        assert!(validate_webhook_url("discord", "https://discord.com/api/webhooks/1/abc").is_ok());
        assert!(validate_webhook_url("slack", "https://hooks.slack.com/services/T/B/x").is_ok());
        assert!(validate_webhook_url(
            "teams",
            "https://acme.webhook.office.com/webhookb2/abc"
        )
        .is_ok());
        assert!(validate_webhook_url(
            "teams",
            "https://prod-1.westus.logic.azure.com/workflows/abc"
        )
        .is_ok());
    }

    #[test]
    fn rejects_plaintext_http() {
        assert!(validate_webhook_url("discord", "http://discord.com/api/webhooks/1/a").is_err());
    }

    #[test]
    fn rejects_internal_targets() {
        // The SSRF cases: without the allowlist, an admin could point the server
        // at itself or at cloud metadata.
        for url in [
            "https://localhost/x",
            "https://127.0.0.1/x",
            "https://169.254.169.254/latest/meta-data/",
            "https://10.0.0.5/x",
            "https://[::1]/x",
        ] {
            assert!(
                validate_webhook_url("discord", url).is_err(),
                "should have rejected {url}"
            );
        }
    }

    #[test]
    fn rejects_lookalike_hosts() {
        for url in [
            "https://discord.com.evil.example/api/webhooks/1/a",
            "https://notdiscord.com/api/webhooks/1/a",
            "https://evil-discord.com/x",
        ] {
            assert!(
                validate_webhook_url("discord", url).is_err(),
                "should have rejected {url}"
            );
        }
        // A genuine subdomain is still fine.
        assert!(validate_webhook_url("discord", "https://ptb.discord.com/api/webhooks/1/a").is_ok());
    }

    #[test]
    fn rejects_userinfo_disguise() {
        // Reads as slack, resolves to evil.example.
        assert!(
            validate_webhook_url("slack", "https://hooks.slack.com@evil.example/x").is_err()
        );
    }

    #[test]
    fn platforms_cannot_be_crossed() {
        assert!(validate_webhook_url("slack", "https://discord.com/api/webhooks/1/a").is_err());
        assert!(validate_webhook_url("nope", "https://discord.com/api/webhooks/1/a").is_err());
    }

    #[test]
    fn tolerates_surrounding_whitespace_and_case() {
        assert!(
            validate_webhook_url("slack", "  https://HOOKS.SLACK.COM/services/T/B/x  ").is_ok()
        );
    }
}

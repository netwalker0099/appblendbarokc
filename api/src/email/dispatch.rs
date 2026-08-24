//! Deciding what to send, addressing it, and recording the outcome.
//!
//! Two paths, deliberately different:
//!
//! - **Sign-in links go out inline.** Someone is staring at a "check your email"
//!   screen, and the token expires in minutes. More importantly the token must
//!   not be persisted: it grants a customer session, and it is already stored
//!   hashed in `customer_login_tokens`. Queueing would mean writing a working
//!   credential to a second table in clear text.
//! - **Everything else is queued** in `email_deliveries` and drained by the
//!   background worker with retries, re-rendered from ids at send time so no
//!   body is ever stored.

use sqlx::PgPool;
use uuid::Uuid;

use super::{templates, MailError, Mailer, Outgoing};

/// Give up after this many attempts.
const MAX_ATTEMPTS: i32 = 5;
const BATCH: i64 = 20;

/// From-address settings, read per send so an admin change takes effect without
/// a restart.
pub struct Sender {
    pub from_address: String,
    pub from_name: String,
    pub reply_to: Option<String>,
}

/// Load the configured sender, or explain what is missing.
pub async fn load_sender(db: &PgPool) -> Result<Sender, MailError> {
    let row: Option<(Option<String>, String, Option<String>)> =
        sqlx::query_as("select from_address, from_name, reply_to from email_settings where id = true")
            .fetch_optional(db)
            .await
            .map_err(|e| MailError::Transport(e.to_string()))?;

    let (from_address, from_name, reply_to) = row.ok_or_else(|| {
        MailError::NotConfigured("email settings row is missing".into())
    })?;

    let from_address = from_address.filter(|s| !s.trim().is_empty()).ok_or_else(|| {
        MailError::NotConfigured(
            "no From address set — add one in Admin → Email. It must be a mailbox on \
             the Workspace domain, or the relay will refuse the message"
                .into(),
        )
    })?;

    Ok(Sender {
        from_address,
        from_name,
        reply_to: reply_to.filter(|s| !s.trim().is_empty()),
    })
}

async fn log_delivery(
    db: &PgPool,
    kind: &str,
    to: &str,
    subject: &str,
    status: &str,
    error: Option<&str>,
    customer_id: Option<Uuid>,
) {
    let result = sqlx::query(
        r#"
        insert into email_deliveries
            (kind, to_address, subject, status, attempts, last_error, customer_id, sent_at)
        values ($1, $2, $3, $4, 1, $5, $6, case when $4 = 'sent' then now() else null end)
        "#,
    )
    .bind(kind)
    .bind(to)
    .bind(subject)
    .bind(status)
    .bind(error)
    .bind(customer_id)
    .execute(db)
    .await;

    if let Err(e) = result {
        // Never let bookkeeping failure mask the send result.
        tracing::error!("could not record email delivery: {e}");
    }
}

/// Send a portal sign-in link. Inline — see the module note.
pub async fn send_magic_link(
    db: &PgPool,
    mailer: &dyn Mailer,
    customer_id: Uuid,
    to: &str,
    link: &str,
    ttl_minutes: i64,
) -> Result<(), MailError> {
    let sender = match load_sender(db).await {
        Ok(s) => s,
        Err(e) => {
            log_delivery(
                db,
                "magic_link",
                to,
                "Your Blend Bar sign-in link",
                "failed",
                Some(&e.to_string()),
                Some(customer_id),
            )
            .await;
            return Err(e);
        }
    };

    let body = templates::magic_link(link, ttl_minutes);
    let subject = body.subject.clone();

    let result = mailer
        .send(Outgoing {
            to: to.to_string(),
            from_address: sender.from_address,
            from_name: sender.from_name,
            reply_to: sender.reply_to,
            body,
            attachments: Vec::new(),
        })
        .await;

    match &result {
        Ok(()) => log_delivery(db, "magic_link", to, &subject, "sent", None, Some(customer_id)).await,
        Err(e) => {
            log_delivery(
                db,
                "magic_link",
                to,
                &subject,
                "failed",
                Some(&e.to_string()),
                Some(customer_id),
            )
            .await
        }
    }
    result
}

/// Queue a "your blend is ready" for an order.
///
/// Returns false when the message was not queued — because it is switched off,
/// because the order has already been told, or because there is no address.
pub async fn queue_order_ready(db: &PgPool, order_id: Uuid) -> Result<bool, sqlx::Error> {
    let enabled: bool =
        sqlx::query_scalar("select order_ready_enabled from email_settings where id = true")
            .fetch_optional(db)
            .await?
            .unwrap_or(false);
    if !enabled {
        return Ok(false);
    }

    let row: Option<(Uuid, String)> = sqlx::query_as(
        "select c.id, c.email from orders o join customers c on c.id = o.customer_id where o.id = $1",
    )
    .bind(order_id)
    .fetch_optional(db)
    .await?;

    let Some((customer_id, email)) = row else {
        return Ok(false);
    };

    // The partial unique index on (order_id) where kind='order_ready' is what
    // actually stops a customer being told twice if staff press the button again.
    let inserted = sqlx::query(
        r#"
        insert into email_deliveries (kind, to_address, subject, customer_id, order_id)
        values ('order_ready', $1, $2, $3, $4)
        on conflict do nothing
        "#,
    )
    .bind(&email)
    .bind("Your blend is ready to collect")
    .bind(customer_id)
    .bind(order_id)
    .execute(db)
    .await?
    .rows_affected();

    Ok(inserted > 0)
}

/// Queue a test message to an arbitrary address, from the admin panel.
pub async fn send_test(
    db: &PgPool,
    mailer: &dyn Mailer,
    to: &str,
    site: &str,
) -> Result<(), MailError> {
    let sender = load_sender(db).await?;
    let body = templates::test_message(site);
    let subject = body.subject.clone();

    let result = mailer
        .send(Outgoing {
            to: to.to_string(),
            from_address: sender.from_address,
            from_name: sender.from_name,
            reply_to: sender.reply_to,
            body,
            attachments: Vec::new(),
        })
        .await;

    match &result {
        Ok(()) => log_delivery(db, "test", to, &subject, "sent", None, None).await,
        Err(e) => log_delivery(db, "test", to, &subject, "failed", Some(&e.to_string()), None).await,
    }
    result
}

#[derive(sqlx::FromRow)]
struct DueEmail {
    id: Uuid,
    attempts: i32,
    kind: String,
    to_address: String,
    order_id: Option<Uuid>,
}

/// Drain queued email. Called from the background worker.
pub async fn drain(db: &PgPool, mailer: &dyn Mailer, site: &str) -> Result<(), sqlx::Error> {
    let due = sqlx::query_as::<_, DueEmail>(
        r#"
        select id, attempts, kind, to_address, order_id
        from email_deliveries
        where status = 'pending' and next_attempt_at <= now()
        order by next_attempt_at
        limit $1
        "#,
    )
    .bind(BATCH)
    .fetch_all(db)
    .await?;

    for job in due {
        let outcome = deliver(db, mailer, &job, site).await;
        settle(db, &job, outcome).await?;
    }
    Ok(())
}

async fn deliver(
    db: &PgPool,
    mailer: &dyn Mailer,
    job: &DueEmail,
    site: &str,
) -> Result<(), MailError> {
    let sender = load_sender(db).await?;

    let body = match job.kind.as_str() {
        "order_ready" => {
            let order_id = job
                .order_id
                .ok_or_else(|| MailError::Rejected("order_ready with no order".into()))?;

            // Re-rendered from live data rather than stored: nothing about the
            // message body is persisted.
            let row: Option<(Option<String>, String, String, Option<String>)> = sqlx::query_as(
                r#"
                select c.name, o.type::text, o.size::text, s.name
                from orders o
                join customers c on c.id = o.customer_id
                left join scents s on s.id = o.scent_id
                where o.id = $1
                "#,
            )
            .bind(order_id)
            .fetch_optional(db)
            .await
            .map_err(|e| MailError::Transport(e.to_string()))?;

            let (customer_name, order_type, size, scent_name) = row.ok_or_else(|| {
                MailError::Rejected(format!("order {order_id} no longer exists"))
            })?;

            let what = describe(&order_type, &size, scent_name.as_deref());
            templates::order_ready(customer_name.as_deref(), &what, &format!("{site}/portal"))
        }
        other => return Err(MailError::Rejected(format!("unknown email kind '{other}'"))),
    };

    mailer
        .send(Outgoing {
            to: job.to_address.clone(),
            from_address: sender.from_address,
            from_name: sender.from_name,
            reply_to: sender.reply_to,
            body,
            attachments: Vec::new(),
        })
        .await
}

/// Human description of an order, for the customer's benefit.
fn describe(order_type: &str, size: &str, scent_name: Option<&str>) -> String {
    let size_label = match size {
        "oz3_4" => "3.4 oz",
        "oz1_7" => "1.7 oz",
        "roller" => "Roller",
        "spray" => "Spray, 10 ml",
        other => other,
    };
    let what = match (order_type, scent_name) {
        ("set_perfume", Some(name)) => name.to_string(),
        ("set_perfume", None) => "Your perfume".to_string(),
        _ => "Your custom blend".to_string(),
    };
    format!("{what} ({size_label})")
}

async fn settle(db: &PgPool, job: &DueEmail, outcome: Result<(), MailError>) -> Result<(), sqlx::Error> {
    match outcome {
        Ok(()) => {
            sqlx::query(
                "update email_deliveries set status = 'sent', last_error = null, \
                 sent_at = now(), updated_at = now() where id = $1",
            )
            .bind(job.id)
            .execute(db)
            .await?;
            tracing::info!(to = %job.to_address, kind = %job.kind, "email sent");
        }
        Err(err) => {
            let attempts = job.attempts + 1;
            let give_up = !err.retryable() || attempts >= MAX_ATTEMPTS;
            let reason = err.to_string();
            if give_up {
                sqlx::query(
                    "update email_deliveries set status = 'failed', attempts = $2, \
                     last_error = $3, updated_at = now() where id = $1",
                )
                .bind(job.id)
                .bind(attempts)
                .bind(&reason)
                .execute(db)
                .await?;
                tracing::error!(to = %job.to_address, "email failed permanently: {reason}");
            } else {
                let backoff = (2i64.pow(attempts as u32) * 30) as f64;
                sqlx::query(
                    "update email_deliveries set attempts = $2, last_error = $3, \
                     next_attempt_at = now() + make_interval(secs => $4), updated_at = now() \
                     where id = $1",
                )
                .bind(job.id)
                .bind(attempts)
                .bind(&reason)
                .bind(backoff)
                .execute(db)
                .await?;
                tracing::warn!(to = %job.to_address, attempts, "email failed, will retry: {reason}");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describes_an_order_the_way_a_customer_would_read_it() {
        assert_eq!(
            describe("set_perfume", "oz3_4", Some("Golden Hour")),
            "Golden Hour (3.4 oz)"
        );
        assert_eq!(
            describe("custom_mix", "spray", None),
            "Your custom blend (Spray, 10 ml)"
        );
        // A set perfume whose scent has since been deleted must still read
        // sensibly rather than showing a blank or an id.
        assert_eq!(describe("set_perfume", "roller", None), "Your perfume (Roller)");
    }
}

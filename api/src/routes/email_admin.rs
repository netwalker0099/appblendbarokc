//! Admin configuration for outbound email.
//!
//! The split follows the same rule as the Square and chat integrations: the
//! **credentials live in the environment** and are never settable or readable
//! from a browser, while the things a person legitimately changes — who the mail
//! comes from, whether the optional messages are on — live in the database.
//!
//! Relay host and password are therefore absent from every response here. What
//! the panel gets instead is whether a relay is configured at all, which is
//! enough to tell someone why nothing is arriving.

use axum::extract::State;
use axum::Json;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::employee_auth::AdminEmployee;
use crate::error::AppError;
use crate::AppState;

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct EmailSettings {
    pub from_address: Option<String>,
    pub from_name: String,
    pub reply_to: Option<String>,
    pub order_ready_enabled: bool,
}

const COLS: &str = "from_address, from_name, reply_to, order_ready_enabled";

/// Settings plus enough runtime state to diagnose "why is nothing sending".
pub async fn get(
    _admin: AdminEmployee,
    State(state): State<AppState>,
) -> Result<Json<Value>, AppError> {
    let settings =
        sqlx::query_as::<_, EmailSettings>(&format!("select {COLS} from email_settings where id = true"))
            .fetch_one(&state.db)
            .await?;

    let counts = sqlx::query_as::<_, (String, i64)>(
        "select status, count(*) from email_deliveries group by status",
    )
    .fetch_all(&state.db)
    .await?;

    let mut by_status = serde_json::Map::new();
    for s in ["pending", "sent", "failed"] {
        by_status.insert(s.to_string(), json!(0));
    }
    for (status, n) in counts {
        by_status.insert(status, json!(n));
    }

    // Which Google identity is in play, if any. The private key is never read
    // back out — only the service-account address, which is not a secret.
    let google_account = crate::email::credentials::load_stored()
        .and_then(|r| r.ok())
        .map(|a| a.client_email);

    let impersonate: Option<String> =
        sqlx::query_scalar("select google_impersonate from email_settings where id = true")
            .fetch_optional(&state.db)
            .await?
            .flatten();

    Ok(Json(json!({
        "settings": settings,
        // Whether a transport is wired up at all. Hosts, keys and passwords are
        // deliberately never included.
        "transport": state.mailer().name(),
        "live": state.mailer().is_live(),
        "counts": by_status,
        "google": {
            // True when the key comes from the server environment, in which case
            // the browser must not pretend it can change it.
            "env_managed": crate::email::credentials::env_managed(),
            "connected": google_account.is_some() || crate::email::credentials::env_managed(),
            "service_account": google_account,
            "impersonate": impersonate,
        },
    })))
}

#[derive(Deserialize)]
pub struct UpdateEmailSettings {
    pub from_address: Option<String>,
    pub from_name: Option<String>,
    pub reply_to: Option<String>,
    pub order_ready_enabled: Option<bool>,
}

/// Cheap sanity check. Real validation is the relay accepting the message, which
/// is what the test button is for.
fn looks_like_an_address(value: &str) -> bool {
    let v = value.trim();
    let mut parts = v.splitn(2, '@');
    let local = parts.next().unwrap_or("");
    let domain = parts.next().unwrap_or("");
    !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !v.contains(' ')
}

pub async fn update(
    _admin: AdminEmployee,
    State(state): State<AppState>,
    Json(body): Json<UpdateEmailSettings>,
) -> Result<Json<EmailSettings>, AppError> {
    if let Some(from) = &body.from_address {
        if !from.trim().is_empty() && !looks_like_an_address(from) {
            return Err(AppError::BadRequest(
                "the From address doesn't look like an email address".into(),
            ));
        }
    }
    if let Some(reply) = &body.reply_to {
        if !reply.trim().is_empty() && !looks_like_an_address(reply) {
            return Err(AppError::BadRequest(
                "the Reply-to address doesn't look like an email address".into(),
            ));
        }
    }
    if let Some(name) = &body.from_name {
        if name.trim().is_empty() {
            return Err(AppError::BadRequest("the sender name can't be blank".into()));
        }
    }

    let settings = sqlx::query_as::<_, EmailSettings>(&format!(
        r#"
        update email_settings set
            from_address = coalesce($1, from_address),
            from_name = coalesce($2, from_name),
            reply_to = coalesce($3, reply_to),
            order_ready_enabled = coalesce($4, order_ready_enabled),
            updated_at = now()
        where id = true
        returning {COLS}
        "#
    ))
    .bind(body.from_address.as_deref().map(str::trim))
    .bind(body.from_name.as_deref().map(str::trim))
    .bind(body.reply_to.as_deref().map(str::trim))
    .bind(body.order_ready_enabled)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(settings))
}

#[derive(Deserialize)]
pub struct ConnectGoogle {
    /// The downloaded service-account JSON, pasted whole.
    pub service_account_json: String,
    /// The Workspace mailbox to send as.
    pub impersonate: String,
}

/// Store a Google service-account key and start using it immediately.
///
/// The key is written to a file on a mounted volume, **not** to a database
/// column — `GET /api/admin/backup` hands an admin a full pg_dump, and a
/// credential in a table would ride along in every copy of it. It is never read
/// back out to a browser; the panel only ever sees the service-account address.
pub async fn connect_google(
    _admin: AdminEmployee,
    State(state): State<AppState>,
    Json(body): Json<ConnectGoogle>,
) -> Result<Json<Value>, AppError> {
    if crate::email::credentials::env_managed() {
        return Err(AppError::Conflict(
            "a Google key is configured in the server environment, which takes \
             precedence. Remove GOOGLE_SA_KEY_FILE / GOOGLE_SA_KEY_JSON from .env \
             to manage it from here instead."
                .into(),
        ));
    }

    let impersonate = body.impersonate.trim().to_lowercase();
    if !looks_like_an_address(&impersonate) {
        return Err(AppError::BadRequest(
            "enter the Workspace mailbox to send as, e.g. hello@theblendbarokc.com".into(),
        ));
    }

    // Validated and written before anything is switched over, so a bad paste
    // leaves the previous working setup untouched.
    let account = crate::email::credentials::store(&body.service_account_json)
        .map_err(|e| AppError::BadRequest(e.to_string()))?;

    sqlx::query("update email_settings set google_impersonate = $1, updated_at = now() where id = true")
        .bind(&impersonate)
        .execute(&state.db)
        .await?;

    // Swap the live mailer so this takes effect without a restart.
    state.set_mailer(crate::email::build(Some(impersonate.clone())));

    let live = state.mailer().is_live();
    tracing::info!(
        service_account = %account.client_email,
        sending_as = %impersonate,
        "google credentials stored via admin"
    );

    Ok(Json(json!({
        "ok": live,
        "service_account": account.client_email,
        "impersonate": impersonate,
        "detail": if live {
            "Connected. Send a test to confirm the delegation is authorised."
        } else {
            "Saved, but the mailer did not come up — check the server logs."
        },
    })))
}

/// Forget the stored key and fall back to whatever else is configured.
pub async fn disconnect_google(
    _admin: AdminEmployee,
    State(state): State<AppState>,
) -> Result<Json<Value>, AppError> {
    let removed = crate::email::credentials::remove_stored()
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let impersonate: Option<String> =
        sqlx::query_scalar("select google_impersonate from email_settings where id = true")
            .fetch_optional(&state.db)
            .await?
            .flatten();

    state.set_mailer(crate::email::build(impersonate));

    Ok(Json(json!({
        "removed": removed,
        "live": state.mailer().is_live(),
    })))
}

#[derive(Deserialize)]
pub struct TestRequest {
    pub to: String,
}

/// Send a test message, so email can be proved before a customer depends on it.
pub async fn test(
    _admin: AdminEmployee,
    State(state): State<AppState>,
    Json(body): Json<TestRequest>,
) -> Result<Json<Value>, AppError> {
    let to = body.to.trim();
    if !looks_like_an_address(to) {
        return Err(AppError::BadRequest("enter a valid address to test with".into()));
    }

    let site = std::env::var("CUSTOMER_SITE_URL")
        .unwrap_or_else(|_| "https://sandbox.theblendbarokc.com".to_string());

    match crate::email::dispatch::send_test(&state.db, state.mailer().as_ref(), to, &site).await {
        Ok(()) if state.mailer().is_live() => Ok(Json(json!({
            "ok": true,
            "detail": format!("Test message sent to {to}."),
        }))),
        // The mock reports success because it did what it does; saying "sent"
        // would be a lie to whoever is watching for it to arrive.
        Ok(()) => Ok(Json(json!({
            "ok": false,
            "detail": "No SMTP relay is configured, so nothing was actually sent — \
                       the message was written to the server log instead.",
        }))),
        Err(e) => Ok(Json(json!({ "ok": false, "detail": e.to_string() }))),
    }
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct EmailDelivery {
    pub id: Uuid,
    pub kind: String,
    pub to_address: String,
    pub subject: String,
    pub status: String,
    pub attempts: i32,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub sent_at: Option<DateTime<Utc>>,
}

/// Recent deliveries, for answering "did they get it?".
pub async fn recent(
    _admin: AdminEmployee,
    State(state): State<AppState>,
) -> Result<Json<Vec<EmailDelivery>>, AppError> {
    let rows = sqlx::query_as::<_, EmailDelivery>(
        r#"
        select id, kind, to_address, subject, status, attempts, last_error, created_at, sent_at
        from email_deliveries
        order by created_at desc
        limit 50
        "#,
    )
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_ordinary_addresses() {
        for a in [
            "hello@theblendbarokc.com",
            "no-reply@mail.theblendbarokc.com",
            "a.b+tag@example.co.uk",
        ] {
            assert!(looks_like_an_address(a), "rejected {a}");
        }
    }

    #[test]
    fn rejects_things_that_are_not_addresses() {
        for a in ["", "nodomain", "@example.com", "a@b", "a@.com", "a@b.", "two words@x.com"] {
            assert!(!looks_like_an_address(a), "accepted {a}");
        }
    }
}

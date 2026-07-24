//! Customer portal auth: magic-link login tokens + customer sessions. Reuses the
//! generic token/cookie helpers from `employee_auth`, but with its own cookie
//! (scoped to the customer site) and no password/MFA — customers prove ownership
//! of their email via the emailed link.

use axum::http::{header, HeaderMap};
use sqlx::PgPool;

use crate::employee_auth::hash_token;
use crate::models::customer::Customer;

pub const CUSTOMER_COOKIE: &str = "bb_customer";
pub const SESSION_TTL_DAYS: i64 = 30;
pub const LOGIN_TTL_MINUTES: i64 = 15;

pub fn set_cookie(token: &str) -> String {
    format!(
        "{CUSTOMER_COOKIE}={token}; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age={}",
        SESSION_TTL_DAYS * 24 * 3600
    )
}

pub fn clear_cookie() -> String {
    format!("{CUSTOMER_COOKIE}=; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age=0")
}

pub fn read_cookie(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    let prefix = format!("{CUSTOMER_COOKIE}=");
    raw.split(';')
        .map(str::trim)
        .find_map(|kv| kv.strip_prefix(&prefix))
        .map(str::to_string)
}

/// The logged-in customer, from the session cookie, if unexpired.
pub async fn load_customer(db: &PgPool, headers: &HeaderMap) -> Option<Customer> {
    let token = read_cookie(headers)?;
    let token_hash = hash_token(&token);
    sqlx::query_as::<_, Customer>(
        "select c.* from customers c \
         join customer_sessions s on s.customer_id = c.id \
         where s.token_hash = $1 and s.expires_at > now()",
    )
    .bind(&token_hash)
    .fetch_optional(db)
    .await
    .ok()?
}

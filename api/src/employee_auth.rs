//! Employee auth primitives: argon2 passwords, TOTP (RFC 6238), server-side
//! sessions, and the httpOnly session cookie. Route handlers live in
//! `routes::session`; middleware guards live here.

use std::sync::OnceLock;

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use axum::extract::{FromRequestParts, Request, State};
use axum::http::request::Parts;
use axum::http::{header, HeaderMap, HeaderValue};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{DateTime, Utc};
use rand::{Rng, RngCore};
use serde_json::Value;
use sqlx::PgPool;
use totp_rs::{Algorithm, Secret, TOTP};
use uuid::Uuid;

use crate::error::AppError;
use crate::models::employee::{Employee, EmployeeRole, EmployeeSession};
use crate::AppState;

pub const SESSION_COOKIE: &str = "bb_session";
pub const SESSION_TTL_HOURS: i64 = 12;
/// Cookie holding an MFA trust grant — see `mfa_trust_hours` in settings and the
/// header comment on migration 0022. Separate from the session cookie because it
/// outlives the session and carries no authority of its own.
pub const MFA_TRUST_COOKIE: &str = "bb_mfa_trust";
const TOTP_ISSUER: &str = "The Blend Bar";

// --- Passwords (argon2id) ---

pub fn hash_password(password: &str) -> Result<String, argon2::password_hash::Error> {
    let salt = SaltString::generate(&mut OsRng);
    Ok(Argon2::default()
        .hash_password(password.as_bytes(), &salt)?
        .to_string())
}

pub fn verify_password(password: &str, hash: &str) -> bool {
    match PasswordHash::new(hash) {
        Ok(parsed) => Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok(),
        Err(_) => false,
    }
}

/// A stable dummy hash so a login for a non-existent email still spends argon2
/// time — closes the "does this email exist?" timing oracle.
pub fn dummy_hash() -> &'static str {
    static DUMMY: OnceLock<String> = OnceLock::new();
    DUMMY.get_or_init(|| hash_password("blendbar-dummy-password").expect("hash dummy"))
}

pub fn generate_temp_password() -> String {
    // Unambiguous alphabet (no O/0/I/l/1) for a password read aloud/typed once.
    const CHARS: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnpqrstuvwxyz23456789";
    let mut rng = rand::thread_rng();
    (0..16).map(|_| CHARS[rng.gen_range(0..CHARS.len())] as char).collect()
}

// --- TOTP ---

pub fn generate_totp_secret() -> String {
    match Secret::generate_secret().to_encoded() {
        Secret::Encoded(s) => s,
        Secret::Raw(_) => unreachable!("to_encoded always yields an Encoded secret"),
    }
}

fn totp_for(secret_b32: &str, email: &str) -> Result<TOTP, String> {
    let bytes = Secret::Encoded(secret_b32.to_string())
        .to_bytes()
        .map_err(|e| format!("bad totp secret: {e}"))?;
    TOTP::new(
        Algorithm::SHA1,
        6,
        1, // ±1 step (±30s) tolerance
        30,
        bytes,
        Some(TOTP_ISSUER.to_string()),
        email.to_string(),
    )
    .map_err(|e| e.to_string())
}

/// otpauth:// provisioning URI + a PNG-data-URI QR code for enrollment.
pub fn totp_provisioning(secret_b32: &str, email: &str) -> Result<(String, String), String> {
    let totp = totp_for(secret_b32, email)?;
    let uri = totp.get_url();
    let qr = totp.get_qr_base64().map_err(|e| e.to_string())?;
    Ok((uri, format!("data:image/png;base64,{qr}")))
}

pub fn verify_totp(secret_b32: &str, email: &str, code: &str) -> bool {
    match totp_for(secret_b32, email) {
        Ok(totp) => totp.check_current(code.trim()).unwrap_or(false),
        Err(_) => false,
    }
}

// --- Sessions + cookie ---

pub fn generate_session_token() -> String {
    let mut b = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut b);
    hex::encode(b)
}

pub fn hash_token(token: &str) -> String {
    crate::auth::hash_token(token)
}

pub fn session_expiry() -> DateTime<Utc> {
    Utc::now() + chrono::Duration::hours(SESSION_TTL_HOURS)
}

pub fn set_cookie(token: &str) -> String {
    format!(
        "{SESSION_COOKIE}={token}; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age={}",
        SESSION_TTL_HOURS * 3600
    )
}

pub fn clear_cookie() -> String {
    format!("{SESSION_COOKIE}=; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age=0")
}

pub fn read_cookie(headers: &HeaderMap) -> Option<String> {
    read_named_cookie(headers, SESSION_COOKIE)
}

fn read_named_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    let prefix = format!("{name}=");
    raw.split(';')
        .map(str::trim)
        .find_map(|kv| kv.strip_prefix(&prefix))
        .map(str::to_string)
}

/// Attach `body` as JSON plus a `Set-Cookie` header.
pub fn json_with_cookie(cookie: String, body: Value) -> Response {
    json_with_cookies(vec![cookie], body)
}

/// Attach `body` as JSON plus one `Set-Cookie` header per entry. Appends rather
/// than inserts, so the session cookie and the MFA-trust cookie can both be set
/// on one response instead of silently overwriting each other.
pub fn json_with_cookies(cookies: Vec<String>, body: Value) -> Response {
    let mut res = Json(body).into_response();
    for cookie in cookies {
        if let Ok(v) = HeaderValue::from_str(&cookie) {
            res.headers_mut().append(header::SET_COOKIE, v);
        }
    }
    res
}

// --- MFA trust window ---
//
// Once MFA has been verified on a device, a later sign-in from that same device
// may skip the code for `settings.mfa_trust_hours`. The password is always still
// required: these helpers are only ever reached after argon2 has passed, so the
// cookie by itself authenticates nobody.

/// The configured trust window, in hours. 0 means "always ask for MFA".
///
/// Read fresh on every sign-in rather than cached, because the point of the
/// admin toggle is that turning it off stops the next sign-in, not the next
/// deploy. A failed read is treated as 0 — if we cannot establish that skipping
/// MFA is permitted, we ask for it.
pub async fn trust_window_hours(db: &PgPool) -> i64 {
    sqlx::query_scalar::<_, i32>("select mfa_trust_hours from settings where id = true")
        .fetch_one(db)
        .await
        .unwrap_or(0) as i64
}

pub fn set_trust_cookie(token: &str, hours: i64) -> String {
    format!(
        "{MFA_TRUST_COOKIE}={token}; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age={}",
        hours * 3600
    )
}

pub fn clear_trust_cookie() -> String {
    format!("{MFA_TRUST_COOKIE}=; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age=0")
}

pub fn read_trust_cookie(headers: &HeaderMap) -> Option<String> {
    read_named_cookie(headers, MFA_TRUST_COOKIE)
}

/// Is this device still inside its MFA trust window **for this employee**?
///
/// Bound to `employee_id` on purpose: the stand tablet is shared, and a grant
/// issued to one person must not wave the next person past their own second
/// factor. The window is checked against the *current* setting as well as the
/// row's own expiry, so shortening it (or setting it to 0) immediately shortens
/// grants that are already out there.
pub async fn device_trusted(db: &PgPool, headers: &HeaderMap, employee_id: Uuid) -> bool {
    let hours = trust_window_hours(db).await;
    if hours == 0 {
        return false;
    }
    let Some(token) = read_trust_cookie(headers) else {
        return false;
    };
    sqlx::query_scalar::<_, i64>(
        "select count(*) from employee_mfa_trust \
         where token_hash = $1 and employee_id = $2 and expires_at > now() \
           and created_at > now() - make_interval(hours => $3)",
    )
    .bind(hash_token(&token))
    .bind(employee_id)
    .bind(hours as i32)
    .fetch_one(db)
    .await
    .map(|n| n > 0)
    .unwrap_or(false)
}

/// Issue a fresh trust grant for this device and return the cookie to set.
/// `None` when the window is off, in which case no cookie is sent at all.
pub async fn grant_trust(
    db: &PgPool,
    employee_id: Uuid,
    user_agent: Option<&str>,
) -> Result<Option<String>, sqlx::Error> {
    let hours = trust_window_hours(db).await;
    if hours == 0 {
        return Ok(None);
    }
    let token = generate_session_token();
    sqlx::query(
        "insert into employee_mfa_trust (token_hash, employee_id, expires_at, user_agent) \
         values ($1, $2, now() + make_interval(hours => $3), $4)",
    )
    .bind(hash_token(&token))
    .bind(employee_id)
    .bind(hours as i32)
    .bind(user_agent)
    .execute(db)
    .await?;
    // Opportunistic tidy-up. The table is tiny and only this path writes to it,
    // so it needs no scheduled job of its own.
    let _ = sqlx::query("delete from employee_mfa_trust where expires_at <= now()")
        .execute(db)
        .await;
    Ok(Some(set_trust_cookie(&token, hours)))
}

/// Drop every trust grant for an employee, optionally sparing the one presented
/// by the current request. Called wherever the answer to "is this still the same
/// trusted device?" has been invalidated: password reset or change, MFA reset,
/// deactivation.
pub async fn revoke_trust(
    db: &PgPool,
    employee_id: Uuid,
    keep_token: Option<&str>,
) -> Result<(), sqlx::Error> {
    match keep_token {
        Some(token) => {
            sqlx::query(
                "delete from employee_mfa_trust where employee_id = $1 and token_hash <> $2",
            )
            .bind(employee_id)
            .bind(hash_token(token))
            .execute(db)
            .await?;
        }
        None => {
            sqlx::query("delete from employee_mfa_trust where employee_id = $1")
                .bind(employee_id)
                .execute(db)
                .await?;
        }
    }
    Ok(())
}

/// Load a (session, employee) pair from the cookie, if the session is unexpired
/// and the employee active. Does not check `mfa_pending`.
pub async fn load_session(db: &PgPool, headers: &HeaderMap) -> Option<(EmployeeSession, Employee)> {
    let token = read_cookie(headers)?;
    let token_hash = hash_token(&token);
    let session = sqlx::query_as::<_, EmployeeSession>(
        "select * from employee_sessions where token_hash = $1 and expires_at > now()",
    )
    .bind(&token_hash)
    .fetch_optional(db)
    .await
    .ok()??;
    let employee = sqlx::query_as::<_, Employee>(
        "select * from employees where id = $1 and active = true",
    )
    .bind(session.employee_id)
    .fetch_optional(db)
    .await
    .ok()??;
    Some((session, employee))
}

/// The authenticated employee, injected into requests by [`require_employee`].
#[derive(Debug, Clone)]
pub struct AuthedEmployee {
    pub id: Uuid,
    pub email: String,
    pub role: EmployeeRole,
}

/// Middleware: require a fully-authenticated (MFA-complete) employee session.
pub async fn require_employee(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Result<Response, AppError> {
    let (session, employee) = load_session(&state.db, req.headers())
        .await
        .ok_or(AppError::Unauthorized)?;
    if session.mfa_pending {
        return Err(AppError::Unauthorized);
    }
    req.extensions_mut().insert(AuthedEmployee {
        id: employee.id,
        email: employee.email,
        role: employee.role,
    });
    Ok(next.run(req).await)
}

/// Extractor that requires the request's employee to be an admin. Add it as a
/// handler parameter (`_admin: AdminEmployee`) to gate admin-only routes; relies
/// on [`require_employee`] having injected the `AuthedEmployee` extension.
pub struct AdminEmployee(#[allow(dead_code)] pub AuthedEmployee);

#[async_trait::async_trait]
impl<S> FromRequestParts<S> for AdminEmployee
where
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let employee = parts
            .extensions
            .get::<AuthedEmployee>()
            .cloned()
            .ok_or(AppError::Unauthorized)?;
        if employee.role != EmployeeRole::Admin {
            return Err(AppError::Forbidden);
        }
        Ok(AdminEmployee(employee))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers_with_cookie(value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(header::COOKIE, HeaderValue::from_str(value).unwrap());
        h
    }

    /// Both cookies ride on the same `Cookie:` header once a device is trusted,
    /// so each reader has to find its own. Getting this wrong would hand the
    /// session token to the trust check (and never match), quietly making the
    /// window do nothing.
    #[test]
    fn reads_each_cookie_from_a_shared_header() {
        let h = headers_with_cookie("bb_session=sess-tok; bb_mfa_trust=trust-tok");
        assert_eq!(read_cookie(&h).as_deref(), Some("sess-tok"));
        assert_eq!(read_trust_cookie(&h).as_deref(), Some("trust-tok"));

        // Order must not matter, and neither should a missing space.
        let h = headers_with_cookie("bb_mfa_trust=trust-tok;bb_session=sess-tok");
        assert_eq!(read_cookie(&h).as_deref(), Some("sess-tok"));
        assert_eq!(read_trust_cookie(&h).as_deref(), Some("trust-tok"));
    }

    #[test]
    fn missing_trust_cookie_reads_as_none() {
        let h = headers_with_cookie("bb_session=sess-tok");
        assert_eq!(read_trust_cookie(&h), None);
        assert_eq!(read_trust_cookie(&HeaderMap::new()), None);
    }

    #[test]
    fn trust_cookie_is_httponly_and_expires_with_the_window() {
        let c = set_trust_cookie("tok", 4);
        assert!(c.starts_with("bb_mfa_trust=tok;"));
        assert!(c.contains("HttpOnly"));
        assert!(c.contains("Secure"));
        assert!(c.contains("Max-Age=14400"));

        assert!(clear_trust_cookie().contains("Max-Age=0"));
    }

    /// Two cookies on one response must both survive. `insert` would drop the
    /// first, so a trusted device would never be remembered.
    #[test]
    fn both_cookies_survive_on_one_response() {
        let res = json_with_cookies(
            vec![set_cookie("sess"), set_trust_cookie("trust", 4)],
            serde_json::json!({ "status": "ok" }),
        );
        let set: Vec<_> = res
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .map(|v| v.to_str().unwrap().to_string())
            .collect();
        assert_eq!(set.len(), 2);
        assert!(set.iter().any(|c| c.starts_with("bb_session=sess;")));
        assert!(set.iter().any(|c| c.starts_with("bb_mfa_trust=trust;")));
    }
}

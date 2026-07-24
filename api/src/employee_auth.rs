//! Employee auth primitives: argon2 passwords, TOTP (RFC 6238), server-side
//! sessions, and the httpOnly session cookie. Route handlers live in
//! `routes::session`; middleware guards live here.

use std::sync::OnceLock;

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use axum::extract::{Request, State};
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
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    let prefix = format!("{SESSION_COOKIE}=");
    raw.split(';')
        .map(str::trim)
        .find_map(|kv| kv.strip_prefix(&prefix))
        .map(str::to_string)
}

/// Attach `body` as JSON plus a `Set-Cookie` header.
pub fn json_with_cookie(cookie: String, body: Value) -> Response {
    let mut res = Json(body).into_response();
    if let Ok(v) = HeaderValue::from_str(&cookie) {
        res.headers_mut().insert(header::SET_COOKIE, v);
    }
    res
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

/// Middleware: require an admin session. Applied on top of `require_employee`,
/// so the `AuthedEmployee` extension is already present.
pub async fn require_admin(req: Request, next: Next) -> Result<Response, AppError> {
    let is_admin = req
        .extensions()
        .get::<AuthedEmployee>()
        .map(|e| e.role == EmployeeRole::Admin)
        .unwrap_or(false);
    if !is_admin {
        return Err(AppError::Forbidden);
    }
    Ok(next.run(req).await)
}

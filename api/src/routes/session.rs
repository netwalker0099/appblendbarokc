//! Employee auth flow: login → (MFA enroll) → MFA verify → session; plus logout
//! and `/me`. Sessions are cookie-based and self-managed, so these live on the
//! open router (they can't require an existing full session).

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::employee_auth as ea;
use crate::error::AppError;
use crate::models::employee::Employee;
use crate::AppState;

#[derive(Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

/// Verify the password and open a *pending* session (cookie set, but only the MFA
/// endpoints accept it). Returns whether the employee must enroll MFA or verify.
pub async fn login(
    State(state): State<AppState>,
    Json(body): Json<LoginRequest>,
) -> Result<Response, AppError> {
    let email = body.email.trim().to_lowercase();
    let employee = sqlx::query_as::<_, Employee>(
        "select * from employees where email = $1 and active = true",
    )
    .bind(&email)
    .fetch_optional(&state.db)
    .await?;

    // Always spend argon2 time (dummy hash when the email doesn't exist) so login
    // timing doesn't reveal whether an account exists.
    let ok = match &employee {
        Some(e) => ea::verify_password(&body.password, &e.password_hash),
        None => {
            ea::verify_password(&body.password, ea::dummy_hash());
            false
        }
    };
    let Some(employee) = employee.filter(|_| ok) else {
        return Err(AppError::Unauthorized);
    };

    let token = ea::generate_session_token();
    sqlx::query(
        "insert into employee_sessions (token_hash, employee_id, mfa_pending, expires_at) values ($1, $2, true, $3)",
    )
    .bind(ea::hash_token(&token))
    .bind(employee.id)
    .bind(ea::session_expiry())
    .execute(&state.db)
    .await?;

    let status = if employee.mfa_enrolled {
        "mfa_required"
    } else {
        "enroll_required"
    };
    Ok(ea::json_with_cookie(
        ea::set_cookie(&token),
        json!({ "status": status, "email": employee.email, "role": employee.role }),
    ))
}

/// Begin MFA enrollment: generate a TOTP secret (stored as a candidate) and return
/// the provisioning URI + QR. Requires a pending session for a not-yet-enrolled
/// employee.
pub async fn mfa_enroll(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, AppError> {
    let (session, employee) = ea::load_session(&state.db, &headers)
        .await
        .ok_or(AppError::Unauthorized)?;
    if !session.mfa_pending {
        return Err(AppError::BadRequest("already authenticated".into()));
    }
    if employee.mfa_enrolled {
        return Err(AppError::BadRequest("MFA already set up — verify instead".into()));
    }

    let secret = ea::generate_totp_secret();
    let (uri, qr) = ea::totp_provisioning(&secret, &employee.email)
        .map_err(|e| AppError::Internal(format!("totp provisioning failed: {e}")))?;

    sqlx::query("update employees set totp_secret = $1 where id = $2")
        .bind(&secret)
        .bind(employee.id)
        .execute(&state.db)
        .await?;

    Ok(Json(json!({ "otpauth_uri": uri, "qr": qr, "secret": secret })))
}

#[derive(Deserialize)]
pub struct MfaVerifyRequest {
    pub code: String,
}

/// Verify a TOTP code and upgrade the pending session to full. Handles both
/// first-time enrollment confirmation and normal login MFA.
pub async fn mfa_verify(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<MfaVerifyRequest>,
) -> Result<Json<Value>, AppError> {
    let (session, employee) = ea::load_session(&state.db, &headers)
        .await
        .ok_or(AppError::Unauthorized)?;
    if !session.mfa_pending {
        return Ok(Json(json!({ "status": "ok", "email": employee.email, "role": employee.role })));
    }
    let secret = employee
        .totp_secret
        .as_deref()
        .ok_or_else(|| AppError::BadRequest("start MFA enrollment first".into()))?;

    if !ea::verify_totp(secret, &employee.email, &body.code) {
        return Err(AppError::Unauthorized);
    }

    let mut tx = state.db.begin().await?;
    if !employee.mfa_enrolled {
        sqlx::query("update employees set mfa_enrolled = true where id = $1")
            .bind(employee.id)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query("update employees set last_login_at = now() where id = $1")
        .bind(employee.id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("update employee_sessions set mfa_pending = false where id = $1")
        .bind(session.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    Ok(Json(json!({ "status": "ok", "email": employee.email, "role": employee.role })))
}

pub async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    if let Some(token) = ea::read_cookie(&headers) {
        sqlx::query("delete from employee_sessions where token_hash = $1")
            .bind(ea::hash_token(&token))
            .execute(&state.db)
            .await?;
    }
    Ok(ea::json_with_cookie(ea::clear_cookie(), json!({ "status": "ok" })))
}

/// Who am I — the frontend calls this on load to route by auth state + role.
pub async fn me(State(state): State<AppState>, headers: HeaderMap) -> Result<Json<Value>, AppError> {
    let (session, employee) = ea::load_session(&state.db, &headers)
        .await
        .ok_or(AppError::Unauthorized)?;
    if session.mfa_pending {
        return Err(AppError::Unauthorized);
    }
    Ok(Json(json!({
        "email": employee.email,
        "role": employee.role,
        "mfa_enrolled": employee.mfa_enrolled,
    })))
}

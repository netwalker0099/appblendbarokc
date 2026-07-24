//! Admin user management: list/create employees, change role, activate/deactivate,
//! reset password, reset MFA. All admin-only (via the `AdminEmployee` extractor).
//! Self-service password change lives in `routes::session`.

use axum::extract::{Path, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::employee_auth::{self as ea, AdminEmployee};
use crate::error::AppError;
use crate::models::employee::{EmployeeRole, EmployeeView};
use crate::AppState;

const VIEW_COLS: &str =
    "id, email, role, mfa_enrolled, active, created_at, last_login_at";

pub async fn list(
    _admin: AdminEmployee,
    State(state): State<AppState>,
) -> Result<Json<Vec<EmployeeView>>, AppError> {
    let rows = sqlx::query_as::<_, EmployeeView>(&format!(
        "select {VIEW_COLS} from employees order by created_at"
    ))
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

#[derive(Deserialize)]
pub struct CreateEmployeeRequest {
    pub email: String,
    pub role: EmployeeRole,
}

pub async fn create(
    _admin: AdminEmployee,
    State(state): State<AppState>,
    Json(body): Json<CreateEmployeeRequest>,
) -> Result<Json<Value>, AppError> {
    let email = body.email.trim().to_lowercase();
    if !email.contains('@') {
        return Err(AppError::BadRequest("a valid email is required".into()));
    }
    let temp = ea::generate_temp_password();
    let hash = ea::hash_password(&temp)
        .map_err(|e| AppError::Internal(format!("password hash failed: {e}")))?;

    let view = sqlx::query_as::<_, EmployeeView>(&format!(
        "insert into employees (email, password_hash, role) values ($1, $2, $3) returning {VIEW_COLS}"
    ))
    .bind(&email)
    .bind(&hash)
    .bind(body.role)
    .fetch_one(&state.db)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(db) if db.is_unique_violation() => {
            AppError::Conflict("an account with this email already exists".into())
        }
        _ => AppError::from(e),
    })?;

    // The new hire signs in with this once, then sets up MFA and changes it.
    Ok(Json(json!({ "employee": view, "temp_password": temp })))
}

#[derive(Deserialize)]
pub struct UpdateEmployeeRequest {
    pub role: Option<EmployeeRole>,
    pub active: Option<bool>,
}

pub async fn update(
    _admin: AdminEmployee,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateEmployeeRequest>,
) -> Result<Json<EmployeeView>, AppError> {
    // Apply then check inside a transaction so we can't leave zero active admins
    // (locking everyone out of admin) — rolls back if the change would.
    let mut tx = state.db.begin().await?;
    let view = sqlx::query_as::<_, EmployeeView>(&format!(
        "update employees set role = coalesce($2, role), active = coalesce($3, active) where id = $1 returning {VIEW_COLS}"
    ))
    .bind(id)
    .bind(body.role)
    .bind(body.active)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::NotFound("employee not found".into()))?;

    let active_admins: i64 =
        sqlx::query_scalar("select count(*) from employees where role = 'admin' and active = true")
            .fetch_one(&mut *tx)
            .await?;
    if active_admins == 0 {
        // tx drops here without commit -> rollback
        return Err(AppError::BadRequest(
            "can't remove the last active admin".into(),
        ));
    }
    tx.commit().await?;
    Ok(Json(view))
}

pub async fn reset_password(
    _admin: AdminEmployee,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    let temp = ea::generate_temp_password();
    let hash = ea::hash_password(&temp)
        .map_err(|e| AppError::Internal(format!("password hash failed: {e}")))?;

    let updated = sqlx::query("update employees set password_hash = $1 where id = $2")
        .bind(&hash)
        .bind(id)
        .execute(&state.db)
        .await?;
    if updated.rows_affected() == 0 {
        return Err(AppError::NotFound("employee not found".into()));
    }
    // Kick any active sessions so the old password stops working immediately.
    sqlx::query("delete from employee_sessions where employee_id = $1")
        .bind(id)
        .execute(&state.db)
        .await?;

    Ok(Json(json!({ "temp_password": temp })))
}

/// Clears MFA so the employee re-enrolls on next login (lost-device recovery).
pub async fn reset_mfa(
    _admin: AdminEmployee,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    let updated =
        sqlx::query("update employees set totp_secret = null, mfa_enrolled = false where id = $1")
            .bind(id)
            .execute(&state.db)
            .await?;
    if updated.rows_affected() == 0 {
        return Err(AppError::NotFound("employee not found".into()));
    }
    sqlx::query("delete from employee_sessions where employee_id = $1")
        .bind(id)
        .execute(&state.db)
        .await?;

    Ok(Json(json!({ "status": "ok" })))
}

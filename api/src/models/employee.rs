use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum EmployeeRole {
    Worker,
    Admin,
}

/// Full employee row. Holds secrets (`password_hash`, `totp_secret`) so it is
/// never serialized to a client directly — use `EmployeeView` for that.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Employee {
    pub id: Uuid,
    pub email: String,
    pub password_hash: String,
    pub role: EmployeeRole,
    pub totp_secret: Option<String>,
    pub mfa_enrolled: bool,
    pub active: bool,
    pub created_at: DateTime<Utc>,
    pub last_login_at: Option<DateTime<Utc>>,
}

/// Safe-to-expose projection (no secrets), for user management / `/me`.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct EmployeeView {
    pub id: Uuid,
    pub email: String,
    pub role: EmployeeRole,
    pub mfa_enrolled: bool,
    pub active: bool,
    pub created_at: DateTime<Utc>,
    pub last_login_at: Option<DateTime<Utc>>,
}

#[derive(Debug, sqlx::FromRow)]
pub struct EmployeeSession {
    pub id: Uuid,
    pub token_hash: String,
    pub employee_id: Uuid,
    pub mfa_pending: bool,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

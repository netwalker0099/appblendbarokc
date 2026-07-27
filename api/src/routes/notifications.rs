//! Admin management of chat notification targets (Discord / Slack / Teams).
//!
//! A webhook URL is a bearer credential — anyone holding it can post into the
//! channel — so it is write-only from the browser's point of view: it can be
//! set, never read back. Responses carry a redacted hint instead.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Extension;
use axum::Json;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::employee_auth::{AdminEmployee, AuthedEmployee};
use crate::error::AppError;
use crate::notify;
use crate::AppState;

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct NotificationTarget {
    pub id: Uuid,
    pub label: String,
    pub platform: String,
    pub active: bool,
    pub notify_online_sale: bool,
    pub notify_event_booked: bool,
    pub include_customer_email: bool,
    pub created_at: DateTime<Utc>,
    pub last_success_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    /// Enough to tell two webhooks apart, not enough to use one.
    pub url_hint: String,
}

const SELECT_COLS: &str = r#"
    id, label, platform, active, notify_online_sale, notify_event_booked,
    include_customer_email, created_at, last_success_at, last_error,
    -- Host plus a short tail. Never the secret path segment.
    (split_part(split_part(webhook_url, '//', 2), '/', 1) || '/…' ||
     right(webhook_url, 4)) as url_hint
"#;

pub async fn list(
    _admin: AdminEmployee,
    State(state): State<AppState>,
) -> Result<Json<Vec<NotificationTarget>>, AppError> {
    let rows = sqlx::query_as::<_, NotificationTarget>(&format!(
        "select {SELECT_COLS} from notification_targets order by created_at"
    ))
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

#[derive(Deserialize)]
pub struct CreateTarget {
    pub label: String,
    pub platform: String,
    pub webhook_url: String,
    #[serde(default = "yes")]
    pub notify_online_sale: bool,
    #[serde(default = "yes")]
    pub notify_event_booked: bool,
    #[serde(default)]
    pub include_customer_email: bool,
}

fn yes() -> bool {
    true
}

pub async fn create(
    _admin: AdminEmployee,
    Extension(employee): Extension<AuthedEmployee>,
    State(state): State<AppState>,
    Json(body): Json<CreateTarget>,
) -> Result<(StatusCode, Json<NotificationTarget>), AppError> {
    let label = body.label.trim();
    if label.is_empty() {
        return Err(AppError::BadRequest("a label is required".into()));
    }

    // Rejects non-https, credentials-in-URL, and anything off the per-platform
    // host allowlist — this endpoint makes the server fetch an admin-supplied
    // URL, which is the classic SSRF shape.
    notify::validate_webhook_url(&body.platform, &body.webhook_url).map_err(AppError::BadRequest)?;

    let row = sqlx::query_as::<_, NotificationTarget>(&format!(
        r#"
        insert into notification_targets
            (label, platform, webhook_url, notify_online_sale, notify_event_booked,
             include_customer_email, created_by)
        values ($1, $2, $3, $4, $5, $6, $7)
        returning {SELECT_COLS}
        "#
    ))
    .bind(label)
    .bind(&body.platform)
    .bind(body.webhook_url.trim())
    .bind(body.notify_online_sale)
    .bind(body.notify_event_booked)
    .bind(body.include_customer_email)
    .bind(employee.id)
    .fetch_one(&state.db)
    .await?;

    tracing::info!(target = %row.id, platform = %row.platform, "notification target added");
    Ok((StatusCode::CREATED, Json(row)))
}

#[derive(Deserialize)]
pub struct UpdateTarget {
    pub label: Option<String>,
    pub active: Option<bool>,
    pub notify_online_sale: Option<bool>,
    pub notify_event_booked: Option<bool>,
    pub include_customer_email: Option<bool>,
    /// Replaces the stored URL when present; omitted leaves it untouched.
    pub webhook_url: Option<String>,
}

pub async fn update(
    _admin: AdminEmployee,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateTarget>,
) -> Result<Json<NotificationTarget>, AppError> {
    if let Some(url) = &body.webhook_url {
        let platform: String =
            sqlx::query_scalar("select platform from notification_targets where id = $1")
                .bind(id)
                .fetch_optional(&state.db)
                .await?
                .ok_or_else(|| AppError::NotFound("target not found".into()))?;
        notify::validate_webhook_url(&platform, url).map_err(AppError::BadRequest)?;
    }

    let row = sqlx::query_as::<_, NotificationTarget>(&format!(
        r#"
        update notification_targets set
            label = coalesce($2, label),
            active = coalesce($3, active),
            notify_online_sale = coalesce($4, notify_online_sale),
            notify_event_booked = coalesce($5, notify_event_booked),
            include_customer_email = coalesce($6, include_customer_email),
            webhook_url = coalesce($7, webhook_url),
            updated_at = now()
        where id = $1
        returning {SELECT_COLS}
        "#
    ))
    .bind(id)
    .bind(body.label.as_deref().map(str::trim))
    .bind(body.active)
    .bind(body.notify_online_sale)
    .bind(body.notify_event_booked)
    .bind(body.include_customer_email)
    .bind(body.webhook_url.as_deref().map(str::trim))
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError::NotFound("target not found".into()))?;

    Ok(Json(row))
}

pub async fn delete(
    _admin: AdminEmployee,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    let n = sqlx::query("delete from notification_targets where id = $1")
        .bind(id)
        .execute(&state.db)
        .await?
        .rows_affected();
    if n == 0 {
        return Err(AppError::NotFound("target not found".into()));
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Post a sample message, so an admin can prove the plumbing before a real
/// customer depends on it.
pub async fn test(
    _admin: AdminEmployee,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    let row: Option<(String, String)> =
        sqlx::query_as("select platform, webhook_url from notification_targets where id = $1")
            .bind(id)
            .fetch_optional(&state.db)
            .await?;
    let (platform, url) = row.ok_or_else(|| AppError::NotFound("target not found".into()))?;

    let client = reqwest::Client::new();
    match notify::send_test(&client, &platform, &url).await {
        Ok(()) => {
            sqlx::query(
                "update notification_targets set last_success_at = now(), last_error = null where id = $1",
            )
            .bind(id)
            .execute(&state.db)
            .await?;
            Ok(Json(json!({ "ok": true, "detail": "Test message sent." })))
        }
        Err(reason) => {
            sqlx::query("update notification_targets set last_error = $2 where id = $1")
                .bind(id)
                .bind(&reason)
                .execute(&state.db)
                .await?;
            Ok(Json(json!({ "ok": false, "detail": reason })))
        }
    }
}

/// Recent delivery attempts, for debugging a channel that has gone quiet.
pub async fn recent(
    _admin: AdminEmployee,
    State(state): State<AppState>,
) -> Result<Json<Value>, AppError> {
    let rows = sqlx::query_as::<_, (Uuid, String, String, i32, Option<String>, DateTime<Utc>, String)>(
        r#"
        select d.id, d.event_type, d.status, d.attempts, d.last_error, d.created_at, t.label
        from notification_deliveries d
        join notification_targets t on t.id = d.target_id
        order by d.created_at desc
        limit 50
        "#,
    )
    .fetch_all(&state.db)
    .await?;

    let items: Vec<Value> = rows
        .into_iter()
        .map(|(id, event_type, status, attempts, last_error, created_at, label)| {
            json!({
                "id": id,
                "event_type": event_type,
                "status": status,
                "attempts": attempts,
                "last_error": last_error,
                "created_at": created_at,
                "target": label,
            })
        })
        .collect();

    Ok(Json(json!({ "deliveries": items })))
}

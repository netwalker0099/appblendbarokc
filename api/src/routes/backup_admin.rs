//! Admin management of scheduled backups.
//!
//! Everything here is admin-only. The endpoints configure, and can trigger, a
//! full export of every customer record the business holds — a worker having
//! this would be a bigger hole than the manual download button, not a smaller
//! one.
//!
//! The passphrase is write-only from the browser's point of view: it can be set
//! and its presence reported, never read back. Same treatment as the chat
//! webhook URLs and the Google key, and for the same reason — an endpoint that
//! returns it turns any admin session into a copy of the decryption key.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::backup::{self, destination, schedule, DestinationRow};
use crate::employee_auth::AdminEmployee;
use crate::error::AppError;
use crate::AppState;

const SELECT_COLS: &str = r#"
    id, label, kind, config, schedule, timezone, retain_count, enabled,
    last_run_at, last_status, last_error, next_run_at, created_at
"#;

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct Destination {
    pub id: Uuid,
    pub label: String,
    pub kind: String,
    pub config: Value,
    pub schedule: String,
    pub timezone: String,
    pub retain_count: i32,
    pub enabled: bool,
    pub last_run_at: Option<DateTime<Utc>>,
    pub last_status: Option<String>,
    pub last_error: Option<String>,
    pub next_run_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// Overall readiness, so the page can say what is missing rather than leaving
/// someone to infer it from a failed run.
pub async fn status(
    _admin: AdminEmployee,
    State(state): State<AppState>,
) -> Result<Json<Value>, AppError> {
    let destinations: i64 =
        sqlx::query_scalar("select count(*) from backup_destinations where enabled")
            .fetch_one(&state.db)
            .await?;

    // A destination existing is not the same as a backup having happened.
    let last_success: Option<DateTime<Utc>> =
        sqlx::query_scalar("select max(started_at) from backup_runs where status = 'ok'")
            .fetch_optional(&state.db)
            .await?
            .flatten();

    Ok(Json(json!({
        "passphrase_set": backup::passphrase::is_set(),
        "passphrase_env_managed": backup::passphrase::env_managed(),
        // Drive reuses the mailer's key, so "can we back up to Drive" is
        // answered by whether Google is connected at all.
        "google_connected": crate::email::gmail::load_service_account()
            .or_else(crate::email::credentials::load_stored)
            .is_some(),
        "email_live": state.mailer().is_live(),
        "enabled_destinations": destinations,
        "last_success_at": last_success,
        "sharepoint_available": false,
    })))
}

#[derive(Deserialize)]
pub struct SetPassphrase {
    pub passphrase: String,
}

/// Set (or replace) the encryption passphrase.
///
/// Replacing it does not re-encrypt anything: existing backups still need the
/// old passphrase, and the response says so. Silently invalidating every
/// historical backup would be the worst possible thing to do quietly.
pub async fn set_passphrase(
    _admin: AdminEmployee,
    Json(body): Json<SetPassphrase>,
) -> Result<Json<Value>, AppError> {
    let had_one = backup::passphrase::is_set();
    backup::passphrase::store(&body.passphrase)?;
    tracing::info!("backup passphrase {}", if had_one { "replaced" } else { "set" });

    Ok(Json(json!({
        "ok": true,
        "replaced": had_one,
        "note": if had_one {
            "Backups taken before now still need the OLD passphrase — keep it somewhere safe."
        } else {
            "Store this passphrase somewhere that is not this server. Without it the backups cannot be restored."
        },
    })))
}

pub async fn list(
    _admin: AdminEmployee,
    State(state): State<AppState>,
) -> Result<Json<Vec<Destination>>, AppError> {
    let rows = sqlx::query_as::<_, Destination>(&format!(
        "select {SELECT_COLS} from backup_destinations order by created_at"
    ))
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

#[derive(Deserialize)]
pub struct CreateDestination {
    pub label: String,
    pub kind: String,
    #[serde(default)]
    pub config: Value,
    pub schedule: String,
    #[serde(default = "default_tz")]
    pub timezone: String,
    #[serde(default = "default_retain")]
    pub retain_count: i32,
}

fn default_tz() -> String {
    "America/Chicago".to_string()
}

fn default_retain() -> i32 {
    30
}

pub async fn create(
    _admin: AdminEmployee,
    State(state): State<AppState>,
    Json(body): Json<CreateDestination>,
) -> Result<(StatusCode, Json<Destination>), AppError> {
    let label = body.label.trim();
    if label.is_empty() {
        return Err(AppError::BadRequest("a label is required".into()));
    }

    let config = if body.config.is_null() {
        json!({})
    } else {
        body.config.clone()
    };

    // All three checks up front. A destination that cannot work should be
    // refused by the form, not discovered as a red row at 2am — by which point
    // that night's backup has already not happened.
    destination::validate(&body.kind, &config)?;
    let next = schedule::next_after(&body.schedule, &body.timezone, Utc::now())
        .map_err(AppError::BadRequest)?;
    if !(1..=3650).contains(&body.retain_count) {
        return Err(AppError::BadRequest(
            "keep between 1 and 3650 backups".into(),
        ));
    }

    let row = sqlx::query_as::<_, Destination>(&format!(
        r#"
        insert into backup_destinations
            (label, kind, config, schedule, timezone, retain_count, next_run_at)
        values ($1, $2, $3, $4, $5, $6, $7)
        returning {SELECT_COLS}
        "#
    ))
    .bind(label)
    .bind(&body.kind)
    .bind(&config)
    .bind(body.schedule.trim())
    .bind(&body.timezone)
    .bind(body.retain_count)
    .bind(next)
    .fetch_one(&state.db)
    .await?;

    tracing::info!(destination = %row.id, kind = %row.kind, "backup destination added");
    Ok((StatusCode::CREATED, Json(row)))
}

#[derive(Deserialize)]
pub struct UpdateDestination {
    pub label: Option<String>,
    pub config: Option<Value>,
    pub schedule: Option<String>,
    pub timezone: Option<String>,
    pub retain_count: Option<i32>,
    pub enabled: Option<bool>,
}

pub async fn update(
    _admin: AdminEmployee,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateDestination>,
) -> Result<Json<Destination>, AppError> {
    let existing = sqlx::query_as::<_, Destination>(&format!(
        "select {SELECT_COLS} from backup_destinations where id = $1"
    ))
    .bind(id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError::NotFound("no such backup destination".into()))?;

    let config = body.config.clone().unwrap_or(existing.config);
    let schedule_expr = body
        .schedule
        .clone()
        .unwrap_or(existing.schedule)
        .trim()
        .to_string();
    let timezone = body.timezone.clone().unwrap_or(existing.timezone);
    let retain = body.retain_count.unwrap_or(existing.retain_count);
    let enabled = body.enabled.unwrap_or(existing.enabled);

    destination::validate(&existing.kind, &config)?;
    if !(1..=3650).contains(&retain) {
        return Err(AppError::BadRequest(
            "keep between 1 and 3650 backups".into(),
        ));
    }

    // Recomputed on every edit, not just when the expression changes: a timezone
    // change moves the next run too, and leaving a stale next_run_at would fire
    // the old schedule one more time.
    let next = schedule::next_after(&schedule_expr, &timezone, Utc::now())
        .map_err(AppError::BadRequest)?;

    let row = sqlx::query_as::<_, Destination>(&format!(
        r#"
        update backup_destinations
           set label = coalesce($2, label),
               config = $3,
               schedule = $4,
               timezone = $5,
               retain_count = $6,
               enabled = $7,
               next_run_at = case when $7 then $8 else null end,
               updated_at = now()
         where id = $1
        returning {SELECT_COLS}
        "#
    ))
    .bind(id)
    .bind(body.label.as_deref().map(str::trim).filter(|s| !s.is_empty()))
    .bind(&config)
    .bind(&schedule_expr)
    .bind(&timezone)
    .bind(retain)
    .bind(enabled)
    .bind(next)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(row))
}

pub async fn delete(
    _admin: AdminEmployee,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    let result = sqlx::query("delete from backup_destinations where id = $1")
        .bind(id)
        .execute(&state.db)
        .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound("no such backup destination".into()));
    }
    // The run history goes with it (on delete cascade). Files already uploaded
    // are left alone — deleting a schedule is not a request to destroy the
    // backups it made.
    tracing::info!(destination = %id, "backup destination removed");
    Ok(StatusCode::NO_CONTENT)
}

/// Run one destination now.
///
/// This is the button that proves the whole chain works — dump, encrypt,
/// upload — without waiting for 2am. It runs inline rather than handing off to
/// the worker so the caller gets the real error, which is the entire point of
/// pressing it.
pub async fn run_now(
    _admin: AdminEmployee,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    let dest = sqlx::query_as::<_, DestinationRow>(
        "select id, label, kind, config, schedule, timezone, retain_count \
         from backup_destinations where id = $1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError::NotFound("no such backup destination".into()))?;

    let artifact = backup::build_artifact().await?;
    let bytes = artifact.bytes.len();
    let filename = artifact.filename.clone();

    backup::run_once(&state, &dest, &artifact, "manual").await?;
    backup::prune(&state, &dest).await;

    Ok(Json(json!({
        "ok": true,
        "filename": filename,
        "bytes": bytes,
        "plain_bytes": artifact.plain_bytes,
    })))
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct Run {
    pub id: Uuid,
    pub destination_id: Uuid,
    pub destination_label: String,
    pub trigger: String,
    pub status: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub filename: Option<String>,
    pub bytes: Option<i64>,
    pub error: Option<String>,
}

/// Recent runs across every destination — the answer to "is this actually
/// working", which is the only question that matters about a backup system.
pub async fn runs(
    _admin: AdminEmployee,
    State(state): State<AppState>,
) -> Result<Json<Vec<Run>>, AppError> {
    let rows = sqlx::query_as::<_, Run>(
        r#"
        select r.id, r.destination_id, d.label as destination_label, r.trigger, r.status,
               r.started_at, r.finished_at, r.filename, r.bytes, r.error
          from backup_runs r
          join backup_destinations d on d.id = r.destination_id
         order by r.started_at desc
         limit 50
        "#,
    )
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

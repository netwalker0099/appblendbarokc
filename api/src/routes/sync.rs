use axum::extract::State;
use axum::Json;
use serde_json::{json, Value};

use crate::employee_auth::AdminEmployee;
use crate::error::AppError;
use crate::models::sync::SyncJob;
use crate::AppState;

/// Observability for the contact outbox: how many jobs sit in each state, and
/// the most recent permanent failures. Billing/reconciliation status lives
/// separately at `/api/square/status`.
pub async fn status(
    _admin: AdminEmployee,
    State(state): State<AppState>,
) -> Result<Json<Value>, AppError> {
    let rows = sqlx::query_as::<_, (String, i64)>(
        "select status, count(*) from sync_outbox group by status",
    )
    .fetch_all(&state.db)
    .await?;

    let mut counts = serde_json::Map::new();
    for status in ["pending", "succeeded", "failed"] {
        counts.insert(status.to_string(), json!(0));
    }
    for (status, count) in rows {
        counts.insert(status, json!(count));
    }

    let recent_failures = sqlx::query_as::<_, SyncJob>(
        "select * from sync_outbox where status = 'failed' order by updated_at desc limit 20",
    )
    .fetch_all(&state.db)
    .await?;

    Ok(Json(json!({
        "backend": state.square.name(),
        "counts": counts,
        "recent_failures": recent_failures,
    })))
}

/// Requeue every permanently-failed job for another pass — the manual "try again
/// now" after fixing whatever broke (e.g. finally setting the API key).
pub async fn retry(
    _admin: AdminEmployee,
    State(state): State<AppState>,
) -> Result<Json<Value>, AppError> {
    let requeued = sqlx::query(
        "update sync_outbox set status = 'pending', attempts = 0, last_error = null, next_attempt_at = now(), updated_at = now() where status = 'failed'",
    )
    .execute(&state.db)
    .await?
    .rows_affected();

    Ok(Json(json!({ "requeued": requeued })))
}

//! Uploading a backup file to inspect or restore.
//!
//! Two behaviours behind one endpoint, chosen by a header:
//!
//!   * **No `X-Restore-Confirm`** — decrypt, validate, load into a scratch
//!     database, report what is inside. The live database is not touched. This
//!     is what the browser does when a file is chosen, so the operator sees the
//!     contents *before* deciding.
//!   * **`X-Restore-Confirm: REPLACE ALL DATA`** — all of the above, then take a
//!     safety copy and replace the live database.
//!
//! Destruction being opt-in via an explicit header, rather than a flag that
//! defaults to something, means a truncated or malformed request inspects
//! instead of destroying.

use axum::body::Bytes;
use axum::http::HeaderMap;
use axum::response::Response;
use axum::Json;
use serde_json::{json, Value};

use crate::backup::restore;
use crate::employee_auth::AdminEmployee;
use crate::error::AppError;

/// Inspect, or replace the database, depending on the confirmation header.
pub async fn upload(
    _admin: AdminEmployee,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, AppError> {
    if body.is_empty() {
        return Err(AppError::BadRequest("no file was uploaded".into()));
    }

    let confirmed = headers
        .get("x-restore-confirm")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        == Some(restore::CONFIRM_PHRASE);

    if !confirmed {
        let report = restore::inspect(&body).await?;
        return Ok(Json(json!({
            "restored": false,
            "report": report,
            "confirm_with": restore::CONFIRM_PHRASE,
        })));
    }

    // Recorded before the restore runs, because the restore replaces the audit
    // log with the backup's own. The middleware entry lands afterwards, in the
    // *restored* log, chained onto its head — so both halves survive: this line
    // in the server log, and the completed action in the new chain.
    tracing::warn!(
        bytes = body.len(),
        "admin confirmed a full database restore from an uploaded file"
    );

    let outcome = restore::restore(&body).await?;

    // The connection pool holds prepared statements planned against the old
    // schema. If the backup predates a migration, those plans are now wrong and
    // queries fail with "cached plan must not change result type" until the
    // process restarts. Compose has `restart: unless-stopped`, so exiting is the
    // restart — done after the response is sent.
    tokio::spawn(async {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        tracing::warn!("restarting after restore to rebuild the connection pool");
        std::process::exit(0);
    });

    Ok(Json(json!({
        "restored": outcome.restored,
        "report": outcome.report,
        "safety_copy": outcome.safety_copy,
        "rows_before": outcome.rows_before,
        "rows_after": outcome.rows_after,
        "note": "The app is restarting to pick up the restored database. It will be back \
                 in a few seconds.",
    })))
}

/// The pre-restore safety copies still on disk.
pub async fn safety_copies(_admin: AdminEmployee) -> Result<Json<Value>, AppError> {
    let copies: Vec<Value> = restore::list_safety_copies()
        .into_iter()
        .map(|(name, bytes)| json!({ "name": name, "bytes": bytes }))
        .collect();
    Ok(Json(json!({ "copies": copies })))
}

/// Download one, so the way back does not depend on shell access.
pub async fn download_safety_copy(
    _admin: AdminEmployee,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> Result<Response, AppError> {
    let bytes = restore::read_safety_copy(&name)?;
    Response::builder()
        .header(axum::http::header::CONTENT_TYPE, "application/octet-stream")
        .header(
            axum::http::header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{name}\""),
        )
        .body(axum::body::Body::from(bytes))
        .map_err(|e| AppError::Internal(e.to_string()))
}

use axum::body::Body;
use axum::http::header;
use axum::response::Response;
use tokio::process::Command;

use crate::employee_auth::AdminEmployee;
use crate::error::AppError;

/// Full `pg_dump` of the database, returned as a downloadable `.sql` file.
///
/// Restore into a fresh Postgres 16 with:
///   `psql "<DATABASE_URL of the new instance>" < blendbar-backup-*.sql`
/// (`--no-owner --no-privileges` makes it portable across instances/roles). The
/// dump includes `_sqlx_migrations`, so the app treats the schema as already
/// migrated on first boot against the restored DB.
///
/// Behind operator auth. NOTE: this is a full export of ALL customer PII and the
/// (hashed) device tokens — the download is as sensitive as the database itself.
/// No request input reaches the command, so there's no injection surface.
pub async fn backup(_admin: AdminEmployee) -> Result<Response, AppError> {
    let database_url = std::env::var("DATABASE_URL")
        .map_err(|_| AppError::Internal("DATABASE_URL not set".into()))?;

    let output = Command::new("pg_dump")
        .args(["--no-owner", "--no-privileges"])
        .arg(&database_url)
        .output()
        .await
        .map_err(|e| AppError::Internal(format!("could not run pg_dump: {e}")))?;

    if !output.status.success() {
        tracing::error!(
            "pg_dump failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        return Err(AppError::Internal("backup failed".into()));
    }

    let filename = format!(
        "blendbar-backup-{}.sql",
        chrono::Utc::now().format("%Y%m%d-%H%M%S")
    );

    Response::builder()
        .header(header::CONTENT_TYPE, "application/sql")
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{filename}\""),
        )
        .body(Body::from(output.stdout))
        .map_err(|e| AppError::Internal(e.to_string()))
}

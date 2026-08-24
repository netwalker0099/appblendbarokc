use axum::body::Body;
use axum::http::header;
use axum::response::Response;

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
/// This is the *manual pull*: unencrypted, straight to the browser, for taking a
/// snapshot before a risky change. The scheduled equivalent in `crate::backup`
/// encrypts and sends the same dump somewhere off this box; both call the same
/// `pg_dump` so the two can never drift into producing different output — the
/// restore instructions have to stay true for both.
///
/// Behind admin auth. NOTE: this is a full export of ALL customer PII — the
/// download is as sensitive as the database itself. No request input reaches the
/// command, so there is no injection surface.
pub async fn backup(_admin: AdminEmployee) -> Result<Response, AppError> {
    let dump = crate::backup::pg_dump().await?;

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
        .body(Body::from(dump))
        .map_err(|e| AppError::Internal(e.to_string()))
}

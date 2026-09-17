//! Event booking enquiries.
//!
//! Two audiences, two access levels, one table:
//!
//! - [`submit`] is **public and unauthenticated** — it is the form at `/book` on
//!   the customer site. It is the only route here that anyone on the internet
//!   can reach, and it is treated accordingly: rate limited, every field bounded
//!   and validated server-side, and it returns the same shape whatever happens
//!   so it cannot be used to probe for anything.
//! - The rest require an admin session and are the working view: list, read,
//!   update status, leave an internal note.
//!
//! The browser's captcha, honeypot and timing gate (see `site/book.js`) are
//! conveniences for the human and stop naive form-fillers. They are **not** a
//! boundary — anything POSTing here directly skips all three — which is why the
//! real limits live in this file.

use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::Json;
use chrono::{Datelike, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::employee_auth::{AdminEmployee, AuthedEmployee};
use crate::error::AppError;
use crate::models::enquiry::{is_valid_status, EventEnquiry, STATUSES};
use crate::ratelimit::client_key;
use crate::AppState;

/// Field caps. These match the `maxlength` attributes on the form, but are
/// enforced here because the form is not what protects the database.
const MAX_NAME: usize = 80;
const MAX_EMAIL: usize = 254;
const MAX_PHONE: usize = 40;
const MAX_DETAILS: usize = 2000;

/// How far ahead an event may be booked. Someone typing 2087 has mis-keyed a
/// year; accepting it silently puts a row in the list that sorts strangely
/// forever.
const MAX_YEARS_AHEAD: i32 = 5;

#[derive(Deserialize)]
pub struct SubmitRequest {
    pub first_name: String,
    pub last_name: String,
    pub email: String,
    pub phone: String,
    pub event_date: String,
    pub details: String,
}

#[derive(Serialize)]
pub struct SubmitResponse {
    pub received: bool,
}

/// Trim, then reject if empty or over the cap. Returns the cleaned value.
fn required(value: &str, field: &str, max: usize) -> Result<String, AppError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(AppError::BadRequest(format!("{field} is required")));
    }
    if value.chars().count() > max {
        return Err(AppError::BadRequest(format!(
            "{field} must be {max} characters or fewer"
        )));
    }
    Ok(value.to_string())
}

/// POST /api/public/event-enquiry — the public booking form.
pub async fn submit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<SubmitRequest>,
) -> Result<Json<SubmitResponse>, AppError> {
    // First line of defence and the only one that applies before any work is
    // done. This endpoint writes a row and pings a chat channel, so without a
    // cap one laptop could both fill the table and make the team's Discord
    // unusable — which would get notifications switched off, losing the real
    // ones too.
    if !state.enquiry_limiter.check(&client_key(&headers)) {
        return Err(AppError::TooManyRequests(
            "too many enquiries from this connection — please wait a few minutes, \
             or message us on Instagram"
                .into(),
        ));
    }

    let first_name = required(&body.first_name, "first name", MAX_NAME)?;
    let last_name = required(&body.last_name, "last name", MAX_NAME)?;
    let details = required(&body.details, "event details", MAX_DETAILS)?;

    let email = required(&body.email, "email", MAX_EMAIL)?.to_lowercase();
    // The same shallow check the public checkout uses. Deliberately not a full
    // RFC 5322 parse: the only thing that proves an address works is sending to
    // it, and over-strict patterns reject real addresses.
    if !email.contains('@') || email.starts_with('@') || email.ends_with('@') {
        return Err(AppError::BadRequest("a valid email is required".into()));
    }

    let phone = required(&body.phone, "phone number", MAX_PHONE)?;
    if phone.chars().filter(char::is_ascii_digit).count() < 7 {
        return Err(AppError::BadRequest(
            "a valid phone number is required".into(),
        ));
    }

    let event_date = NaiveDate::parse_from_str(body.event_date.trim(), "%Y-%m-%d")
        .map_err(|_| AppError::BadRequest("event date must be a real date".into()))?;
    let today = Utc::now().date_naive();
    // One day of slack rather than a hard `< today`: a submission at 11pm in a
    // timezone behind UTC would otherwise be rejected for "yesterday".
    if event_date < today - chrono::Duration::days(1) {
        return Err(AppError::BadRequest(
            "that event date is in the past".into(),
        ));
    }
    if event_date.year() > today.year() + MAX_YEARS_AHEAD {
        return Err(AppError::BadRequest(
            "that event date is too far in the future".into(),
        ));
    }

    // The insert and the notification queue share one transaction. Either the
    // enquiry is stored *and* the team is told, or neither happened — an enquiry
    // nobody hears about is the failure this whole feature exists to prevent.
    let mut tx = state.db.begin().await?;

    let id: Uuid = sqlx::query_scalar(
        "insert into event_enquiries (first_name, last_name, email, phone, event_date, details) \
         values ($1, $2, $3, $4, $5, $6) returning id",
    )
    .bind(&first_name)
    .bind(&last_name)
    .bind(&email)
    .bind(&phone)
    .bind(event_date)
    .bind(&details)
    .fetch_one(&mut *tx)
    .await?;

    crate::notify::enqueue_for_enquiry(&mut tx, id).await?;
    tx.commit().await?;

    tracing::info!(enquiry = %id, "event enquiry received");
    Ok(Json(SubmitResponse { received: true }))
}

#[derive(Deserialize)]
pub struct ListQuery {
    /// `open` (new + contacted), `all`, or one exact status.
    pub status: Option<String>,
}

/// GET /api/enquiries
pub async fn list(
    _admin: AdminEmployee,
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Value>, AppError> {
    let filter = q.status.as_deref().unwrap_or("all");

    let rows: Vec<EventEnquiry> = match filter {
        "all" => {
            sqlx::query_as("select * from event_enquiries order by created_at desc limit 500")
                .fetch_all(&state.db)
                .await?
        }
        "open" => {
            sqlx::query_as(
                "select * from event_enquiries where status in ('new', 'contacted') \
                 order by created_at desc limit 500",
            )
            .fetch_all(&state.db)
            .await?
        }
        s if is_valid_status(s) => {
            sqlx::query_as(
                "select * from event_enquiries where status = $1 \
                 order by created_at desc limit 500",
            )
            .bind(s)
            .fetch_all(&state.db)
            .await?
        }
        other => {
            return Err(AppError::BadRequest(format!(
                "unknown status filter '{other}'"
            )))
        }
    };

    // The count of things still needing a human, for the tab's attention dot.
    let open_count: i64 = sqlx::query_scalar(
        "select count(*) from event_enquiries where status in ('new', 'contacted')",
    )
    .fetch_one(&state.db)
    .await?;

    Ok(Json(json!({ "enquiries": rows, "open_count": open_count })))
}

#[derive(Deserialize)]
pub struct UpdateRequest {
    pub status: Option<String>,
    pub staff_notes: Option<String>,
}

/// PATCH /api/enquiries/:id
pub async fn update(
    admin: AdminEmployee,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateRequest>,
) -> Result<Json<EventEnquiry>, AppError> {
    let AdminEmployee(AuthedEmployee { id: actor, .. }) = admin;

    if let Some(status) = &body.status {
        if !is_valid_status(status) {
            return Err(AppError::BadRequest(format!(
                "status must be one of {}",
                STATUSES.join(", ")
            )));
        }
    }
    if let Some(notes) = &body.staff_notes {
        if notes.chars().count() > MAX_DETAILS {
            return Err(AppError::BadRequest(format!(
                "notes must be {MAX_DETAILS} characters or fewer"
            )));
        }
    }
    if body.status.is_none() && body.staff_notes.is_none() {
        return Err(AppError::BadRequest("nothing to update".into()));
    }

    // `handled_by`/`handled_at` record who last moved it off 'new' and when, so
    // "I thought you were dealing with that" has an answer. Only stamped on a
    // status change — editing a note is not taking ownership.
    let updated: Option<EventEnquiry> = sqlx::query_as(
        r#"
        update event_enquiries set
            status      = coalesce($2, status),
            staff_notes = coalesce($3, staff_notes),
            handled_by  = case when $2 is null then handled_by else $4 end,
            handled_at  = case when $2 is null then handled_at else now() end,
            updated_at  = now()
        where id = $1
        returning *
        "#,
    )
    .bind(id)
    .bind(body.status.as_deref())
    .bind(body.staff_notes.as_deref())
    .bind(actor)
    .fetch_optional(&state.db)
    .await?;

    updated
        .map(Json)
        .ok_or_else(|| AppError::NotFound("enquiry not found".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_trims_and_bounds() {
        assert_eq!(required("  Ada  ", "first name", 80).unwrap(), "Ada");
        assert!(required("   ", "first name", 80).is_err());
        assert!(required(&"x".repeat(81), "first name", 80).is_err());
        // Counted in characters, not bytes — an 80-character name of accented
        // letters is a real name, not an overflow attempt.
        assert!(required(&"é".repeat(80), "first name", 80).is_ok());
    }

    #[test]
    fn status_values_are_closed() {
        for s in STATUSES {
            assert!(is_valid_status(s));
        }
        assert!(!is_valid_status("spam"));
        assert!(!is_valid_status(""));
        // The filter word is not itself a status; it must not slip through a
        // PATCH as one.
        assert!(!is_valid_status("open"));
    }
}

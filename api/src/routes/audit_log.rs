//! Reading the audit log.
//!
//! Read-only by construction: there is no handler here that writes, edits or
//! deletes an entry, and the database refuses those operations anyway (see
//! `0019_admin_audit_log.sql`). Admin-only — the log records who did what, which
//! is management information, not shop-floor information.

use axum::extract::{Query, State};
use axum::Json;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::employee_auth::AdminEmployee;
use crate::error::AppError;
use crate::AppState;

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct Entry {
    pub id: i64,
    pub at: DateTime<Utc>,
    pub actor_id: Option<Uuid>,
    pub actor_email: String,
    pub actor_role: String,
    pub method: String,
    pub path: String,
    pub status: i32,
    pub ip: Option<String>,
    pub summary: Option<String>,
    pub detail: Option<Value>,
    /// Shown in the UI so an entry can be quoted in an email or a ticket and
    /// still be checkable against the chain afterwards.
    pub entry_hash: String,
}

#[derive(Deserialize)]
pub struct ListQuery {
    /// Filter to one person.
    pub actor: Option<String>,
    /// `admin`, `worker`, or absent for everyone.
    pub role: Option<String>,
    /// Substring match on the path, for "everything that touched pricing".
    pub path: Option<String>,
    /// Hide the successes. The 4xx/5xx rows are the interesting ones when
    /// something is being investigated.
    #[serde(default)]
    pub failures_only: bool,
    #[serde(default = "default_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
}

fn default_limit() -> i64 {
    100
}

pub async fn list(
    _admin: AdminEmployee,
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Value>, AppError> {
    // Capped regardless of what was asked for: this table only grows, and an
    // unbounded query against it is a slow request that gets slower every month.
    let limit = q.limit.clamp(1, 500);
    let offset = q.offset.max(0);

    let rows = sqlx::query_as::<_, Entry>(
        r#"
        select id, at, actor_id, actor_email, actor_role, method, path, status,
               ip, summary, detail, entry_hash
          from admin_audit_log
         where ($1::text is null or actor_email ilike '%' || $1 || '%')
           and ($2::text is null or actor_role = $2)
           and ($3::text is null or path ilike '%' || $3 || '%')
           and (not $4::boolean or status >= 400)
         order by id desc
         limit $5 offset $6
        "#,
    )
    .bind(q.actor.as_deref().filter(|s| !s.trim().is_empty()))
    .bind(q.role.as_deref().filter(|s| !s.trim().is_empty()))
    .bind(q.path.as_deref().filter(|s| !s.trim().is_empty()))
    .bind(q.failures_only)
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.db)
    .await?;

    let total: i64 = sqlx::query_scalar("select count(*) from admin_audit_log")
        .fetch_one(&state.db)
        .await?;

    Ok(Json(json!({
        "entries": rows,
        "total": total,
        "limit": limit,
        "offset": offset,
    })))
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct Segment {
    pub id: i64,
    pub from_id: i64,
    pub to_id: i64,
    pub entry_count: i64,
    pub from_at: DateTime<Utc>,
    pub to_at: DateTime<Utc>,
    pub content_sha256: String,
    pub filename: String,
    pub destinations: Value,
    pub created_at: DateTime<Utc>,
}

/// History that has been archived off-box and pruned from the table.
///
/// These rows are the receipt. They are never removed, they are tiny, and they
/// are what lets the chain still verify across the gap — so an archived span is
/// visibly *archived* rather than indistinguishable from a deletion.
pub async fn segments(
    _admin: AdminEmployee,
    State(state): State<AppState>,
) -> Result<Json<Vec<Segment>>, AppError> {
    let rows = sqlx::query_as::<_, Segment>(
        "select id, from_id, to_id, entry_count, from_at, to_at, content_sha256, \
                filename, destinations, created_at \
           from audit_archive_segments order by from_id desc",
    )
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

/// Archive now, rather than waiting for the worker's tick.
///
/// Runs inline so the caller gets the real error. Retention refuses to prune
/// anything it could not deliver, and this is the button that surfaces *why*
/// rather than leaving it in the server log.
pub async fn archive_now(
    _admin: AdminEmployee,
    State(state): State<AppState>,
) -> Result<Json<Value>, AppError> {
    match crate::audit::archive::run(&state).await? {
        Some(o) => Ok(Json(json!({
            "archived": true,
            "segment_id": o.segment_id,
            "entry_count": o.entry_count,
            "filename": o.filename,
            "bytes": o.bytes,
            "delivered_to": o.delivered_to,
        }))),
        None => Ok(Json(json!({
            "archived": false,
            "reason": "nothing is older than the retention window (or retention is off)",
        }))),
    }
}

/// Recompute the hash chain and report any break.
///
/// This is the part that makes the log worth trusting. Without it the chain is
/// decoration: the value of tamper-evidence is entirely in somebody actually
/// checking, and a button is the only way that ever happens.
///
/// The recomputation runs in Postgres against the same function the writer uses,
/// so the verifier cannot drift from the writer and start reporting tampering
/// that never occurred.
pub async fn verify(
    _admin: AdminEmployee,
    State(state): State<AppState>,
) -> Result<Json<Value>, AppError> {
    let breaks: Vec<(i64, String)> = sqlx::query_as("select * from verify_admin_audit_chain()")
        .fetch_all(&state.db)
        .await?;

    let total: i64 = sqlx::query_scalar("select count(*) from admin_audit_log")
        .fetch_one(&state.db)
        .await?;

    // The current head, so it can be written down somewhere outside this box.
    // Anchoring the chain externally is what would turn this from
    // tamper-evident into tamper-proof; recording the head by hand is the
    // low-tech version of that and costs nothing.
    let head: Option<String> =
        sqlx::query_scalar("select entry_hash from admin_audit_log order by id desc limit 1")
            .fetch_optional(&state.db)
            .await?;

    if !breaks.is_empty() {
        tracing::error!(
            "AUDIT CHAIN VERIFICATION FAILED at {} entr{}",
            breaks.len(),
            if breaks.len() == 1 { "y" } else { "ies" }
        );
    }

    Ok(Json(json!({
        "intact": breaks.is_empty(),
        "entries_checked": total,
        "head": head,
        "breaks": breaks
            .into_iter()
            .map(|(id, reason)| json!({ "id": id, "reason": reason }))
            .collect::<Vec<_>>(),
    })))
}

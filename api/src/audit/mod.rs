//! Recording what staff did.
//!
//! ## Why this is middleware and not a call in each handler
//!
//! An audit log's only real failure mode is the entry that was never written.
//! A `record(...)` call at the top of every admin handler works until someone
//! adds a route and forgets — and nothing breaks, no test fails, and the gap is
//! discovered months later by the person trying to find out who deleted a
//! customer. Middleware cannot be forgotten: a new route is covered the moment
//! it exists.
//!
//! ## What gets logged
//!
//! Every **state-changing** request (POST/PUT/PATCH/DELETE) from an authenticated
//! employee, plus a small list of reads that are themselves sensitive — the
//! database download most of all, since it exports every customer record in the
//! business and leaves no other trace.
//!
//! Not an allowlist of admin paths. That would be the same trap as the
//! per-handler call: a list that drifts out of date silently. Logging every
//! employee mutation over-covers slightly (a worker's intake submission is also
//! recorded), and the UI defaults to showing admin actions only. Over-coverage
//! is recoverable; a missing entry is not.
//!
//! Reads are otherwise skipped deliberately. Logging every GET would bury the
//! twelve entries a month that matter under thousands that do not, and an audit
//! log nobody can read is an audit log nobody reads.
//!
//! ## What must never reach it
//!
//! Passwords, MFA codes, the backup passphrase, service-account keys, webhook
//! URLs, session tokens. A log that captured those would become the single
//! richest target in the system — it is append-only, so a secret written into it
//! cannot even be removed afterwards. [`redact`] works from a denylist of key
//! names and is applied before anything is stored.

pub mod archive;

use axum::body::{Body, Bytes};
use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;
use serde_json::{json, Value};

use crate::employee_auth::AuthedEmployee;
use crate::models::employee::EmployeeRole;
use crate::AppState;

/// Bodies larger than this are not captured — only the fact of the request. No
/// realistic admin action sends more (the biggest is a service-account key at a
/// couple of KB, and that is redacted anyway).
const MAX_BODY_CAPTURE: usize = 64 * 1024;

/// Reads worth recording, because the read *is* the sensitive act.
const AUDITED_READS: &[&str] = &[
    // A full export of every customer record. Nothing else in the system marks
    // that it happened.
    "/api/admin/backup",
];

/// Key names whose values are replaced with a marker before storage.
///
/// Matched as a substring on the lowercased key, so `new_password`,
/// `current_password` and `password` are all caught by one entry. Erring towards
/// over-redaction is deliberate: a redacted field costs a little context, an
/// unredacted secret costs the secret — permanently, in an append-only table.
const SENSITIVE_KEYS: &[&str] = &[
    "password",
    "passphrase",
    "secret",
    "token",
    "webhook_url",
    "private_key",
    "service_account",
    "key_json",
    "code",
    "totp",
    "credential",
    "authorization",
];

const REDACTED: &str = "[redacted]";

/// Replace the values of sensitive-looking keys, recursively.
pub fn redact(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| {
                    let lower = k.to_lowercase();
                    if SENSITIVE_KEYS.iter().any(|s| lower.contains(s)) {
                        (k.clone(), Value::String(REDACTED.into()))
                    } else {
                        (k.clone(), redact(v))
                    }
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(redact).collect()),
        other => other.clone(),
    }
}

/// A short human-readable line for the list view, so the common case is legible
/// without expanding the JSON.
pub fn summarise(method: &str, path: &str, detail: Option<&Value>) -> String {
    let noun = path
        .trim_start_matches("/api/")
        .split('/')
        .next()
        .unwrap_or(path);

    let label = detail
        .and_then(|d| {
            ["label", "name", "email", "title"]
                .iter()
                .find_map(|k| d.get(*k).and_then(|v| v.as_str()))
        })
        .map(|s| format!(" “{s}”"))
        .unwrap_or_default();

    let verb = match method {
        "POST" => "created",
        "PATCH" | "PUT" => "changed",
        "DELETE" => "deleted",
        _ => "accessed",
    };

    format!("{verb} {noun}{label}")
}

fn should_audit(method: &str, path: &str) -> bool {
    matches!(method, "POST" | "PUT" | "PATCH" | "DELETE") || AUDITED_READS.contains(&path)
}

/// Middleware. Sits inside `require_employee`, so the actor is already known.
pub async fn record(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Result<Response, std::convert::Infallible> {
    let method = req.method().as_str().to_string();
    let path = req.uri().path().to_string();

    if !should_audit(&method, &path) {
        return Ok(next.run(req).await);
    }

    let actor = req.extensions().get::<AuthedEmployee>().cloned();

    let ip = header(&req, "x-forwarded-for")
        // Caddy appends; the client is the first entry.
        .map(|v| v.split(',').next().unwrap_or("").trim().to_string())
        .filter(|s| !s.is_empty());
    let user_agent = header(&req, "user-agent");

    // The body has to be buffered to be read, then handed on to the handler.
    // Skipped entirely when it is too large, rather than risking the request
    // itself — an audit log that breaks the app it is auditing gets turned off.
    let too_big = req
        .headers()
        .get(axum::http::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok())
        .is_some_and(|len| len > MAX_BODY_CAPTURE);

    let (req, detail) = if too_big {
        (req, Some(json!({ "note": "body too large to record" })))
    } else {
        let (parts, body) = req.into_parts();
        match axum::body::to_bytes(body, MAX_BODY_CAPTURE).await {
            Ok(bytes) => {
                let detail = parse_detail(&bytes);
                (Request::from_parts(parts, Body::from(bytes)), detail)
            }
            Err(_) => (
                Request::from_parts(parts, Body::empty()),
                Some(json!({ "note": "body could not be read" })),
            ),
        }
    };

    let response = next.run(req).await;
    let status = response.status().as_u16() as i32;

    // Written after the fact so the recorded outcome is the real one. A log of
    // attempts that all read as successes is worse than none: it would show a
    // rejected deletion as a deletion.
    let summary = summarise(&method, &path, detail.as_ref());
    let (actor_id, actor_email, actor_role) = match actor {
        Some(a) => (
            Some(a.id),
            a.email,
            match a.role {
                EmployeeRole::Admin => "admin",
                EmployeeRole::Worker => "worker",
            },
        ),
        // The layering means this should not happen; recording it as `unknown`
        // rather than dropping the entry means a hole in the auth chain shows up
        // in the log instead of hiding in it.
        None => (None, "unknown".to_string(), "unknown"),
    };

    let result = sqlx::query(
        "insert into admin_audit_log \
           (actor_id, actor_email, actor_role, method, path, status, ip, user_agent, summary, detail) \
         values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(actor_id)
    .bind(&actor_email)
    .bind(actor_role)
    .bind(&method)
    .bind(&path)
    .bind(status)
    .bind(ip.as_deref())
    .bind(user_agent.as_deref())
    .bind(&summary)
    .bind(detail.as_ref())
    .execute(&state.db)
    .await;

    if let Err(e) = result {
        // Never fail the request over this. The alternative — a failing audit
        // insert taking down intake — is how audit logging gets removed
        // entirely. It is loud in the log instead.
        tracing::error!(
            actor = %actor_email, %method, %path,
            "AUDIT WRITE FAILED — this action was not recorded: {e}"
        );
    }

    Ok(response)
}

fn header(req: &Request, name: &str) -> Option<String> {
    req.headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.chars().take(400).collect())
}

fn parse_detail(bytes: &Bytes) -> Option<Value> {
    if bytes.is_empty() {
        return None;
    }
    match serde_json::from_slice::<Value>(bytes) {
        Ok(value) => Some(redact(&value)),
        // A non-JSON body is not something this app sends; record that it was
        // there without guessing at its contents.
        Err(_) => Some(json!({ "note": "non-JSON body", "bytes": bytes.len() })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn secrets_are_stripped_before_storage() {
        let body = json!({
            "email": "someone@theblendbarokc.com",
            "password": "hunter2",
            "new_password": "hunter3",
            "passphrase": "the backup key",
            "webhook_url": "https://discord.com/api/webhooks/123/abcdef",
            "label": "Front desk",
        });
        let out = redact(&body);

        assert_eq!(out["password"], REDACTED);
        assert_eq!(out["new_password"], REDACTED);
        assert_eq!(out["passphrase"], REDACTED);
        assert_eq!(out["webhook_url"], REDACTED);
        // Non-secret context survives — an entry with everything redacted tells
        // you nothing about what happened.
        assert_eq!(out["email"], "someone@theblendbarokc.com");
        assert_eq!(out["label"], "Front desk");
    }

    #[test]
    fn nested_and_arrayed_secrets_are_stripped_too() {
        // The service-account upload arrives as a nested object; a top-level-only
        // pass would write a private key into an append-only table.
        let body = json!({
            "google": { "service_account_json": { "private_key": "-----BEGIN…" } },
            "targets": [{ "webhook_url": "https://x/y" }, { "label": "ok" }],
        });
        let out = redact(&body);
        assert_eq!(out["google"]["service_account_json"], REDACTED);
        assert_eq!(out["targets"][0]["webhook_url"], REDACTED);
        assert_eq!(out["targets"][1]["label"], "ok");
    }

    #[test]
    fn mfa_codes_and_tokens_are_secrets() {
        let out = redact(&json!({ "code": "123456", "token": "abc", "totp_secret": "xyz" }));
        assert_eq!(out["code"], REDACTED);
        assert_eq!(out["token"], REDACTED);
        assert_eq!(out["totp_secret"], REDACTED);
    }

    #[test]
    fn mutations_are_audited_and_ordinary_reads_are_not() {
        assert!(should_audit("POST", "/api/intake"));
        assert!(should_audit("PATCH", "/api/settings"));
        assert!(should_audit("DELETE", "/api/admin/backup/destinations/1"));
        // Noise: thousands of these would bury the entries that matter.
        assert!(!should_audit("GET", "/api/customers"));
        assert!(!should_audit("GET", "/api/orders"));
    }

    #[test]
    fn downloading_the_database_is_audited_even_though_it_is_a_get() {
        // A full PII export that otherwise leaves no trace anywhere.
        assert!(should_audit("GET", "/api/admin/backup"));
    }

    #[test]
    fn summaries_read_like_sentences() {
        assert_eq!(
            summarise("POST", "/api/employees", Some(&json!({ "email": "a@b.com" }))),
            "created employees “a@b.com”"
        );
        assert_eq!(
            summarise("DELETE", "/api/customers/123", None),
            "deleted customers"
        );
        assert_eq!(
            summarise("GET", "/api/admin/backup", None),
            "accessed admin"
        );
    }

    #[test]
    fn a_redacted_field_never_leaks_through_the_summary() {
        // The summary picks a label out of the body; it must pick from the
        // redacted copy, never the raw one.
        let raw = json!({ "name": "Nightly", "passphrase": "super-secret" });
        let safe = redact(&raw);
        let line = summarise("POST", "/api/admin/backup/destinations", Some(&safe));
        assert!(!line.contains("super-secret"));
        assert!(line.contains("Nightly"));
    }
}

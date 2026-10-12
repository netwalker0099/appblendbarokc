//! Connect Square (Admin → Billing). The flow and where the token lives are
//! described in `square::oauth`; these are its HTTP ends.

use axum::extract::{Query, State};
use axum::response::Redirect;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::employee_auth::AdminEmployee;
use crate::error::AppError;
use crate::AppState;

/// Where the callback sends the browser afterwards: back to the Billing tab,
/// which reads the outcome from the query string.
const BILLING_TAB: &str = "/admin?tab=billing";

/// Start Connect Square. Returns Square's approval URL for the browser to open.
pub async fn start(
    AdminEmployee(admin): AdminEmployee,
    State(state): State<AppState>,
) -> Result<Json<Value>, AppError> {
    let url = state
        .square_connection
        .begin(admin.id)
        .map_err(AppError::Conflict)?;
    Ok(Json(json!({ "url": url })))
}

#[derive(Deserialize)]
pub struct CallbackParams {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

/// Square sends the browser here after the owner approves (or declines).
///
/// Behind the employee session like every operator route — the session cookie
/// is `SameSite=Lax`, so it comes along on this top-level redirect from Square —
/// and additionally bound to the admin who pressed the button by the one-time
/// `state`. Always answers with a redirect to the Billing tab, never a JSON
/// error page in the middle of the flow.
pub async fn callback(
    AdminEmployee(admin): AdminEmployee,
    State(state): State<AppState>,
    Query(params): Query<CallbackParams>,
) -> Redirect {
    let outcome = match (&params.code, &params.state, &params.error) {
        (_, _, Some(error)) => {
            // `access_denied` is the owner pressing Deny on Square's page.
            let why = params.error_description.as_deref().unwrap_or(error);
            tracing::warn!(%error, "square: Connect Square declined or failed at Square");
            Err(format!("Square did not connect: {why}"))
        }
        (Some(code), Some(st), None) => state.square_connection.complete(admin.id, st, code).await,
        _ => Err("Square sent back an incomplete answer — press Connect Square again".into()),
    };

    match outcome {
        Ok(()) => Redirect::to(&format!("{BILLING_TAB}&square=connected")),
        Err(msg) => {
            tracing::warn!("square: Connect Square failed: {msg}");
            Redirect::to(&format!(
                "{BILLING_TAB}&square_error={}",
                encode_query_value(&msg)
            ))
        }
    }
}

/// Renew the token now rather than waiting for the weekly renewal — to check
/// that renewal works, or after fixing whatever made it fail.
pub async fn refresh(
    _admin: AdminEmployee,
    State(state): State<AppState>,
) -> Result<Json<Value>, AppError> {
    state
        .square_connection
        .refresh(true)
        .await
        .map_err(AppError::Unavailable)?;
    Ok(Json(state.square_connection.status()))
}

/// Revoke the token at Square and forget it. Payments stop until reconnected.
pub async fn disconnect(
    _admin: AdminEmployee,
    State(state): State<AppState>,
) -> Result<Json<Value>, AppError> {
    state
        .square_connection
        .disconnect()
        .await
        .map_err(AppError::Conflict)?;
    Ok(Json(state.square_connection.status()))
}

fn encode_query_value(s: &str) -> String {
    reqwest::Url::parse_with_params("http://x/", &[("v", s)])
        .ok()
        .and_then(|u| u.query().map(|q| q.trim_start_matches("v=").to_string()))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_messages_survive_the_query_string() {
        let encoded = encode_query_value("Square answered 400: bad & worse — 100%");
        assert!(!encoded.contains(' ') && !encoded.contains('&'));
        let back = reqwest::Url::parse(&format!("http://x/?v={encoded}")).unwrap();
        let (_, v) = back.query_pairs().next().unwrap();
        assert_eq!(v, "Square answered 400: bad & worse — 100%");
    }
}

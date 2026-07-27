//! Customer-facing portal endpoints (open router; self-managed customer session
//! cookie). Passwordless: request a magic link by email, consume it to get a
//! session, then view saved blends and place a staff-fulfilled reorder.

use std::collections::HashMap;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{Duration, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::customer_auth as ca;
use crate::employee_auth::{generate_session_token, hash_token, json_with_cookie};
use crate::error::AppError;
use crate::models::customer::Customer;
use crate::models::mix::{Mix, MixItem};
use crate::models::order::{BottleSize, Order, OrderStatus, OrderType};
use crate::routes::mixes::MixDetail;
use crate::routes::scents::{fetch_scent_detail, ScentDetail};
use crate::AppState;

fn site_url() -> String {
    std::env::var("CUSTOMER_SITE_URL")
        .unwrap_or_else(|_| "https://sandbox.theblendbarokc.com".to_string())
}

#[derive(Deserialize)]
pub struct RequestLinkBody {
    pub email: String,
}

/// Email a single-use sign-in link — but only if a customer with that email
/// exists. Always returns the same generic response so the endpoint can't be used
/// to discover which emails are registered.
pub async fn request_link(
    State(state): State<AppState>,
    Json(body): Json<RequestLinkBody>,
) -> Result<Response, AppError> {
    let email = body.email.trim().to_lowercase();

    // ⚠️ TEMPORARY DEV BYPASS (remove when real email is wired — see RESUME).
    // A single whitelisted email (PORTAL_BYPASS_EMAIL) signs straight into the
    // portal without a magic link, so the owner can preview the customer page.
    let bypass = std::env::var("PORTAL_BYPASS_EMAIL")
        .ok()
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty());
    if bypass.as_deref() == Some(email.as_str()) {
        let customer = sqlx::query_as::<_, Customer>(
            "insert into customers (email) values ($1) on conflict (email) do update set email = excluded.email returning *",
        )
        .bind(&email)
        .fetch_one(&state.db)
        .await?;
        let token = generate_session_token();
        let expires = Utc::now() + Duration::days(ca::SESSION_TTL_DAYS);
        sqlx::query(
            "insert into customer_sessions (token_hash, customer_id, expires_at) values ($1, $2, $3)",
        )
        .bind(hash_token(&token))
        .bind(customer.id)
        .bind(expires)
        .execute(&state.db)
        .await?;
        tracing::warn!(email = %email, "PORTAL DEV BYPASS used — remove before launch");
        return Ok(json_with_cookie(
            ca::set_cookie(&token),
            json!({ "status": "bypass" }),
        ));
    }

    if let Some(customer) =
        sqlx::query_as::<_, Customer>("select * from customers where email = $1")
            .bind(&email)
            .fetch_optional(&state.db)
            .await?
    {
        let token = generate_session_token();
        let expires = Utc::now() + Duration::minutes(ca::LOGIN_TTL_MINUTES);
        sqlx::query(
            "insert into customer_login_tokens (token_hash, customer_id, expires_at) values ($1, $2, $3)",
        )
        .bind(hash_token(&token))
        .bind(customer.id)
        .bind(expires)
        .execute(&state.db)
        .await?;

        let link = format!("{}/portal/verify?token={}", site_url(), token);

        // Sent inline, not queued: someone is watching a "check your email"
        // screen and the token expires in minutes. A failure is logged and
        // recorded, but never surfaced to the caller — the response is identical
        // whether or not the address exists, so this endpoint cannot be used to
        // discover who is a customer.
        if let Err(e) = crate::email::dispatch::send_magic_link(
            &state.db,
            state.mailer.as_ref(),
            customer.id,
            &email,
            &link,
            ca::LOGIN_TTL_MINUTES,
        )
        .await
        {
            tracing::error!(email = %email, "could not send sign-in link: {e}");
        }
    }
    Ok(Json(json!({ "status": "sent" })).into_response())
}

#[derive(Deserialize)]
pub struct VerifyBody {
    pub token: String,
}

/// Consume a magic-link token and open a customer session.
pub async fn verify(
    State(state): State<AppState>,
    Json(body): Json<VerifyBody>,
) -> Result<Response, AppError> {
    let token_hash = hash_token(body.token.trim());

    let mut tx = state.db.begin().await?;
    let customer_id: Option<Uuid> = sqlx::query_scalar(
        "update customer_login_tokens set used_at = now() \
         where token_hash = $1 and used_at is null and expires_at > now() \
         returning customer_id",
    )
    .bind(&token_hash)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(customer_id) = customer_id else {
        return Err(AppError::Unauthorized);
    };

    let session_token = generate_session_token();
    let expires = Utc::now() + Duration::days(ca::SESSION_TTL_DAYS);
    sqlx::query(
        "insert into customer_sessions (token_hash, customer_id, expires_at) values ($1, $2, $3)",
    )
    .bind(hash_token(&session_token))
    .bind(customer_id)
    .bind(expires)
    .execute(&mut *tx)
    .await?;

    let customer = sqlx::query_as::<_, Customer>("select * from customers where id = $1")
        .bind(customer_id)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;

    Ok(json_with_cookie(
        ca::set_cookie(&session_token),
        json!({ "email": customer.email, "name": customer.name }),
    ))
}

pub async fn me(State(state): State<AppState>, headers: HeaderMap) -> Result<Json<Value>, AppError> {
    let customer = ca::load_customer(&state.db, &headers)
        .await
        .ok_or(AppError::Unauthorized)?;
    Ok(Json(json!({ "email": customer.email, "name": customer.name })))
}

pub async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    if let Some(token) = ca::read_cookie(&headers) {
        sqlx::query("delete from customer_sessions where token_hash = $1")
            .bind(hash_token(&token))
            .execute(&state.db)
            .await?;
    }
    Ok(json_with_cookie(ca::clear_cookie(), json!({ "status": "ok" })))
}

/// The customer's reorderable history: their custom mixes (with items) and the
/// set-perfume scents they've ordered (with formulas).
pub async fn history(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, AppError> {
    let customer = ca::load_customer(&state.db, &headers)
        .await
        .ok_or(AppError::Unauthorized)?;

    let mixes = sqlx::query_as::<_, Mix>(
        "select * from mixes where customer_id = $1 order by created_at desc",
    )
    .bind(customer.id)
    .fetch_all(&state.db)
    .await?;
    let mix_ids: Vec<Uuid> = mixes.iter().map(|m| m.id).collect();
    let items = sqlx::query_as::<_, MixItem>("select * from mix_items where mix_id = any($1)")
        .bind(&mix_ids)
        .fetch_all(&state.db)
        .await?;
    let mut by_mix: HashMap<Uuid, Vec<MixItem>> = HashMap::new();
    for item in items {
        by_mix.entry(item.mix_id).or_default().push(item);
    }
    let mixes: Vec<MixDetail> = mixes
        .into_iter()
        .map(|mix| {
            let items = by_mix.remove(&mix.id).unwrap_or_default();
            MixDetail { mix, items }
        })
        .collect();

    let scent_ids: Vec<Uuid> = sqlx::query_scalar(
        "select distinct scent_id from orders where customer_id = $1 and type = 'set_perfume' and scent_id is not null",
    )
    .bind(customer.id)
    .fetch_all(&state.db)
    .await?;
    let mut scents = Vec::new();
    for id in scent_ids {
        if let Some(detail) = fetch_scent_detail(&state.db, id).await? {
            scents.push(detail);
        }
    }

    Ok(Json(json!({ "mixes": mixes, "scents": scents })))
}

#[derive(Deserialize)]
pub struct ReorderBody {
    #[serde(rename = "type")]
    pub order_type: OrderType,
    pub size: BottleSize,
    pub mix_id: Option<Uuid>,
    pub scent_id: Option<Uuid>,
}

/// Place a reorder — creates a `lead` order for staff to complete/charge in
/// person. The customer can only reorder their own mixes and scents they've had.
pub async fn reorder(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ReorderBody>,
) -> Result<Json<Order>, AppError> {
    let customer = ca::load_customer(&state.db, &headers)
        .await
        .ok_or(AppError::Unauthorized)?;

    match body.order_type {
        OrderType::CustomMix => {
            let mix_id = body
                .mix_id
                .ok_or_else(|| AppError::BadRequest("mix_id is required".into()))?;
            let owns: bool = sqlx::query_scalar(
                "select exists(select 1 from mixes where id = $1 and customer_id = $2)",
            )
            .bind(mix_id)
            .bind(customer.id)
            .fetch_one(&state.db)
            .await?;
            if !owns {
                return Err(AppError::NotFound("mix not found".into()));
            }
        }
        OrderType::SetPerfume => {
            let scent_id = body
                .scent_id
                .ok_or_else(|| AppError::BadRequest("scent_id is required".into()))?;
            // Must be a scent they've ordered before, and still active.
            let ok: bool = sqlx::query_scalar(
                "select exists(\
                   select 1 from orders o join scents s on s.id = o.scent_id \
                   where o.customer_id = $1 and o.scent_id = $2 and o.type = 'set_perfume' and s.active = true)",
            )
            .bind(customer.id)
            .bind(scent_id)
            .fetch_one(&state.db)
            .await?;
            if !ok {
                return Err(AppError::BadRequest(
                    "that scent isn't available to reorder".into(),
                ));
            }
        }
    }

    let order = sqlx::query_as::<_, Order>(
        "insert into orders (customer_id, type, size, mix_id, scent_id, status) \
         values ($1, $2, $3, $4, $5, $6) returning *",
    )
    .bind(customer.id)
    .bind(body.order_type)
    .bind(body.size)
    .bind(body.mix_id)
    .bind(body.scent_id)
    .bind(OrderStatus::Lead)
    .fetch_one(&state.db)
    .await?;

    // No downstream push: a reorder is a 'lead' until staff cart it and take
    // payment through Square.
    Ok(Json(order))
}

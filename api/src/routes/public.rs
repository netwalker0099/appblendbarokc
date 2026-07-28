//! Public (no-auth) share targets: a customer can share a scent link/QR with a
//! friend. Deliberately exposes ingredient NAMES only (the "notes") and prices —
//! never the ml amounts, which stay employee-only.

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap};
use axum::response::Response;
use axum::Json;
use qrcode::render::svg;
use qrcode::QrCode;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::AppError;
use crate::models::cart::Cart;
use crate::models::customer::Customer;
use crate::models::order::{BottleSize, Order, OrderStatus, OrderType};
use crate::models::scent::Scent;
use crate::models::sync::SyncEntity;
use crate::ratelimit::client_key;
use crate::square::money::{self, DEFAULT_CURRENCY};
use crate::square::{CheckoutPush, LineItemPush};
use crate::AppState;

fn site_url() -> String {
    std::env::var("CUSTOMER_SITE_URL")
        .unwrap_or_else(|_| "https://sandbox.theblendbarokc.com".to_string())
}

#[derive(Serialize)]
pub struct PublicScent {
    pub id: Uuid,
    pub name: String,
    /// Ingredient names only — no amounts.
    pub notes: Vec<String>,
    pub price_oz3_4: Option<Decimal>,
    pub price_oz1_7: Option<Decimal>,
    pub price_roller: Option<Decimal>,
    pub price_spray: Option<Decimal>,
}

pub async fn scent(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<PublicScent>, AppError> {
    let scent = sqlx::query_as::<_, Scent>("select * from scents where id = $1 and active = true")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound("scent not found".into()))?;

    let notes: Vec<String> = sqlx::query_scalar(
        "select i.name from scent_items si join ingredients i on i.id = si.ingredient_id \
         where si.scent_id = $1 order by i.name",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(PublicScent {
        id: scent.id,
        name: scent.name,
        notes,
        price_oz3_4: scent.price_oz3_4,
        price_oz1_7: scent.price_oz1_7,
        price_roller: scent.price_roller,
        price_spray: scent.price_spray,
    }))
}

// --- Public checkout -------------------------------------------------------

#[derive(Deserialize)]
pub struct PublicCheckoutRequest {
    pub scent_id: Uuid,
    pub size: BottleSize,
    pub email: String,
    pub name: Option<String>,
    /// The referral code from the share link (`/s/<id>?ref=CODE`).
    pub referral_code: Option<String>,
    /// A coupon the buyer already holds.
    pub coupon_code: Option<String>,
}

#[derive(Serialize)]
pub struct PublicCheckoutResponse {
    pub checkout_url: String,
    /// What came off, so the page can say so rather than leaving the customer to
    /// discover it on Square's screen.
    pub discount_cents: i64,
    pub total_cents: i64,
}

/// Buy a shared scent. **No authentication** — anyone with the share link.
///
/// The security posture here, since this is the only endpoint on the app that
/// lets an anonymous caller set money in motion:
///
/// - **The price is never taken from the request.** It is read from the scent row
///   for the requested size. A client can choose *what* to buy, never what to pay.
/// - **An existing customer row is never modified.** Buying with someone else's
///   email must not let a stranger rewrite their name or flip their marketing
///   consent, so an existing record is used exactly as-is.
/// - **No marketing opt-in.** A purchase is not consent; new records are created
///   with `marketing_consent = false`.
/// - **Rate limited per caller IP**, because this writes rows and calls Square.
/// - **Refused outright when Square is not live.** A staff member seeing a mock
///   link is an inconvenience; sending a paying customer to a dead URL is not
///   acceptable, so the endpoint 503s rather than hand back a fake checkout.
pub async fn checkout(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<PublicCheckoutRequest>,
) -> Result<Json<PublicCheckoutResponse>, AppError> {
    if !state.public_checkout_limiter.check(&client_key(&headers)) {
        return Err(AppError::TooManyRequests(
            "too many checkout attempts — please wait a minute and try again".into(),
        ));
    }

    let email = body.email.trim().to_lowercase();
    if email.len() > 254 || !email.contains('@') || email.starts_with('@') || email.ends_with('@') {
        return Err(AppError::BadRequest("a valid email is required".into()));
    }
    // Only ever used to create a new record; still bounded so a long string can't
    // be stuffed through into Square's line-item note.
    let name = body
        .name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.chars().take(100).collect::<String>());

    // Active scents only — the same rule the public view enforces.
    let scent = sqlx::query_as::<_, Scent>("select * from scents where id = $1 and active = true")
        .bind(body.scent_id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound("scent not found".into()))?;

    // The price comes from the catalogue, keyed on the requested size. This is
    // the line that makes the endpoint safe to expose — the request cannot
    // influence it.
    let price =
        crate::pricing::catalog_price(&state.db, OrderType::SetPerfume, body.size, Some(scent.id))
            .await?
            .ok_or_else(|| {
                AppError::BadRequest("that size is not available for this scent".into())
            })?;

    let cents = money::to_cents(price)
        .ok_or_else(|| AppError::Internal(format!("scent {} has an invalid price", scent.id)))?;
    if cents == 0 {
        return Err(AppError::BadRequest(
            "that size is not available for this scent".into(),
        ));
    }

    // Never send a real customer to a mock payment link.
    //
    // Checked here, after the request has been validated but before the first
    // write, for two reasons: a malformed request should hear that it is
    // malformed rather than that the service is down, and nothing should be
    // persisted for a checkout that cannot proceed. It also means the validation
    // above is genuinely exercisable while the app is still on the mock.
    if !state.square.is_live() {
        tracing::warn!("public checkout attempted while Square is not configured");
        return Err(AppError::Unavailable(
            "online checkout is not available right now".into(),
        ));
    }

    let mut tx = state.db.begin().await?;

    // Create-if-absent. Note the `do update set email = customers.email` no-op:
    // it makes the row visible to RETURNING without changing a single field of an
    // existing customer's record.
    let customer = sqlx::query_as::<_, Customer>(
        r#"
        insert into customers (email, name, marketing_consent)
        values ($1, $2, false)
        on conflict (email) do update set email = customers.email
        returning *
        "#,
    )
    .bind(&email)
    .bind(&name)
    .fetch_one(&mut *tx)
    .await?;

    let order = sqlx::query_as::<_, Order>(
        r#"
        insert into orders (customer_id, type, size, scent_id, status, amount, external_ref)
        values ($1, $2, $3, $4, $5, $6, 'public_share')
        returning *
        "#,
    )
    .bind(customer.id)
    .bind(OrderType::SetPerfume)
    .bind(body.size)
    .bind(scent.id)
    .bind(OrderStatus::Lead)
    .bind(price)
    .fetch_one(&mut *tx)
    .await?;

    let line_name = format!("{} ({})", scent.name, body.size.label());

    // Work out the reduction now that we know who the buyer is — self-referral
    // and already-rewarded pairs are both worth nothing.
    let (referral_discount, _referrer) = crate::referrals::discount_for(
        &state.db,
        body.referral_code.as_deref(),
        customer.id,
    )
    .await?;

    // A coupon is personal: it only counts if it belongs to this buyer.
    let coupon = match body.coupon_code.as_deref() {
        Some(code) if !code.trim().is_empty() => {
            crate::referrals::find_coupon(&state.db, code, Some(customer.id)).await?
        }
        _ => None,
    };
    let coupon_amount = coupon.as_ref().map(|c| c.amount_cents).unwrap_or(0);

    // Clamped so the order can never go negative — Square rejects that, and a
    // checkout that owes the customer money is not a reachable state.
    let (total_cents, discount_cents) =
        crate::referrals::apply_discount(cents, referral_discount + coupon_amount);

    let cart = sqlx::query_as::<_, Cart>(
        r#"
        insert into carts (customer_id, currency, total_cents, idempotency_key, note,
                           discount_cents, coupon_id, referral_code)
        values ($1, $2, $3, $4, $5, $6, $7, $8)
        returning *
        "#,
    )
    .bind(customer.id)
    .bind(DEFAULT_CURRENCY)
    .bind(total_cents)
    .bind(Uuid::new_v4().to_string())
    // Staff need to know this one arrived from a share link and has to be made
    // up, rather than being handed over at the bar.
    .bind("Online order from a share link — to be crafted by staff")
    .bind(discount_cents)
    .bind(coupon.as_ref().map(|c| c.id))
    .bind(body.referral_code.as_deref().map(|c| c.trim().to_uppercase()))
    .fetch_one(&mut *tx)
    .await?;

    // Hold the coupon against this cart straight away. Conditional on it still
    // being active, so two tabs cannot spend the same coupon twice; if it lost
    // the race the cart simply proceeds without it rather than failing.
    if let Some(c) = &coupon {
        if !crate::referrals::redeem_coupon(&mut tx, c.id, cart.id).await? {
            tracing::warn!(cart_id = %cart.id, "coupon was spent elsewhere first");
        }
    }

    sqlx::query(
        "insert into cart_items (cart_id, order_id, name, quantity, unit_amount_cents, kind) \
         values ($1, $2, $3, 1, $4, 'blend')",
    )
    .bind(cart.id)
    .bind(order.id)
    .bind(&line_name)
    .bind(cents)
    .execute(&mut *tx)
    .await?;

    // Same outbox the operator path uses, so an online buyer reaches Square
    // Customers too.
    crate::sync::enqueue(&mut *tx, SyncEntity::Contact, customer.id).await?;

    tx.commit().await?;

    let push = CheckoutPush {
        cart_id: cart.id,
        idempotency_key: cart.idempotency_key.clone(),
        currency: cart.currency.clone(),
        buyer_email: Some(customer.email.clone()),
        line_items: vec![LineItemPush {
            name: line_name,
            quantity: 1,
            unit_amount_cents: cents,
        }],
        discounts: if discount_cents > 0 {
            vec![crate::square::DiscountPush {
                name: if coupon.is_some() { "Coupon" } else { "Referral discount" }.to_string(),
                amount_cents: discount_cents,
            }]
        } else {
            Vec::new()
        },
        redirect_url: Some(format!("{}/thanks", site_url())),
        note: cart.note.clone(),
    };

    let handle = state.square.create_checkout(&push).await.map_err(|e| {
        tracing::error!(cart_id = %cart.id, "public checkout failed: {e}");
        // Never surface Square's error text to an anonymous caller; it can carry
        // account and configuration detail.
        AppError::Unavailable("could not start checkout — please try again".into())
    })?;

    sqlx::query(
        r#"
        update carts set
            status = 'pending_payment',
            square_order_id = $2,
            square_payment_link_id = $3,
            checkout_url = $4,
            checkout_at = now(),
            updated_at = now()
        where id = $1
        "#,
    )
    .bind(cart.id)
    .bind(&handle.square_order_id)
    .bind(&handle.payment_link_id)
    .bind(&handle.url)
    .execute(&state.db)
    .await?;

    tracing::info!(
        cart_id = %cart.id,
        scent = %scent.name,
        total = %money::format_cents(total_cents, DEFAULT_CURRENCY),
        discount = %money::format_cents(discount_cents, DEFAULT_CURRENCY),
        "public share checkout created"
    );

    Ok(Json(PublicCheckoutResponse {
        checkout_url: handle.url,
        discount_cents,
        total_cents,
    }))
}

/// An SVG QR code that points at the scent's public share page.
pub async fn scent_qr(Path(id): Path<Uuid>) -> Result<Response, AppError> {
    let url = format!("{}/s/{}", site_url(), id);
    let code = QrCode::new(url.as_bytes())
        .map_err(|e| AppError::Internal(format!("qr encode failed: {e}")))?;
    let image = code
        .render::<svg::Color>()
        .min_dimensions(220, 220)
        .quiet_zone(true)
        .build();

    Response::builder()
        .header(header::CONTENT_TYPE, "image/svg+xml")
        .header(header::CACHE_CONTROL, "public, max-age=3600")
        .body(Body::from(image))
        .map_err(|e| AppError::Internal(e.to_string()))
}

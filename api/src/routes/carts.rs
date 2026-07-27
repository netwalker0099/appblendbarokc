//! Carts and checkout.
//!
//! The flow this implements:
//!
//! 1. Intake creates orders as it always has (status `lead`, no money moved).
//! 2. `POST /api/carts` gathers one or more of those orders — plus any ad-hoc
//!    lines like an event deposit — into a cart.
//! 3. `POST /api/carts/:id/checkout` pushes the cart to Square and gets back a
//!    hosted payment link. **This is the only place money is set in motion, and
//!    it happens entirely on Square's side.** No card data touches this box.
//! 4. Square tells us the outcome by webhook; `POST /api/carts/:id/refresh`
//!    is the pull-based backstop for when it doesn't.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Extension;
use axum::Json;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::billing;
use crate::employee_auth::AuthedEmployee;
use crate::error::AppError;
use crate::models::cart::{Cart, CartDetail, CartItem, CartStatus};
use crate::models::customer::Customer;
use crate::models::order::Order;
use crate::square::money::{self, DEFAULT_CURRENCY};
use crate::square::{CheckoutPush, LineItemPush};
use crate::AppState;

/// Cap on lines per cart. Square accepts far more; this is a guard against a
/// runaway client, not a business rule.
const MAX_ITEMS: usize = 50;

// --- Create -----------------------------------------------------------------

/// An ad-hoc line: money that isn't a bottle. Event deposits, rush fees, and the
/// multi-day hotel line from the booking terms all arrive this way.
#[derive(Deserialize)]
pub struct AdHocItemInput {
    pub name: String,
    #[serde(default = "one")]
    pub quantity: i32,
    /// Entered as a decimal (what the operator types); converted to cents here.
    pub unit_amount: Decimal,
    /// What this line *is*: `event_deposit`, `fee`, or `other`. Explicit rather
    /// than inferred from the label, because a deposit settling is what triggers
    /// the "event booked" notification and matching on free text would break the
    /// first time someone retyped it.
    #[serde(default = "other_kind")]
    pub kind: String,
}

fn other_kind() -> String {
    "other".to_string()
}

/// Kinds an operator may set on an ad-hoc line. `blend` is excluded: that kind
/// belongs to lines carrying an `order_id`, and is set by this module, not by a
/// client.
const AD_HOC_KINDS: [&str; 3] = ["event_deposit", "fee", "other"];

fn one() -> i32 {
    1
}

#[derive(Deserialize)]
pub struct CreateCartRequest {
    pub customer_id: Uuid,
    /// Existing orders to sell. Each becomes a line priced from `orders.amount`.
    #[serde(default)]
    pub order_ids: Vec<Uuid>,
    #[serde(default)]
    pub items: Vec<AdHocItemInput>,
    pub note: Option<String>,
}

pub async fn create(
    Extension(employee): Extension<AuthedEmployee>,
    State(state): State<AppState>,
    Json(body): Json<CreateCartRequest>,
) -> Result<(StatusCode, Json<CartDetail>), AppError> {
    if body.order_ids.is_empty() && body.items.is_empty() {
        return Err(AppError::BadRequest("a cart needs at least one line".into()));
    }
    if body.order_ids.len() + body.items.len() > MAX_ITEMS {
        return Err(AppError::BadRequest(format!(
            "a cart is limited to {MAX_ITEMS} lines"
        )));
    }

    let customer = sqlx::query_as::<_, Customer>("select * from customers where id = $1")
        .bind(body.customer_id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound("customer not found".into()))?;

    // Build every line before opening the transaction, so a bad request fails
    // cleanly instead of half-writing a cart.
    // (order_id, name, quantity, unit cents, kind)
    let mut lines: Vec<(Option<Uuid>, String, i32, i64, &str)> = Vec::new();

    for order_id in &body.order_ids {
        let order = sqlx::query_as::<_, Order>("select * from orders where id = $1")
            .bind(order_id)
            .fetch_optional(&state.db)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("order {order_id} not found")))?;

        if order.customer_id != customer.id {
            return Err(AppError::BadRequest(format!(
                "order {order_id} belongs to a different customer"
            )));
        }

        let amount = order.amount.ok_or_else(|| {
            AppError::BadRequest(format!(
                "order {order_id} has no amount — price it before selling it"
            ))
        })?;
        let cents = money::to_cents(amount).ok_or_else(|| {
            AppError::BadRequest(format!("order {order_id} has an invalid amount: {amount}"))
        })?;

        lines.push((
            Some(order.id),
            format!("{} ({})", order.order_type.label(), order.size.label()),
            1,
            cents,
            "blend",
        ));
    }

    for item in &body.items {
        let name = item.name.trim();
        if name.is_empty() {
            return Err(AppError::BadRequest("line name is required".into()));
        }
        if item.quantity <= 0 {
            return Err(AppError::BadRequest("quantity must be positive".into()));
        }
        let cents = money::to_cents(item.unit_amount).ok_or_else(|| {
            AppError::BadRequest(format!("invalid amount for \"{name}\": {}", item.unit_amount))
        })?;

        let kind = AD_HOC_KINDS
            .iter()
            .find(|k| **k == item.kind)
            .copied()
            .ok_or_else(|| {
                AppError::BadRequest(format!(
                    "unknown line kind '{}' — expected one of {}",
                    item.kind,
                    AD_HOC_KINDS.join(", ")
                ))
            })?;

        lines.push((None, name.to_string(), item.quantity, cents, kind));
    }

    let total: i64 = lines.iter().map(|(_, _, q, c, _)| c * *q as i64).sum();

    let mut tx = state.db.begin().await?;

    let cart = sqlx::query_as::<_, Cart>(
        r#"
        insert into carts (customer_id, currency, total_cents, idempotency_key, note, created_by)
        values ($1, $2, $3, $4, $5, $6)
        returning *
        "#,
    )
    .bind(customer.id)
    .bind(DEFAULT_CURRENCY)
    .bind(total)
    // Generated once and reused for the life of the cart, so a retried checkout
    // resolves to the same Square payment link rather than a second one.
    .bind(Uuid::new_v4().to_string())
    .bind(&body.note)
    .bind(employee.id)
    .fetch_one(&mut *tx)
    .await?;

    let mut items = Vec::with_capacity(lines.len());
    for (order_id, name, quantity, unit_amount_cents, kind) in lines {
        let item = sqlx::query_as::<_, CartItem>(
            r#"
            insert into cart_items (cart_id, order_id, name, quantity, unit_amount_cents, kind)
            values ($1, $2, $3, $4, $5, $6)
            returning *
            "#,
        )
        .bind(cart.id)
        .bind(order_id)
        .bind(&name)
        .bind(quantity)
        .bind(unit_amount_cents)
        .bind(kind)
        .fetch_one(&mut *tx)
        .await
        // The unique index on cart_items.order_id is the double-billing guard.
        // Hitting it means another cart already claims this blend — a real
        // conflict the operator can act on, not a server fault. Most likely two
        // staff carted the same customer at once.
        .map_err(|e| match &e {
            sqlx::Error::Database(db) if db.is_unique_violation() => AppError::Conflict(
                format!("\"{name}\" is already on another cart; cancel that cart or refresh this screen"),
            ),
            _ => AppError::from(e),
        })?;
        items.push(item);
    }

    tx.commit().await?;

    Ok((StatusCode::CREATED, Json(CartDetail { cart, items })))
}

// --- Read -------------------------------------------------------------------

#[derive(Deserialize)]
pub struct ListCartsQuery {
    pub status: Option<String>,
    pub customer_id: Option<Uuid>,
}

pub async fn list(
    State(state): State<AppState>,
    Query(q): Query<ListCartsQuery>,
) -> Result<Json<Vec<Cart>>, AppError> {
    let carts = sqlx::query_as::<_, Cart>(
        r#"
        select * from carts
        where ($1::text is null or status = $1)
          and ($2::uuid is null or customer_id = $2)
        order by created_at desc
        limit 100
        "#,
    )
    .bind(q.status)
    .bind(q.customer_id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(carts))
}

pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<CartDetail>, AppError> {
    Ok(Json(load_cart(&state, id).await?))
}

async fn load_cart(state: &AppState, id: Uuid) -> Result<CartDetail, AppError> {
    let cart = sqlx::query_as::<_, Cart>("select * from carts where id = $1")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound("cart not found".into()))?;

    let items = sqlx::query_as::<_, CartItem>(
        "select * from cart_items where cart_id = $1 order by created_at",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;

    Ok(CartDetail { cart, items })
}

// --- Checkout ---------------------------------------------------------------

#[derive(Serialize)]
pub struct CheckoutResponse {
    pub cart_id: Uuid,
    pub checkout_url: String,
    pub square_order_id: String,
    pub total_cents: i64,
    pub currency: String,
    /// False when the mock backend produced this — the URL is not payable and
    /// the UI must say so rather than showing a customer a dead link.
    pub live: bool,
}

pub async fn checkout(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<CheckoutResponse>, AppError> {
    let CartDetail { cart, items } = load_cart(&state, id).await?;

    // Already sent: hand back the same link. Checkout is safe to press twice.
    if let (Some(url), Some(order_id)) = (&cart.checkout_url, &cart.square_order_id) {
        if cart.status == CartStatus::PendingPayment {
            return Ok(Json(CheckoutResponse {
                cart_id: cart.id,
                checkout_url: url.clone(),
                square_order_id: order_id.clone(),
                total_cents: cart.total_cents,
                currency: cart.currency,
                live: state.square.is_live(),
            }));
        }
    }

    if !cart.status.is_open() {
        return Err(AppError::Conflict(format!(
            "cart is {:?} and cannot be checked out again",
            cart.status
        )));
    }
    if items.is_empty() {
        return Err(AppError::BadRequest("cart is empty".into()));
    }

    // The stored total and the lines are written in one transaction, so they
    // should never disagree — but this is the last point before a customer is
    // charged, and "should never" is not a guarantee worth betting money on.
    // Charge the sum of what's actually on the cart, or refuse.
    let line_sum: i64 = items.iter().map(|i| i.line_total_cents()).sum();
    if line_sum != cart.total_cents {
        tracing::error!(
            cart_id = %cart.id,
            stored = cart.total_cents,
            line_sum,
            "cart total disagrees with its line items — refusing to check out"
        );
        return Err(AppError::Conflict(
            "cart total does not match its line items; rebuild the cart".into(),
        ));
    }

    let customer = sqlx::query_as::<_, Customer>("select * from customers where id = $1")
        .bind(cart.customer_id)
        .fetch_one(&state.db)
        .await?;

    let push = CheckoutPush {
        cart_id: cart.id,
        idempotency_key: cart.idempotency_key.clone(),
        currency: cart.currency.clone(),
        buyer_email: Some(customer.email.clone()),
        line_items: items
            .iter()
            .map(|i| LineItemPush {
                name: i.name.clone(),
                quantity: i.quantity,
                unit_amount_cents: i.unit_amount_cents,
            })
            .collect(),
        redirect_url: None,
        note: cart.note.clone(),
    };

    let handle = state.square.create_checkout(&push).await.map_err(|e| {
        tracing::error!(cart_id = %cart.id, "square checkout failed: {e}");
        match e.retryable() {
            // Transient: tell the operator to try again, don't burn the cart.
            true => AppError::Unavailable(format!("Square is unavailable: {e}")),
            false => AppError::BadRequest(format!("Square rejected the checkout: {e}")),
        }
    })?;

    // Only now does the cart leave 'open'. If the write below fails, the cart
    // stays open and the reused idempotency key makes a retry return this same
    // Square order rather than creating a second one.
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
        square_order_id = %handle.square_order_id,
        total = %money::format_cents(cart.total_cents, &cart.currency),
        "checkout created"
    );

    Ok(Json(CheckoutResponse {
        cart_id: cart.id,
        checkout_url: handle.url,
        square_order_id: handle.square_order_id,
        total_cents: cart.total_cents,
        currency: cart.currency,
        live: state.square.is_live(),
    }))
}

// --- Refresh / cancel -------------------------------------------------------

#[derive(Serialize)]
pub struct RefreshResponse {
    pub cart_id: Uuid,
    pub status: CartStatus,
    /// True when Square had a payment for this cart.
    pub found: bool,
    pub detail: String,
}

/// Pull this cart's state from Square.
///
/// The webhook is the primary path; this is the backstop for when it doesn't
/// arrive — endpoint down during a deploy, signature key rotated mid-flight, or
/// webhooks simply not configured yet. Without it a paid cart would sit at
/// `pending_payment` forever while the customer holds a Square receipt.
pub async fn refresh(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<RefreshResponse>, AppError> {
    let cart = sqlx::query_as::<_, Cart>("select * from carts where id = $1")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound("cart not found".into()))?;

    let Some(square_order_id) = cart.square_order_id.clone() else {
        return Err(AppError::BadRequest(
            "cart has not been sent to Square yet".into(),
        ));
    };

    let payment = state
        .square
        .find_payment_for_order(&square_order_id)
        .await
        .map_err(|e| AppError::Unavailable(format!("could not reach Square: {e}")))?;

    let Some(payment) = payment else {
        return Ok(Json(RefreshResponse {
            cart_id: cart.id,
            status: cart.status,
            found: false,
            detail: "Square has no payment for this cart yet.".into(),
        }));
    };

    billing::apply_payment(&state.db, &payment).await?;

    let updated = sqlx::query_as::<_, Cart>("select * from carts where id = $1")
        .bind(id)
        .fetch_one(&state.db)
        .await?;

    Ok(Json(RefreshResponse {
        cart_id: updated.id,
        status: updated.status,
        found: true,
        detail: format!(
            "Square reports {} ({}).",
            payment.status,
            money::format_cents(payment.amount_cents, &payment.currency)
        ),
    }))
}

/// The checkout link as a QR code.
///
/// This is how a stand actually takes payment: the operator holds up the tablet,
/// the customer scans and pays on their own phone. It keeps the card entirely on
/// the customer's device and Square's page — nothing sensitive crosses the
/// tablet, this server, or the shop's network.
pub async fn checkout_qr(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<axum::response::Response, AppError> {
    use axum::body::Body;
    use axum::http::header;
    use qrcode::render::svg;
    use qrcode::QrCode;

    let url: Option<String> = sqlx::query_scalar("select checkout_url from carts where id = $1")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound("cart not found".into()))?;

    let url = url.ok_or_else(|| {
        AppError::BadRequest("cart has no checkout link yet — check it out first".into())
    })?;

    let code = QrCode::new(url.as_bytes())
        .map_err(|e| AppError::Internal(format!("qr encode failed: {e}")))?;
    let image = code
        .render::<svg::Color>()
        .min_dimensions(260, 260)
        .quiet_zone(true)
        .build();

    axum::response::Response::builder()
        .header(header::CONTENT_TYPE, "image/svg+xml")
        // A checkout link is single-use and short-lived; never let a proxy or
        // browser serve a stale one to the next customer.
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(image))
        .map_err(|e| AppError::Internal(e.to_string()))
}

pub async fn cancel(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<CartDetail>, AppError> {
    let canceled = billing::cancel_cart(&state.db, state.square.as_ref(), id).await?;
    if !canceled {
        return Err(AppError::Conflict(
            "only an open or awaiting-payment cart can be canceled".into(),
        ));
    }
    Ok(Json(load_cart(&state, id).await?))
}

//! In-person payment on a Square Terminal, through Square's cloud Terminal API.
//!
//! ## How a sale goes
//!
//! 1. Intake saves the orders and builds a cart (`POST /api/carts`).
//! 2. "Charge on Terminal" (`POST /api/carts/:id/terminal`) creates a Square
//!    order for the cart and sends a checkout for its total to the paired
//!    Terminal. Square relays it over the internet; nothing is sent to the
//!    browser or over Bluetooth, which is why this works the same from a laptop
//!    or an iPad.
//! 3. The customer taps or inserts on the Terminal. Square records a payment
//!    against the order.
//! 4. The cart flips to paid by whichever arrives first: the `payment.updated`
//!    or `terminal.checkout.updated` webhook, or the screen polling
//!    `GET /api/carts/:id/terminal` every couple of seconds. All three end in
//!    the same idempotent [`billing::apply_payment`], so it doesn't matter which
//!    wins or how many run.
//!
//! The polling path means the flow works before webhooks are set up at all.
//!
//! ## Pairing
//!
//! A Terminal is signed in to this app with a one-time device code (`Admin →
//! Billing → Square Terminals → Pair`), which needs the
//! `DEVICE_CREDENTIAL_MANAGEMENT` permission. Sandbox test devices, which have
//! fixed ids, are added by id instead.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::billing;
use crate::employee_auth::AdminEmployee;
use crate::error::AppError;
use crate::models::cart::{Cart, CartDetail, CartItem, CartStatus};
use crate::square::{
    is_active_terminal_status, money, CheckoutPush, LineItemPush, SquareError, TerminalCheckout,
    TerminalCheckoutPush,
};
use crate::AppState;

// --- Paired Terminals --------------------------------------------------------

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct Terminal {
    pub id: Uuid,
    pub name: String,
    pub device_id: Option<String>,
    pub code: Option<String>,
    pub pair_by: Option<DateTime<Utc>>,
    pub status: String,
    pub is_default: bool,
    pub created_at: DateTime<Utc>,
    pub paired_at: Option<DateTime<Utc>>,
}

const TERMINAL_COLS: &str =
    "id, name, device_id, code, pair_by, status, is_default, created_at, paired_at";

/// Square's published Sandbox test devices (the same list as the Admin →
/// Billing Sandbox section). They exist only in Sandbox: in production a
/// checkout sent to one can never be paid, so there they are hidden and
/// refused. One stayed set as the default after the 2026-10-22 switch to
/// production and was offered on the charge button.
pub(crate) const NOT_CONNECTED: &str =
    "Square isn't connected yet, so no payment can be taken — an admin needs to press \
     Connect Square in Admin → Billing";

const SANDBOX_TEST_DEVICE_IDS: [&str; 5] = [
    "9fa747a2-25ff-48ee-b078-04381f7c828f",
    "22cd266c-6246-4c06-9983-67f0c26346b0",
    "841100b9-ee60-4537-9bcf-e30b2ba5e215",
    "0a956d49-619a-4530-8e5e-8eac603ffc5e",
    "da40d603-c2ea-4a65-8cfd-f42e36dab0c7",
];

fn is_production(state: &AppState) -> bool {
    state.square.name() == "square-production"
}

fn is_sandbox_test_device(device_id: Option<&str>) -> bool {
    device_id.is_some_and(|d| SANDBOX_TEST_DEVICE_IDS.contains(&d))
}

/// Every Terminal, paired ones first. Any employee: intake and checkout need to
/// know whether there is one to charge on.
pub async fn list(State(state): State<AppState>) -> Result<Json<Vec<Terminal>>, AppError> {
    // A pending code whose deadline has passed can never pair; say so.
    sqlx::query(
        "update square_terminals set status = 'expired' \
         where status = 'pending' and pair_by is not null and pair_by < now()",
    )
    .execute(&state.db)
    .await?;

    let mut rows = sqlx::query_as::<_, Terminal>(&format!(
        "select {TERMINAL_COLS} from square_terminals \
         order by (status = 'paired') desc, is_default desc, created_at"
    ))
    .fetch_all(&state.db)
    .await?;
    if is_production(&state) {
        rows.retain(|t| !is_sandbox_test_device(t.device_id.as_deref()));
    }
    Ok(Json(rows))
}

#[derive(Deserialize)]
pub struct PairRequest {
    pub name: String,
}

fn clean_name(name: &str) -> Result<String, AppError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::BadRequest("give the Terminal a name".into()));
    }
    Ok(name.chars().take(60).collect())
}

/// Start pairing a Terminal: get a sign-in code from Square to enter on it.
pub async fn pair(
    _admin: AdminEmployee,
    State(state): State<AppState>,
    Json(body): Json<PairRequest>,
) -> Result<(StatusCode, Json<Terminal>), AppError> {
    let name = clean_name(&body.name)?;
    let code = state
        .square
        .create_device_code(&name)
        .await
        .map_err(|e| square_error("could not get a pairing code from Square", e))?;

    let row = sqlx::query_as::<_, Terminal>(&format!(
        "insert into square_terminals (name, device_code_id, code, pair_by) \
         values ($1, $2, $3, $4) returning {TERMINAL_COLS}"
    ))
    .bind(&name)
    .bind(&code.id)
    .bind(&code.code)
    .bind(code.pair_by)
    .fetch_one(&state.db)
    .await?;
    Ok((StatusCode::CREATED, Json(row)))
}

#[derive(Deserialize)]
pub struct AddByIdRequest {
    pub name: String,
    pub device_id: String,
}

/// Add a Terminal by its device id, skipping the code. For Square's Sandbox
/// test devices, which can't be paired because they don't physically exist.
pub async fn add_by_id(
    _admin: AdminEmployee,
    State(state): State<AppState>,
    Json(body): Json<AddByIdRequest>,
) -> Result<(StatusCode, Json<Terminal>), AppError> {
    // Only Sandbox's pretend devices are added by id; a real Terminal pairs
    // with a code, which proves it is signed in to this Square account.
    if is_production(&state) {
        return Err(AppError::BadRequest(
            "in production a Terminal is paired with a code — use Get a pairing code".into(),
        ));
    }
    let name = clean_name(&body.name)?;
    let device_id = body.device_id.trim();
    if device_id.is_empty() || device_id.len() > 100 {
        return Err(AppError::BadRequest("enter the Terminal's device id".into()));
    }
    let row = sqlx::query_as::<_, Terminal>(&format!(
        "insert into square_terminals (name, device_id, status, paired_at, is_default) \
         values ($1, $2, 'paired', now(), \
                 not exists (select 1 from square_terminals where is_default)) \
         returning {TERMINAL_COLS}"
    ))
    .bind(&name)
    .bind(device_id)
    .fetch_one(&state.db)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(db) if db.is_unique_violation() => {
            AppError::Conflict("that Terminal is already added".into())
        }
        _ => AppError::from(e),
    })?;
    Ok((StatusCode::CREATED, Json(row)))
}

/// Check whether a pending code has been used. The pairing screen polls this.
pub async fn check_pairing(
    _admin: AdminEmployee,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Terminal>, AppError> {
    refresh_pairing(&state, id).await?;
    let row = sqlx::query_as::<_, Terminal>(&format!(
        "select {TERMINAL_COLS} from square_terminals where id = $1"
    ))
    .bind(id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError::NotFound("Terminal not found".into()))?;
    Ok(Json(row))
}

/// Ask Square about a pending pairing code and record the result. Shared with
/// the `device.code.paired` webhook, which identifies the row by code id.
pub async fn refresh_pairing(state: &AppState, id: Uuid) -> Result<(), AppError> {
    let row: Option<(Option<String>, String)> =
        sqlx::query_as("select device_code_id, status from square_terminals where id = $1")
            .bind(id)
            .fetch_optional(&state.db)
            .await?;
    let Some((Some(code_id), status)) = row else {
        return Ok(());
    };
    if status != "pending" {
        return Ok(());
    }

    let code = state
        .square
        .get_device_code(&code_id)
        .await
        .map_err(|e| square_error("could not check the pairing code with Square", e))?;

    match (code.status.as_str(), code.device_id) {
        ("PAIRED", Some(device_id)) => {
            // The first Terminal paired becomes the one "Charge" uses.
            sqlx::query(
                "update square_terminals set status = 'paired', device_id = $2, \
                   paired_at = now(), code = null, \
                   is_default = not exists (select 1 from square_terminals where is_default) \
                 where id = $1",
            )
            .bind(id)
            .bind(&device_id)
            .execute(&state.db)
            .await?;
            tracing::info!(%id, %device_id, "square terminal paired");
        }
        ("EXPIRED", _) => {
            sqlx::query("update square_terminals set status = 'expired' where id = $1")
                .bind(id)
                .execute(&state.db)
                .await?;
        }
        _ => {}
    }
    Ok(())
}

/// Handle a `device.code.paired` webhook: find the row by code id and refresh it.
pub async fn on_device_code_event(state: &AppState, code_id: &str) -> Result<bool, AppError> {
    let id: Option<Uuid> =
        sqlx::query_scalar("select id from square_terminals where device_code_id = $1")
            .bind(code_id)
            .fetch_optional(&state.db)
            .await?;
    match id {
        Some(id) => {
            refresh_pairing(state, id).await?;
            Ok(true)
        }
        None => Ok(false),
    }
}

#[derive(Deserialize)]
pub struct UpdateTerminal {
    pub name: Option<String>,
    /// Only `true` is meaningful: make this the Terminal "Charge" uses.
    pub is_default: Option<bool>,
}

pub async fn update(
    _admin: AdminEmployee,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateTerminal>,
) -> Result<Json<Terminal>, AppError> {
    let mut tx = state.db.begin().await?;
    if let Some(name) = &body.name {
        sqlx::query("update square_terminals set name = $2 where id = $1")
            .bind(id)
            .bind(clean_name(name)?)
            .execute(&mut *tx)
            .await?;
    }
    if body.is_default == Some(true) {
        let paired: bool = sqlx::query_scalar(
            "select exists (select 1 from square_terminals where id = $1 and status = 'paired')",
        )
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
        if !paired {
            return Err(AppError::BadRequest(
                "only a paired Terminal can be the default".into(),
            ));
        }
        sqlx::query("update square_terminals set is_default = false where is_default")
            .execute(&mut *tx)
            .await?;
        sqlx::query("update square_terminals set is_default = true where id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    let row = sqlx::query_as::<_, Terminal>(&format!(
        "select {TERMINAL_COLS} from square_terminals where id = $1"
    ))
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::NotFound("Terminal not found".into()))?;
    tx.commit().await?;
    Ok(Json(row))
}

/// Forget a Terminal here. It stays signed in on the device until it is signed
/// out there (or removed in the Square dashboard); this only stops the app
/// sending it checkouts.
pub async fn remove(
    _admin: AdminEmployee,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    let mut tx = state.db.begin().await?;
    let was_default: Option<bool> =
        sqlx::query_scalar("delete from square_terminals where id = $1 returning is_default")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
    let Some(was_default) = was_default else {
        return Err(AppError::NotFound("Terminal not found".into()));
    };
    // Hand the default to another paired Terminal, if there is one.
    if was_default {
        sqlx::query(
            "update square_terminals set is_default = true where id = \
             (select id from square_terminals where status = 'paired' order by paired_at limit 1)",
        )
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

// --- Charging a cart -----------------------------------------------------------

#[derive(Deserialize, Default)]
pub struct ChargeRequest {
    /// Which Terminal. Omitted means the default one.
    pub terminal_id: Option<Uuid>,
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

fn square_error(context: &str, e: SquareError) -> AppError {
    tracing::error!("{context}: {e}");
    if e.retryable() {
        AppError::Unavailable(format!("{context} — Square is unavailable, try again: {e}"))
    } else {
        AppError::BadRequest(format!("{context}: {e}"))
    }
}

/// Send a cart to the Terminal.
///
/// Safe to press twice: while a checkout is still on the device, this returns
/// the cart as it is rather than sending another. After the customer (or staff)
/// cancels on the Terminal, pressing it again sends a fresh checkout for the
/// same Square order.
pub async fn charge(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    body: Option<Json<ChargeRequest>>,
) -> Result<Json<CartDetail>, AppError> {
    let body = body.map(|Json(b)| b).unwrap_or_default();
    let CartDetail { cart, items } = load_cart(&state, id).await?;

    match cart.status {
        CartStatus::Open => {}
        CartStatus::PendingPayment if cart.payment_method.as_deref() == Some("terminal") => {
            if cart
                .terminal_status
                .as_deref()
                .is_some_and(is_active_terminal_status)
            {
                return Ok(Json(CartDetail { cart, items }));
            }
        }
        CartStatus::PendingPayment => {
            return Err(AppError::Conflict(
                "this cart already has a payment link — cancel it to charge on the Terminal instead"
                    .into(),
            ))
        }
        other => {
            return Err(AppError::Conflict(format!(
                "this cart is {other:?} and can't be charged"
            )))
        }
    }

    if state.square.awaiting_connection() {
        return Err(AppError::Unavailable(NOT_CONNECTED.into()));
    }
    if items.is_empty() {
        return Err(AppError::BadRequest("cart is empty".into()));
    }
    // Same last-moment check as the payment link: charge what's on the cart.
    let line_sum: i64 = items.iter().map(|i| i.line_total_cents()).sum();
    if line_sum - cart.discount_cents != cart.total_cents {
        return Err(AppError::Conflict(
            "cart total does not match its line items; rebuild the cart".into(),
        ));
    }
    if cart.total_cents <= 0 {
        return Err(AppError::BadRequest(
            "nothing to charge — the cart total is $0".into(),
        ));
    }

    let device_id: Option<String> = match body.terminal_id {
        Some(tid) => sqlx::query_scalar(
            "select device_id from square_terminals where id = $1 and status = 'paired'",
        )
        .bind(tid)
        .fetch_optional(&state.db)
        .await?
        .flatten(),
        None => sqlx::query_scalar(
            "select device_id from square_terminals where is_default and status = 'paired'",
        )
        .fetch_optional(&state.db)
        .await?
        .flatten(),
    };
    let device_id = device_id
        .filter(|d| !(is_production(&state) && is_sandbox_test_device(Some(d))))
        .ok_or_else(|| {
            AppError::BadRequest(
                "no Square Terminal is paired — an admin can pair one in Admin → Billing".into(),
            )
        })?;

    // The Square order: created once per cart, then reused by every attempt.
    let square_order_id = match &cart.square_order_id {
        Some(existing) => existing.clone(),
        None => {
            let push = CheckoutPush {
                cart_id: cart.id,
                // Distinct from the key a payment link would use, so the two
                // endpoints can never be confused by Square's dedup.
                idempotency_key: format!("{}-order", cart.idempotency_key),
                currency: cart.currency.clone(),
                buyer_email: None,
                line_items: items
                    .iter()
                    .map(|i| LineItemPush {
                        name: i.name.clone(),
                        quantity: i.quantity,
                        unit_amount_cents: i.unit_amount_cents,
                    })
                    .collect(),
                discounts: crate::routes::carts::discount_push(&cart),
                redirect_url: None,
                note: cart.note.clone(),
                ask_for_shipping_address: false,
            };
            let order_id = state
                .square
                .create_order(&push)
                .await
                .map_err(|e| square_error("Square would not create the order", e))?;
            // Recorded straight away: the cart is now tied to this Square order,
            // and settlement finds it by this id.
            sqlx::query(
                "update carts set status = 'pending_payment', payment_method = 'terminal', \
                   square_order_id = $2, checkout_at = now(), updated_at = now() \
                 where id = $1",
            )
            .bind(cart.id)
            .bind(&order_id)
            .execute(&state.db)
            .await?;
            order_id
        }
    };

    // Deterministic per attempt, so a request that timed out and is retried
    // resolves to the same checkout instead of queueing a second one on the
    // device. A new attempt after a cancel gets a new key.
    let idempotency_key = match &cart.terminal_checkout_id {
        None => format!("{}-tc-first", cart.idempotency_key),
        Some(previous) => format!("{}-tc-after-{previous}", cart.idempotency_key),
    };

    let checkout = state
        .square
        .create_terminal_checkout(&TerminalCheckoutPush {
            idempotency_key,
            square_order_id,
            amount_cents: cart.total_cents,
            currency: cart.currency.clone(),
            device_id: device_id.clone(),
            reference_id: cart.id.to_string(),
            note: cart.note.clone(),
        })
        .await
        .map_err(|e| square_error("Square would not send the checkout to the Terminal", e))?;

    record_checkout(&state, cart.id, &checkout, Some(&device_id)).await?;

    tracing::info!(
        cart_id = %cart.id,
        checkout_id = %checkout.id,
        total = %money::format_cents(cart.total_cents, &cart.currency),
        "sent to square terminal"
    );

    Ok(Json(load_cart(&state, id).await?))
}

async fn record_checkout(
    state: &AppState,
    cart_id: Uuid,
    checkout: &TerminalCheckout,
    device_id: Option<&str>,
) -> Result<(), AppError> {
    sqlx::query(
        "update carts set terminal_checkout_id = $2, terminal_status = $3, \
           terminal_cancel_reason = $4, \
           terminal_device_id = coalesce($5, terminal_device_id), updated_at = now() \
         where id = $1",
    )
    .bind(cart_id)
    .bind(&checkout.id)
    .bind(&checkout.status)
    .bind(&checkout.cancel_reason)
    .bind(device_id)
    .execute(&state.db)
    .await?;
    Ok(())
}

/// Bring a Terminal checkout's state in from Square and, once it's paid, settle
/// the cart. The one place this happens, for the poll and the webhook alike.
/// Returns the cart it belongs to, if any.
pub async fn sync_checkout(state: &AppState, checkout_id: &str) -> Result<Option<Uuid>, AppError> {
    let cart_id: Option<Uuid> =
        sqlx::query_scalar("select id from carts where terminal_checkout_id = $1")
            .bind(checkout_id)
            .fetch_optional(&state.db)
            .await?;
    let Some(cart_id) = cart_id else {
        return Ok(None);
    };

    let checkout = state
        .square
        .get_terminal_checkout(checkout_id)
        .await
        .map_err(|e| square_error("could not check the Terminal with Square", e))?;
    record_checkout(state, cart_id, &checkout, None).await?;

    // Settlement re-fetches each payment rather than trusting anything the
    // checkout says about it, the same as the payment webhook.
    for payment_id in &checkout.payment_ids {
        let payment = state
            .square
            .get_payment(payment_id)
            .await
            .map_err(|e| square_error("could not fetch the payment from Square", e))?;
        billing::apply_payment(&state.db, &payment).await?;
    }
    Ok(Some(cart_id))
}

/// The cart with its Terminal state brought up to date. The checkout screen
/// polls this every couple of seconds while the customer is at the Terminal.
pub async fn status(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<CartDetail>, AppError> {
    let detail = load_cart(&state, id).await?;
    // Only ask Square while there's something to learn: an unpaid cart with a
    // checkout out. A paid or canceled cart is answered from the database.
    if detail.cart.status == CartStatus::PendingPayment {
        if let Some(checkout_id) = detail.cart.terminal_checkout_id.clone() {
            sync_checkout(&state, &checkout_id).await?;
            return Ok(Json(load_cart(&state, id).await?));
        }
    }
    Ok(Json(detail))
}

/// Take the checkout off the Terminal. The cart stays as it is, so it can be
/// sent again or canceled outright.
pub async fn cancel(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<CartDetail>, AppError> {
    let cart = load_cart(&state, id).await?.cart;
    let Some(checkout_id) = cart.terminal_checkout_id.clone() else {
        return Err(AppError::BadRequest("this cart isn't on a Terminal".into()));
    };
    match state.square.cancel_terminal_checkout(&checkout_id).await {
        Ok(checkout) => record_checkout(&state, id, &checkout, None).await?,
        // Too late to cancel — most likely the customer has just paid. Pull the
        // real state in rather than reporting a failure that isn't one.
        Err(SquareError::Api { status: 400, .. }) => {
            sync_checkout(&state, &checkout_id).await?;
        }
        Err(e) => return Err(square_error("could not cancel on the Terminal", e)),
    }
    Ok(Json(load_cart(&state, id).await?))
}

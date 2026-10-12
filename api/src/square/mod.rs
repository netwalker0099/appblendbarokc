//! The Square boundary. Everything this app asks of Square goes through the
//! [`Square`] trait, so the cart/checkout/Terminal code can be driven by an
//! in-process mock (no credentials) or the real HTTP client (credentials present)
//! without any caller changing. Same shape as the Squarespace boundary it
//! replaces, for the same reason: the interesting logic stays testable on a box
//! with no API keys.
//!
//! ## Money
//!
//! Square speaks integer **minor units** — cents for USD — plus an ISO currency
//! code. It never accepts a decimal. Every amount crossing this boundary is
//! therefore an `i64` of cents, and the conversion from the `numeric(10,2)`
//! prices the operator types lives in [`money`], which is unit-tested. Doing the
//! conversion here rather than at each call site is deliberate: a stray
//! `as i64` on a float is exactly how you ship an order for $0.60 instead of $60.

mod http;
mod mock;
pub mod money;

pub use http::HttpSquare;
pub use mock::MockSquare;

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use uuid::Uuid;

/// Which Square environment the credentials point at. Sandbox and production are
/// entirely separate hosts *and* separate credential sets — a sandbox token will
/// simply 401 against production, so this is a hard config choice, not a flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SquareEnv {
    Sandbox,
    Production,
}

impl SquareEnv {
    pub fn base_url(&self) -> &'static str {
        match self {
            SquareEnv::Sandbox => "https://connect.squareupsandbox.com",
            SquareEnv::Production => "https://connect.squareup.com",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            SquareEnv::Sandbox => "sandbox",
            SquareEnv::Production => "production",
        }
    }
}

/// One line of a cart on its way to Square.
#[derive(Debug, Clone)]
pub struct LineItemPush {
    pub name: String,
    pub quantity: i32,
    pub unit_amount_cents: i64,
}

/// A fixed amount off the whole order. Square models this as an order-level
/// discount rather than a negative line item, which it does not accept.
#[derive(Debug, Clone)]
pub struct DiscountPush {
    pub name: String,
    pub amount_cents: i64,
}

/// A cart being handed to Square to collect money for.
#[derive(Debug)]
pub struct CheckoutPush {
    /// Our cart id. Travels as the Square order's `reference_id`, which is what
    /// makes reconciliation possible in the direction Square -> us.
    pub cart_id: Uuid,
    /// Reused verbatim on retries so a timed-out create doesn't mint a second link.
    pub idempotency_key: String,
    pub currency: String,
    pub buyer_email: Option<String>,
    pub line_items: Vec<LineItemPush>,
    /// Order-level discounts (referral, coupon). Never enough to take the order
    /// below zero — the caller clamps first.
    pub discounts: Vec<DiscountPush>,
    /// Where Square sends the browser after a successful payment.
    pub redirect_url: Option<String>,
    pub note: Option<String>,
    /// Have Square's hosted page collect a shipping address. Only the online
    /// shop sets this — everything else is handed over at the bar.
    pub ask_for_shipping_address: bool,
}

/// What Square hands back when a payment link is created.
#[derive(Debug)]
pub struct CheckoutHandle {
    pub payment_link_id: String,
    pub square_order_id: String,
    /// The hosted page the customer actually pays on.
    pub url: String,
}

/// A checkout sent to a Square Terminal through the cloud Terminal API.
///
/// The Terminal is a Wi-Fi device paired to this app's Square account; Square
/// relays the request to it, the customer taps or inserts, and the result comes
/// back as a payment on the Square order. The browser that pressed the button
/// is never involved in the card side at all, so it works the same from a
/// laptop or an iPad.
#[derive(Debug, Clone)]
pub struct TerminalCheckoutPush {
    /// Reused on retries so a timed-out request can't put two checkouts on
    /// the device.
    pub idempotency_key: String,
    /// The Square order this pays. The payment Square records carries it,
    /// which is how settlement finds the cart — the same join the payment-link
    /// path uses.
    pub square_order_id: String,
    pub amount_cents: i64,
    pub currency: String,
    pub device_id: String,
    /// Our cart id, for Square-side searching.
    pub reference_id: String,
    pub note: Option<String>,
}

/// A Terminal checkout as Square currently reports it.
#[derive(Debug, Clone)]
pub struct TerminalCheckout {
    pub id: String,
    /// PENDING, IN_PROGRESS, CANCEL_REQUESTED, CANCELED or COMPLETED.
    pub status: String,
    /// Set once the customer has paid.
    pub payment_ids: Vec<String>,
    /// BUYER_CANCELED, SELLER_CANCELED or TIMED_OUT, when canceled.
    pub cancel_reason: Option<String>,
}

impl TerminalCheckout {
    /// Still waiting on the device — another checkout must not be sent.
    pub fn is_active(&self) -> bool {
        is_active_terminal_status(&self.status)
    }
}

pub fn is_active_terminal_status(status: &str) -> bool {
    matches!(status, "PENDING" | "IN_PROGRESS" | "CANCEL_REQUESTED")
}

/// A one-time code for signing a Square Terminal in to this app.
#[derive(Debug, Clone)]
pub struct DeviceCode {
    pub id: String,
    /// What the operator types on the Terminal's sign-in screen.
    pub code: String,
    /// UNPAIRED, PAIRED or EXPIRED.
    pub status: String,
    /// Set once paired — the id checkouts are sent to.
    pub device_id: Option<String>,
    pub pair_by: Option<DateTime<Utc>>,
}

/// Authoritative state of a payment, fetched back from Square. A webhook carries
/// a payload we could read directly, but we re-fetch instead: the webhook body is
/// attacker-shaped input until proven otherwise, and re-fetching costs one call.
#[derive(Debug, Clone)]
pub struct RemotePayment {
    pub payment_id: String,
    pub square_order_id: Option<String>,
    /// APPROVED / COMPLETED / CANCELED / FAILED.
    pub status: String,
    pub amount_cents: i64,
    pub currency: String,
    pub created_at: DateTime<Utc>,
    /// Total refunded against this payment, if any.
    pub refunded_cents: i64,
}

impl RemotePayment {
    pub fn is_completed(&self) -> bool {
        self.status.eq_ignore_ascii_case("COMPLETED")
    }

    pub fn is_refunded(&self) -> bool {
        self.refunded_cents > 0
    }
}

/// A customer to upsert into Square Customers.
#[derive(Debug)]
pub struct CustomerPush {
    pub id: Uuid,
    pub email: String,
    pub name: Option<String>,
    pub marketing_consent: bool,
}

#[derive(Debug)]
pub enum SquareError {
    /// Transport/connection problem — always worth retrying.
    Transport(String),
    /// Square answered with an error status. `retryable` is true for 5xx and 429.
    Api {
        status: u16,
        body: String,
        retryable: bool,
    },
    /// Misconfiguration or bad data — retrying won't help until something changes.
    Config(String),
}

impl SquareError {
    pub fn retryable(&self) -> bool {
        match self {
            SquareError::Transport(_) => true,
            SquareError::Api { retryable, .. } => *retryable,
            SquareError::Config(_) => false,
        }
    }
}

impl std::fmt::Display for SquareError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SquareError::Transport(msg) => write!(f, "transport error: {msg}"),
            SquareError::Api { status, body, .. } => write!(f, "api error {status}: {body}"),
            SquareError::Config(msg) => write!(f, "config error: {msg}"),
        }
    }
}

#[async_trait]
pub trait Square: Send + Sync {
    /// Short label for logs and the integration-status endpoint.
    fn name(&self) -> &'static str;

    /// True when this backend talks to the real Square. The admin UI uses it to
    /// make "you are not actually charging anyone" impossible to miss.
    fn is_live(&self) -> bool;

    /// Push a cart and get back a hosted checkout URL.
    async fn create_checkout(&self, push: &CheckoutPush) -> Result<CheckoutHandle, SquareError>;

    /// Fetch a payment by id (the webhook path).
    async fn get_payment(&self, payment_id: &str) -> Result<RemotePayment, SquareError>;

    /// The payment settled against a Square order, if any.
    ///
    /// This is the missed-webhook escape hatch: webhooks get lost (endpoint down
    /// during a deploy, signature key rotated mid-flight), and without a pull
    /// path a paid cart would sit at `pending_payment` forever with the customer
    /// insisting they paid. Backs the "refresh from Square" action.
    async fn find_payment_for_order(
        &self,
        square_order_id: &str,
    ) -> Result<Option<RemotePayment>, SquareError>;

    /// Create a Square order for a cart, for paying on a Terminal. (A payment
    /// link creates its own order, so the link path doesn't use this.)
    /// Returns the Square order id. Idempotent on `push.idempotency_key`.
    async fn create_order(&self, push: &CheckoutPush) -> Result<String, SquareError>;

    /// Send a checkout to a paired Square Terminal.
    async fn create_terminal_checkout(
        &self,
        push: &TerminalCheckoutPush,
    ) -> Result<TerminalCheckout, SquareError>;

    /// Square's current view of a Terminal checkout — the polling path, and
    /// what the webhook re-fetches rather than trusting the payload.
    async fn get_terminal_checkout(&self, checkout_id: &str)
        -> Result<TerminalCheckout, SquareError>;

    /// Ask the Terminal to drop a checkout that hasn't been paid. Only possible
    /// while it is PENDING or IN_PROGRESS.
    async fn cancel_terminal_checkout(
        &self,
        checkout_id: &str,
    ) -> Result<TerminalCheckout, SquareError>;

    /// Ask Square for a code to sign a Terminal in to this app with.
    async fn create_device_code(&self, name: &str) -> Result<DeviceCode, SquareError>;

    /// Whether a device code has been used yet, and by which Terminal.
    async fn get_device_code(&self, code_id: &str) -> Result<DeviceCode, SquareError>;

    /// Void a payment link so it can no longer be paid.
    ///
    /// Required for correctness, not tidiness. Cancelling a cart releases its
    /// orders to be sold again; if the link stayed live, a customer could pay the
    /// dead link afterwards and Square would hold money against a cart whose
    /// blends had already been re-sold on another. Deleting the link makes that
    /// impossible rather than merely unlikely.
    ///
    /// Idempotent: a link that is already gone is a success, not an error.
    async fn void_checkout(&self, payment_link_id: &str) -> Result<(), SquareError>;

    /// Create or update the customer, returning its Square id.
    async fn upsert_customer(&self, customer: &CustomerPush) -> Result<String, SquareError>;
}

/// Configuration read once at startup.
pub struct SquareConfig {
    pub access_token: String,
    pub location_id: String,
    pub env: SquareEnv,
    pub redirect_url: Option<String>,
}

/// Build the live client when a token *and* a location id are both configured,
/// otherwise the mock.
///
/// Both are required on purpose: Square rejects an order with no `location_id`,
/// so a token-only config would fail at the worst possible moment (mid-checkout,
/// in front of a customer) rather than at boot. Failing to the mock keeps the
/// stand usable and makes the gap obvious in the admin panel.
pub fn from_env() -> Arc<dyn Square> {
    let token = std::env::var("SQUARE_ACCESS_TOKEN")
        .ok()
        .filter(|s| !s.trim().is_empty());
    let location = std::env::var("SQUARE_LOCATION_ID")
        .ok()
        .filter(|s| !s.trim().is_empty());

    let env = match std::env::var("SQUARE_ENV").as_deref() {
        Ok("production") | Ok("prod") => SquareEnv::Production,
        _ => SquareEnv::Sandbox,
    };

    let redirect_url = std::env::var("SQUARE_REDIRECT_URL")
        .ok()
        .filter(|s| !s.trim().is_empty());

    match (token, location) {
        (Some(access_token), Some(location_id)) => {
            tracing::info!(
                env = env.label(),
                location_id = %location_id,
                "square: live HTTP client"
            );
            if env == SquareEnv::Production {
                tracing::warn!("square: PRODUCTION environment — real cards will be charged");
            }
            Arc::new(HttpSquare::new(SquareConfig {
                access_token,
                location_id,
                env,
                redirect_url,
            }))
        }
        (token, location) => {
            tracing::warn!(
                have_token = token.is_some(),
                have_location = location.is_some(),
                "square: SQUARE_ACCESS_TOKEN and SQUARE_LOCATION_ID must both be set \
                 — falling back to the in-process mock, no real payments will be taken"
            );
            Arc::new(MockSquare::default())
        }
    }
}

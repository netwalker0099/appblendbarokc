use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde_json::{json, Value};

use super::{
    CheckoutHandle, CheckoutPush, CustomerPush, DeviceCode, RemotePayment, Square, SquareConfig,
    SquareError, TerminalCheckout, TerminalCheckoutPush,
};

/// Pinned Square API version. Square dates its API and keeps old versions working;
/// pinning means a Square-side release cannot change response shapes under us.
/// Bump deliberately, after reading their changelog — never leave this unset.
const SQUARE_VERSION: &str = "2025-01-23";
const USER_AGENT: &str = "blendbar-app/0.2";

/// Live Square client.
///
/// The payment-link path was verified against Square Sandbox on 2026-10-01, and
/// the Terminal path on 2026-10-08 using Square's Sandbox test devices. Neither
/// has yet taken a production payment.
pub struct HttpSquare {
    client: Client,
    config: SquareConfig,
}

impl HttpSquare {
    pub fn new(config: SquareConfig) -> Self {
        let client = Client::builder()
            .user_agent(USER_AGENT)
            // A stand operator is standing in front of a customer waiting for a
            // QR code; fail fast rather than hang.
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .expect("failed to build reqwest client");
        Self { client, config }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.config.env.base_url(), path)
    }

    async fn send(&self, req: reqwest::RequestBuilder) -> Result<Value, SquareError> {
        let resp = req
            .bearer_auth(&self.config.access_token)
            .header("Square-Version", SQUARE_VERSION)
            .send()
            .await
            .map_err(|e| SquareError::Transport(e.to_string()))?;

        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();

        if status.is_success() {
            return Ok(serde_json::from_str(&text).unwrap_or(Value::Null));
        }

        let code = status.as_u16();
        // 5xx and 429 are transient. Everything else is our fault (bad token,
        // bad location, malformed body) and will fail identically on retry.
        let retryable = status.is_server_error() || code == 429;
        Err(SquareError::Api {
            status: code,
            // Square returns {"errors":[{"category","code","detail"}]}; surface
            // the detail lines rather than the raw envelope so the admin panel
            // shows something actionable.
            body: summarize_errors(&text),
            retryable,
        })
    }

    async fn post(&self, path: &str, body: Value) -> Result<Value, SquareError> {
        self.send(self.client.post(self.url(path)).json(&body)).await
    }

    async fn get(&self, path: &str) -> Result<Value, SquareError> {
        self.send(self.client.get(self.url(path))).await
    }

    /// The Square order for a cart: its lines, discounts and our cart id. Shared
    /// by the payment link (which wraps it) and the Terminal path (which creates
    /// it on its own), so both send Square exactly the same order.
    fn order_body(&self, push: &CheckoutPush) -> Value {
        let line_items: Vec<Value> = push
            .line_items
            .iter()
            .map(|li| {
                json!({
                    "name": li.name,
                    // Square wants quantity as a string. Yes, really.
                    "quantity": li.quantity.to_string(),
                    "base_price_money": {
                        "amount": li.unit_amount_cents,
                        "currency": push.currency,
                    },
                })
            })
            .collect();

        let mut order = json!({
            "location_id": self.config.location_id,
            // Our cart id: given any Square order we can name the local
            // cart that produced it.
            "reference_id": push.cart_id.to_string(),
            "line_items": line_items,
        });
        if let Some(note) = &push.note {
            order["note"] = json!(note);
        }

        // Square has no negative line items; a reduction is an order-level
        // discount with a FIXED_AMOUNT type.
        if !push.discounts.is_empty() {
            order["discounts"] = json!(push
                .discounts
                .iter()
                .map(|d| json!({
                    "name": d.name,
                    "type": "FIXED_AMOUNT",
                    "amount_money": { "amount": d.amount_cents, "currency": push.currency },
                    "scope": "ORDER",
                }))
                .collect::<Vec<_>>());
        }

        order
    }
}

/// Pull `errors[].detail` out of a Square error envelope, falling back to the raw
/// body if it isn't shaped as expected.
fn summarize_errors(text: &str) -> String {
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return text.to_string();
    };
    let Some(errors) = v.get("errors").and_then(Value::as_array) else {
        return text.to_string();
    };
    let details: Vec<String> = errors
        .iter()
        .map(|e| {
            let code = e.get("code").and_then(Value::as_str).unwrap_or("UNKNOWN");
            let detail = e
                .get("detail")
                .and_then(Value::as_str)
                .unwrap_or("(no detail)");
            format!("{code}: {detail}")
        })
        .collect();
    if details.is_empty() {
        text.to_string()
    } else {
        details.join("; ")
    }
}

/// Square money objects are `{"amount": <minor units>, "currency": "USD"}`.
fn money_cents(v: Option<&Value>) -> i64 {
    v.and_then(|m| m.get("amount"))
        .and_then(Value::as_i64)
        .unwrap_or(0)
}

fn money_currency(v: Option<&Value>) -> String {
    v.and_then(|m| m.get("currency"))
        .and_then(Value::as_str)
        .unwrap_or(super::money::DEFAULT_CURRENCY)
        .to_string()
}

fn parse_payment(p: &Value) -> Result<RemotePayment, SquareError> {
    let payment_id = p
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| SquareError::Config(format!("payment has no id: {p}")))?
        .to_string();

    let created_at = p
        .get("created_at")
        .and_then(Value::as_str)
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(Utc::now);

    Ok(RemotePayment {
        payment_id,
        square_order_id: p
            .get("order_id")
            .and_then(Value::as_str)
            .map(str::to_string),
        status: p
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("UNKNOWN")
            .to_string(),
        amount_cents: money_cents(p.get("amount_money")),
        currency: money_currency(p.get("amount_money")),
        created_at,
        refunded_cents: money_cents(p.get("refunded_money")),
    })
}

/// `{"checkout": {...}}` from any of the Terminal checkout endpoints.
fn parse_terminal_checkout(resp: &Value) -> Result<TerminalCheckout, SquareError> {
    let c = resp
        .get("checkout")
        .ok_or_else(|| SquareError::Config(format!("no checkout in response: {resp}")))?;
    let text = |k: &str| c.get(k).and_then(Value::as_str).map(str::to_string);
    Ok(TerminalCheckout {
        id: text("id")
            .ok_or_else(|| SquareError::Config(format!("terminal checkout has no id: {c}")))?,
        status: text("status").unwrap_or_else(|| "UNKNOWN".into()),
        payment_ids: c
            .get("payment_ids")
            .and_then(Value::as_array)
            .map(|ids| ids.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default(),
        cancel_reason: text("cancel_reason"),
    })
}

/// `{"device_code": {...}}` from the Devices API.
fn parse_device_code(resp: &Value) -> Result<DeviceCode, SquareError> {
    let d = resp
        .get("device_code")
        .ok_or_else(|| SquareError::Config(format!("no device_code in response: {resp}")))?;
    let text = |k: &str| d.get(k).and_then(Value::as_str).map(str::to_string);
    Ok(DeviceCode {
        id: text("id").ok_or_else(|| SquareError::Config(format!("device code has no id: {d}")))?,
        code: text("code").unwrap_or_default(),
        status: text("status").unwrap_or_else(|| "UNKNOWN".into()),
        device_id: text("device_id"),
        pair_by: text("pair_by")
            .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
            .map(|dt| dt.with_timezone(&Utc)),
    })
}

#[async_trait]
impl Square for HttpSquare {
    fn name(&self) -> &'static str {
        match self.config.env {
            super::SquareEnv::Sandbox => "square-sandbox",
            super::SquareEnv::Production => "square-production",
        }
    }

    fn is_live(&self) -> bool {
        true
    }

    async fn create_checkout(&self, push: &CheckoutPush) -> Result<CheckoutHandle, SquareError> {
        if push.line_items.is_empty() {
            return Err(SquareError::Config("cannot check out an empty cart".into()));
        }

        let order = self.order_body(push);
        let mut body = json!({
            // Square dedups creates on this key for 24h, so a retried call after
            // a network timeout returns the original link rather than a second one.
            "idempotency_key": push.idempotency_key,
            "order": order,
        });

        let redirect = push
            .redirect_url
            .as_ref()
            .or(self.config.redirect_url.as_ref());
        if redirect.is_some() || push.ask_for_shipping_address {
            let mut options = json!({ "ask_for_shipping_address": push.ask_for_shipping_address });
            if let Some(url) = redirect {
                options["redirect_url"] = json!(url);
            }
            body["checkout_options"] = options;
        }
        if let Some(email) = &push.buyer_email {
            body["pre_populated_data"] = json!({ "buyer_email": email });
        }

        let resp = self.post("/v2/online-checkout/payment-links", body).await?;

        let link = resp
            .get("payment_link")
            .ok_or_else(|| SquareError::Config(format!("no payment_link in response: {resp}")))?;

        let field = |name: &str| -> Result<String, SquareError> {
            link.get(name)
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| SquareError::Config(format!("payment_link has no `{name}`: {link}")))
        };

        Ok(CheckoutHandle {
            payment_link_id: field("id")?,
            square_order_id: field("order_id")?,
            url: field("url")?,
        })
    }

    async fn get_payment(&self, payment_id: &str) -> Result<RemotePayment, SquareError> {
        let resp = self.get(&format!("/v2/payments/{payment_id}")).await?;
        let payment = resp
            .get("payment")
            .ok_or_else(|| SquareError::Config(format!("no payment in response: {resp}")))?;
        parse_payment(payment)
    }

    async fn find_payment_for_order(
        &self,
        square_order_id: &str,
    ) -> Result<Option<RemotePayment>, SquareError> {
        let resp = self.get(&format!("/v2/orders/{square_order_id}")).await?;
        let Some(order) = resp.get("order") else {
            return Ok(None);
        };

        // An order's tenders carry the payment ids that settled it. An unpaid
        // order has none, which is a legitimate "not yet", not an error.
        let payment_id = order
            .get("tenders")
            .and_then(Value::as_array)
            .and_then(|t| t.first())
            .and_then(|t| t.get("payment_id").or_else(|| t.get("id")))
            .and_then(Value::as_str);

        match payment_id {
            Some(id) => Ok(Some(self.get_payment(id).await?)),
            None => Ok(None),
        }
    }

    async fn create_order(&self, push: &CheckoutPush) -> Result<String, SquareError> {
        if push.line_items.is_empty() {
            return Err(SquareError::Config("cannot check out an empty cart".into()));
        }
        let resp = self
            .post(
                "/v2/orders",
                json!({
                    "idempotency_key": push.idempotency_key,
                    "order": self.order_body(push),
                }),
            )
            .await?;
        resp.get("order")
            .and_then(|o| o.get("id"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| SquareError::Config(format!("no order id in response: {resp}")))
    }

    async fn create_terminal_checkout(
        &self,
        push: &TerminalCheckoutPush,
    ) -> Result<TerminalCheckout, SquareError> {
        let mut checkout = json!({
            "amount_money": { "amount": push.amount_cents, "currency": push.currency },
            "order_id": push.square_order_id,
            "reference_id": push.reference_id,
            "device_options": { "device_id": push.device_id },
        });
        if let Some(note) = &push.note {
            // Square caps the note at 500 characters.
            checkout["note"] = json!(note.chars().take(500).collect::<String>());
        }
        let resp = self
            .post(
                "/v2/terminals/checkouts",
                json!({ "idempotency_key": push.idempotency_key, "checkout": checkout }),
            )
            .await?;
        parse_terminal_checkout(&resp)
    }

    async fn get_terminal_checkout(
        &self,
        checkout_id: &str,
    ) -> Result<TerminalCheckout, SquareError> {
        let resp = self
            .get(&format!("/v2/terminals/checkouts/{checkout_id}"))
            .await?;
        parse_terminal_checkout(&resp)
    }

    async fn cancel_terminal_checkout(
        &self,
        checkout_id: &str,
    ) -> Result<TerminalCheckout, SquareError> {
        let resp = self
            .post(
                &format!("/v2/terminals/checkouts/{checkout_id}/cancel"),
                json!({}),
            )
            .await?;
        parse_terminal_checkout(&resp)
    }

    async fn create_device_code(&self, name: &str) -> Result<DeviceCode, SquareError> {
        let resp = self
            .post(
                "/v2/devices/codes",
                json!({
                    "idempotency_key": uuid::Uuid::new_v4().to_string(),
                    "device_code": {
                        "name": name,
                        "product_type": "TERMINAL_API",
                        "location_id": self.config.location_id,
                    },
                }),
            )
            .await?;
        parse_device_code(&resp)
    }

    async fn get_device_code(&self, code_id: &str) -> Result<DeviceCode, SquareError> {
        let resp = self.get(&format!("/v2/devices/codes/{code_id}")).await?;
        parse_device_code(&resp)
    }

    async fn void_checkout(&self, payment_link_id: &str) -> Result<(), SquareError> {
        let result = self
            .send(
                self.client
                    .delete(self.url(&format!("/v2/online-checkout/payment-links/{payment_link_id}"))),
            )
            .await;

        match result {
            Ok(_) => Ok(()),
            // Already gone is the desired end state, so treat it as success —
            // otherwise a retried cancel would fail forever on the second pass.
            Err(SquareError::Api { status: 404, .. }) => Ok(()),
            Err(e) => Err(e),
        }
    }

    async fn upsert_customer(&self, customer: &CustomerPush) -> Result<String, SquareError> {
        // Square has no upsert-by-email, so: search, then update or create.
        let search = self
            .post(
                "/v2/customers/search",
                json!({
                    "limit": 1,
                    "query": { "filter": { "email_address": { "exact": customer.email } } }
                }),
            )
            .await?;

        let existing = search
            .get("customers")
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .and_then(|c| c.get("id"))
            .and_then(Value::as_str)
            .map(str::to_string);

        // Square models consent as its inverse: `email_unsubscribed`. A customer
        // who did not opt in must be marked unsubscribed, or Square Marketing
        // will happily mail them.
        let mut body = json!({
            "email_address": customer.email,
            "reference_id": customer.id.to_string(),
            "preferences": { "email_unsubscribed": !customer.marketing_consent },
        });
        if let Some(name) = &customer.name {
            body["given_name"] = json!(name);
        }

        let resp = match &existing {
            Some(id) => self.post(&format!("/v2/customers/{id}"), body).await?,
            None => {
                body["idempotency_key"] = json!(customer.id.to_string());
                self.post("/v2/customers", body).await?
            }
        };

        resp.get("customer")
            .and_then(|c| c.get("id"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .or(existing)
            .ok_or_else(|| SquareError::Config(format!("no customer id in response: {resp}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarizes_square_error_envelopes() {
        let body = r#"{"errors":[{"category":"INVALID_REQUEST_ERROR","code":"NOT_FOUND","detail":"Location not found."}]}"#;
        assert_eq!(summarize_errors(body), "NOT_FOUND: Location not found.");
    }

    #[test]
    fn falls_back_to_raw_body_when_unrecognized() {
        assert_eq!(summarize_errors("gateway timeout"), "gateway timeout");
    }

    #[test]
    fn parses_a_payment() {
        let v = serde_json::json!({
            "id": "pay_123",
            "order_id": "ord_456",
            "status": "COMPLETED",
            "amount_money": { "amount": 6000, "currency": "USD" },
            "refunded_money": { "amount": 500, "currency": "USD" },
            "created_at": "2026-07-27T10:00:00Z"
        });
        let p = parse_payment(&v).unwrap();
        assert_eq!(p.payment_id, "pay_123");
        assert_eq!(p.square_order_id.as_deref(), Some("ord_456"));
        assert_eq!(p.amount_cents, 6000);
        assert_eq!(p.refunded_cents, 500);
        assert!(p.is_completed());
        assert!(p.is_refunded());
    }

    #[test]
    fn parses_a_completed_terminal_checkout() {
        let v = serde_json::json!({ "checkout": {
            "id": "tc_1", "status": "COMPLETED",
            "payment_ids": ["pay_1"],
            "amount_money": { "amount": 2500, "currency": "USD" }
        }});
        let c = parse_terminal_checkout(&v).unwrap();
        assert_eq!(c.id, "tc_1");
        assert_eq!(c.payment_ids, vec!["pay_1".to_string()]);
        assert!(!c.is_active());
        assert!(c.cancel_reason.is_none());
    }

    #[test]
    fn parses_a_canceled_terminal_checkout() {
        let v = serde_json::json!({ "checkout": {
            "id": "tc_2", "status": "CANCELED", "cancel_reason": "BUYER_CANCELED"
        }});
        let c = parse_terminal_checkout(&v).unwrap();
        assert!(c.payment_ids.is_empty());
        assert_eq!(c.cancel_reason.as_deref(), Some("BUYER_CANCELED"));
    }

    #[test]
    fn parses_a_device_code_before_and_after_pairing() {
        let unpaired = serde_json::json!({ "device_code": {
            "id": "dc_1", "code": "JHHYHH", "status": "UNPAIRED",
            "product_type": "TERMINAL_API", "pair_by": "2026-10-08T12:05:00Z"
        }});
        let d = parse_device_code(&unpaired).unwrap();
        assert_eq!(d.code, "JHHYHH");
        assert!(d.device_id.is_none());
        assert!(d.pair_by.is_some());

        let paired = serde_json::json!({ "device_code": {
            "id": "dc_1", "code": "JHHYHH", "status": "PAIRED", "device_id": "dev_9"
        }});
        assert_eq!(parse_device_code(&paired).unwrap().device_id.as_deref(), Some("dev_9"));
    }

    #[test]
    fn parses_a_payment_with_no_refund_block() {
        let v = serde_json::json!({
            "id": "pay_1", "status": "COMPLETED",
            "amount_money": { "amount": 100, "currency": "USD" },
            "created_at": "2026-07-27T10:00:00Z"
        });
        let p = parse_payment(&v).unwrap();
        assert_eq!(p.refunded_cents, 0);
        assert!(!p.is_refunded());
    }
}

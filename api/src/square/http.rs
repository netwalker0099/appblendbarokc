use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde_json::{json, Value};

use super::{
    CheckoutHandle, CheckoutPush, CustomerPush, RemotePayment, Square, SquareConfig, SquareError,
};

/// Pinned Square API version. Square dates its API and keeps old versions working;
/// pinning means a Square-side release cannot change response shapes under us.
/// Bump deliberately, after reading their changelog — never leave this unset.
const SQUARE_VERSION: &str = "2025-01-23";
const USER_AGENT: &str = "blendbar-app/0.2";

/// Guard against an unbounded pagination loop if a cursor ever fails to advance.
/// 100 pages x 100 payments is far more than a season of stand sales.
const MAX_PAGES: usize = 100;

/// Live Square client.
///
/// ⚠️ The request/response shapes here follow Square's documented Connect v2 API
/// but have NOT been exercised against the live service from this box — no
/// credentials exist here yet. Before taking real money, run the checklist in
/// README ("Going live on Square"): point it at Square **Sandbox** first, take a
/// test payment with a Square test card, and confirm the reconciliation report
/// matches the Square dashboard. The mock backend covers the local logic; only
/// the wire format needs that confirmation.
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
            // Our cart id. This is the anchor for reconciliation: given any
            // Square order we can name the local cart that produced it.
            "reference_id": push.cart_id.to_string(),
            "line_items": line_items,
        });
        if let Some(note) = &push.note {
            order["note"] = json!(note);
        }

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
        if let Some(url) = redirect {
            body["checkout_options"] = json!({
                "redirect_url": url,
                "ask_for_shipping_address": false,
            });
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

    async fn list_payments(
        &self,
        begin: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<RemotePayment>, SquareError> {
        let mut out = Vec::new();
        let mut cursor: Option<String> = None;

        for page in 0..MAX_PAGES {
            let mut path = format!(
                "/v2/payments?begin_time={}&end_time={}&location_id={}&limit=100",
                urlencoding(&begin.to_rfc3339()),
                urlencoding(&end.to_rfc3339()),
                urlencoding(&self.config.location_id),
            );
            if let Some(c) = &cursor {
                path.push_str(&format!("&cursor={}", urlencoding(c)));
            }

            let resp = self.get(&path).await?;

            if let Some(payments) = resp.get("payments").and_then(Value::as_array) {
                for p in payments {
                    out.push(parse_payment(p)?);
                }
            }

            match resp.get("cursor").and_then(Value::as_str) {
                Some(next) if !next.is_empty() => cursor = Some(next.to_string()),
                // No cursor means this was the last page — the normal exit.
                _ => return Ok(out),
            }

            if page == MAX_PAGES - 1 {
                // Truncating silently would understate Square's side and invent
                // "missing in Square" discrepancies. Refuse instead.
                return Err(SquareError::Config(format!(
                    "list_payments exceeded {MAX_PAGES} pages for {begin}..{end}; \
                     narrow the reconciliation window"
                )));
            }
        }
        Ok(out)
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

/// Minimal percent-encoding for query values (timestamps contain `+` and `:`,
/// cursors are opaque base64-ish blobs). Avoids pulling in a dependency for the
/// handful of characters that actually occur here.
fn urlencoding(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urlencodes_timestamp_reserved_characters() {
        // A naive format! would leave `+` in, which a server reads as a space —
        // silently shifting the reconciliation window by the UTC offset.
        assert_eq!(
            urlencoding("2026-07-27T10:00:00+00:00"),
            "2026-07-27T10%3A00%3A00%2B00%3A00"
        );
        assert_eq!(urlencoding("abc-123_x.y~z"), "abc-123_x.y~z");
    }

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

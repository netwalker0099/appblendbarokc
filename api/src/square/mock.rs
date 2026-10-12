use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use chrono::Utc;

use super::{
    CheckoutHandle, CheckoutPush, CustomerPush, DeviceCode, RemotePayment, Square, SquareError,
    TerminalCheckout, TerminalCheckoutPush,
};

/// In-process stand-in for Square, used whenever credentials are absent.
///
/// Unlike the old Squarespace mock (which returned a bare id and forgot), this
/// one keeps state: a checkout records a COMPLETED payment for the cart's total,
/// as though the customer paid the moment the link was issued. That makes the
/// whole downstream path — refresh-from-Square and settlement — exercisable end
/// to end on a box with no credentials.
///
/// Terminal checkouts start PENDING and complete on the first status check, so
/// the "waiting for the customer" screen is seen at least once. Device codes
/// pair on the first check, the same way.
///
/// State is per-process and lost on restart.
#[derive(Default)]
pub struct MockSquare {
    payments: Mutex<HashMap<String, RemotePayment>>,
    /// Terminal checkouts by id, with the push that created each.
    terminal: Mutex<HashMap<String, (TerminalCheckout, TerminalCheckoutPush)>>,
    codes: Mutex<HashMap<String, DeviceCode>>,
}

impl MockSquare {
    fn record(&self, payment: RemotePayment) {
        self.payments
            .lock()
            .expect("mock payment store poisoned")
            .insert(payment.payment_id.clone(), payment);
    }
}

#[async_trait]
impl Square for MockSquare {
    fn name(&self) -> &'static str {
        "mock"
    }

    fn is_live(&self) -> bool {
        false
    }

    async fn create_checkout(&self, push: &CheckoutPush) -> Result<CheckoutHandle, SquareError> {
        if push.line_items.is_empty() {
            return Err(SquareError::Config("cannot check out an empty cart".into()));
        }

        let gross: i64 = push
            .line_items
            .iter()
            .map(|li| li.unit_amount_cents * li.quantity as i64)
            .sum();
        let discounted: i64 = push.discounts.iter().map(|d| d.amount_cents).sum();
        // Mirrors the real thing: the customer is charged net of discounts.
        let total = (gross - discounted).max(0);

        // Deterministic ids derived from the cart, so re-running a checkout is
        // stable and the ids are obviously fake in logs and the admin UI.
        let short = push.cart_id.simple().to_string();
        let square_order_id = format!("mock_ord_{short}");
        let payment_id = format!("mock_pay_{short}");

        self.record(RemotePayment {
            payment_id: payment_id.clone(),
            square_order_id: Some(square_order_id.clone()),
            status: "COMPLETED".to_string(),
            amount_cents: total,
            currency: push.currency.clone(),
            created_at: Utc::now(),
            refunded_cents: 0,
        });

        tracing::info!(
            cart_id = %push.cart_id,
            total_cents = total,
            lines = push.line_items.len(),
            "[mock square] create_checkout — no real payment link was created"
        );

        Ok(CheckoutHandle {
            payment_link_id: format!("mock_link_{short}"),
            square_order_id,
            // Deliberately not a working URL. A mock checkout must never look
            // like a real one to whoever is holding the tablet.
            url: format!("https://example.invalid/mock-checkout/{short}"),
        })
    }

    async fn get_payment(&self, payment_id: &str) -> Result<RemotePayment, SquareError> {
        self.payments
            .lock()
            .expect("mock payment store poisoned")
            .get(payment_id)
            .cloned()
            .ok_or_else(|| SquareError::Config(format!("mock: unknown payment {payment_id}")))
    }

    async fn find_payment_for_order(
        &self,
        square_order_id: &str,
    ) -> Result<Option<RemotePayment>, SquareError> {
        Ok(self
            .payments
            .lock()
            .expect("mock payment store poisoned")
            .values()
            .find(|p| p.square_order_id.as_deref() == Some(square_order_id))
            .cloned())
    }

    async fn create_order(&self, push: &CheckoutPush) -> Result<String, SquareError> {
        if push.line_items.is_empty() {
            return Err(SquareError::Config("cannot check out an empty cart".into()));
        }
        tracing::info!(cart_id = %push.cart_id, "[mock square] create_order");
        Ok(format!("mock_ord_{}", push.cart_id.simple()))
    }

    async fn create_terminal_checkout(
        &self,
        push: &TerminalCheckoutPush,
    ) -> Result<TerminalCheckout, SquareError> {
        let mut store = self.terminal.lock().expect("mock terminal store poisoned");
        // Same key, same checkout — as Square's idempotency does.
        let id = format!("mock_tc_{}", push.idempotency_key.replace('-', ""));
        if let Some((existing, _)) = store.get(&id) {
            return Ok(existing.clone());
        }
        let checkout = TerminalCheckout {
            id: id.clone(),
            status: "PENDING".into(),
            payment_ids: Vec::new(),
            cancel_reason: None,
        };
        store.insert(id, (checkout.clone(), push.clone()));
        tracing::info!(
            device_id = %push.device_id,
            amount_cents = push.amount_cents,
            "[mock square] create_terminal_checkout — no Terminal was contacted"
        );
        Ok(checkout)
    }

    async fn get_terminal_checkout(
        &self,
        checkout_id: &str,
    ) -> Result<TerminalCheckout, SquareError> {
        let mut store = self.terminal.lock().expect("mock terminal store poisoned");
        let (checkout, push) = store
            .get_mut(checkout_id)
            .ok_or_else(|| SquareError::Config(format!("mock: unknown checkout {checkout_id}")))?;
        if checkout.status == "PENDING" {
            // The pretend customer pays the moment anyone looks.
            let payment_id = format!("mock_pay_{}", checkout_id.trim_start_matches("mock_tc_"));
            self.record(RemotePayment {
                payment_id: payment_id.clone(),
                square_order_id: Some(push.square_order_id.clone()),
                status: "COMPLETED".into(),
                amount_cents: push.amount_cents,
                currency: push.currency.clone(),
                created_at: Utc::now(),
                refunded_cents: 0,
            });
            checkout.status = "COMPLETED".into();
            checkout.payment_ids = vec![payment_id];
        }
        Ok(checkout.clone())
    }

    async fn cancel_terminal_checkout(
        &self,
        checkout_id: &str,
    ) -> Result<TerminalCheckout, SquareError> {
        let mut store = self.terminal.lock().expect("mock terminal store poisoned");
        let (checkout, _) = store
            .get_mut(checkout_id)
            .ok_or_else(|| SquareError::Config(format!("mock: unknown checkout {checkout_id}")))?;
        if !checkout.is_active() {
            return Err(SquareError::Api {
                status: 400,
                body: format!("BAD_REQUEST: checkout is {}", checkout.status),
                retryable: false,
            });
        }
        checkout.status = "CANCELED".into();
        checkout.cancel_reason = Some("SELLER_CANCELED".into());
        Ok(checkout.clone())
    }

    async fn create_device_code(&self, name: &str) -> Result<DeviceCode, SquareError> {
        let mut codes = self.codes.lock().expect("mock code store poisoned");
        let n = codes.len() + 1;
        let code = DeviceCode {
            id: format!("mock_dc_{n}"),
            code: format!("MOCK{n:02}"),
            status: "UNPAIRED".into(),
            device_id: None,
            pair_by: Some(Utc::now() + chrono::Duration::minutes(5)),
        };
        codes.insert(code.id.clone(), code.clone());
        tracing::info!(name, "[mock square] create_device_code");
        Ok(code)
    }

    async fn get_device_code(&self, code_id: &str) -> Result<DeviceCode, SquareError> {
        let mut codes = self.codes.lock().expect("mock code store poisoned");
        let code = codes
            .get_mut(code_id)
            .ok_or_else(|| SquareError::Config(format!("mock: unknown device code {code_id}")))?;
        if code.status == "UNPAIRED" {
            code.status = "PAIRED".into();
            code.device_id = Some(format!("mock-terminal-{}", code_id.trim_start_matches("mock_dc_")));
        }
        Ok(code.clone())
    }

    async fn void_checkout(&self, payment_link_id: &str) -> Result<(), SquareError> {
        // Mirror the real thing: drop the payment this link would have settled, so
        // a canceled cart stops showing up as paid in mock reconciliation too.
        let short = payment_link_id.strip_prefix("mock_link_").unwrap_or("");
        self.payments
            .lock()
            .expect("mock payment store poisoned")
            .remove(&format!("mock_pay_{short}"));
        tracing::info!(payment_link_id, "[mock square] void_checkout");
        Ok(())
    }

    async fn upsert_customer(&self, customer: &CustomerPush) -> Result<String, SquareError> {
        tracing::info!(
            email = %customer.email,
            marketing_consent = customer.marketing_consent,
            "[mock square] upsert_customer"
        );
        Ok(format!("mock_cus_{}", customer.id.simple()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::square::LineItemPush;
    use uuid::Uuid;

    fn push(cart_id: Uuid, cents: i64, qty: i32) -> CheckoutPush {
        CheckoutPush {
            cart_id,
            idempotency_key: cart_id.to_string(),
            currency: "USD".into(),
            buyer_email: Some("guest@example.com".into()),
            line_items: vec![LineItemPush {
                name: "Custom mix (3.4 oz)".into(),
                quantity: qty,
                unit_amount_cents: cents,
            }],
            discounts: Vec::new(),
            redirect_url: None,
            note: None,
            ask_for_shipping_address: false,
        }
    }

    #[tokio::test]
    async fn checkout_records_a_findable_payment_for_the_cart_total() {
        let sq = MockSquare::default();
        let cart_id = Uuid::new_v4();

        let handle = sq.create_checkout(&push(cart_id, 6000, 2)).await.unwrap();

        let found = sq
            .find_payment_for_order(&handle.square_order_id)
            .await
            .unwrap()
            .expect("payment should exist for the order");
        assert_eq!(found.amount_cents, 12000, "quantity must multiply through");
        assert!(found.is_completed());
    }

    #[tokio::test]
    async fn repeated_checkout_is_stable_and_does_not_duplicate() {
        let sq = MockSquare::default();
        let cart_id = Uuid::new_v4();

        let a = sq.create_checkout(&push(cart_id, 6000, 1)).await.unwrap();
        let b = sq.create_checkout(&push(cart_id, 6000, 1)).await.unwrap();

        assert_eq!(a.square_order_id, b.square_order_id);
        assert_eq!(
            sq.payments.lock().unwrap().len(),
            1,
            "same cart must not create a second payment"
        );
    }

    fn terminal_push(order: &str, key: &str) -> TerminalCheckoutPush {
        TerminalCheckoutPush {
            idempotency_key: key.into(),
            square_order_id: order.into(),
            amount_cents: 2500,
            currency: "USD".into(),
            device_id: "mock-terminal-1".into(),
            reference_id: "cart".into(),
            note: None,
        }
    }

    #[tokio::test]
    async fn terminal_checkout_pays_the_order_it_was_sent_for() {
        let sq = MockSquare::default();
        let order = sq.create_order(&push(Uuid::new_v4(), 2500, 1)).await.unwrap();
        let tc = sq.create_terminal_checkout(&terminal_push(&order, "k1")).await.unwrap();
        assert_eq!(tc.status, "PENDING");

        let done = sq.get_terminal_checkout(&tc.id).await.unwrap();
        assert_eq!(done.status, "COMPLETED");
        let payment = sq.get_payment(&done.payment_ids[0]).await.unwrap();
        assert_eq!(payment.square_order_id.as_deref(), Some(order.as_str()));
        assert_eq!(payment.amount_cents, 2500);
    }

    #[tokio::test]
    async fn terminal_checkout_is_idempotent_and_cancelable_only_while_active() {
        let sq = MockSquare::default();
        let a = sq.create_terminal_checkout(&terminal_push("o", "same")).await.unwrap();
        let b = sq.create_terminal_checkout(&terminal_push("o", "same")).await.unwrap();
        assert_eq!(a.id, b.id, "a retried send must not put a second checkout on the device");

        let canceled = sq.cancel_terminal_checkout(&a.id).await.unwrap();
        assert_eq!(canceled.status, "CANCELED");
        assert!(sq.cancel_terminal_checkout(&a.id).await.is_err());
        // A canceled checkout stays canceled — looking at it doesn't pay it.
        assert_eq!(sq.get_terminal_checkout(&a.id).await.unwrap().status, "CANCELED");
    }

    #[tokio::test]
    async fn device_code_pairs_on_first_check() {
        let sq = MockSquare::default();
        let code = sq.create_device_code("Bar").await.unwrap();
        assert_eq!(code.status, "UNPAIRED");
        let paired = sq.get_device_code(&code.id).await.unwrap();
        assert_eq!(paired.status, "PAIRED");
        assert!(paired.device_id.is_some());
    }

    #[tokio::test]
    async fn voiding_a_checkout_makes_it_unpayable() {
        // The guarantee that matters: after a cart is canceled its link must not
        // be able to settle, or money could land against blends already re-sold.
        let sq = MockSquare::default();
        let cart_id = Uuid::new_v4();
        let handle = sq.create_checkout(&push(cart_id, 6000, 1)).await.unwrap();

        sq.void_checkout(&handle.payment_link_id).await.unwrap();

        assert!(sq
            .find_payment_for_order(&handle.square_order_id)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn voiding_twice_is_not_an_error() {
        // Cancel is retried by the expiry sweep; a second void must not fail.
        let sq = MockSquare::default();
        let handle = sq
            .create_checkout(&push(Uuid::new_v4(), 100, 1))
            .await
            .unwrap();
        sq.void_checkout(&handle.payment_link_id).await.unwrap();
        assert!(sq.void_checkout(&handle.payment_link_id).await.is_ok());
    }

    #[tokio::test]
    async fn empty_cart_is_refused() {
        let sq = MockSquare::default();
        let mut p = push(Uuid::new_v4(), 100, 1);
        p.line_items.clear();
        assert!(sq.create_checkout(&p).await.is_err());
    }
}

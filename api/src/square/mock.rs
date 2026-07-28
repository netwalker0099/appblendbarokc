use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use super::{
    CheckoutHandle, CheckoutPush, CustomerPush, RemotePayment, Square, SquareError,
};

/// In-process stand-in for Square, used whenever credentials are absent.
///
/// Unlike the old Squarespace mock (which returned a bare id and forgot), this
/// one keeps state: a checkout records a COMPLETED payment for the cart's total,
/// as though the customer paid the moment the link was issued. That makes the
/// whole downstream path — refresh-from-Square, the reconciliation report, the
/// matched/mismatched buckets — exercisable end to end on a box with no
/// credentials, which is the only way any of it gets tested before go-live.
///
/// State is per-process and lost on restart. That is fine for its purpose and
/// worth stating plainly: after an API restart the mock will report payments it
/// previously invented as missing, so reconciliation in mock mode is a logic
/// test, never a financial one.
#[derive(Default)]
pub struct MockSquare {
    payments: Mutex<HashMap<String, RemotePayment>>,
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

    async fn list_payments(
        &self,
        begin: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<RemotePayment>, SquareError> {
        Ok(self
            .payments
            .lock()
            .expect("mock payment store poisoned")
            .values()
            .filter(|p| p.created_at >= begin && p.created_at <= end)
            .cloned()
            .collect())
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
        let all = sq
            .list_payments(Utc::now() - chrono::Duration::hours(1), Utc::now())
            .await
            .unwrap();
        assert_eq!(all.len(), 1, "same cart must not create a second payment");
    }

    #[tokio::test]
    async fn list_payments_respects_the_window() {
        let sq = MockSquare::default();
        sq.create_checkout(&push(Uuid::new_v4(), 100, 1))
            .await
            .unwrap();

        let past_start = Utc::now() - chrono::Duration::days(10);
        let past_end = Utc::now() - chrono::Duration::days(9);
        assert!(sq.list_payments(past_start, past_end).await.unwrap().is_empty());
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

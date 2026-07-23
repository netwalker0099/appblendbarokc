use async_trait::async_trait;

use super::{ContactPush, OrderPush, RemoteOrder, Squarespace, SyncError};

/// In-process stand-in for Squarespace. Returns a deterministic id derived from
/// the entity's uuid (so re-runs are stable and the written-back id is
/// recognisable as a mock), and logs the call. No network, never fails.
#[derive(Default)]
pub struct MockSquarespace;

#[async_trait]
impl Squarespace for MockSquarespace {
    fn name(&self) -> &'static str {
        "mock"
    }

    async fn upsert_contact(&self, contact: &ContactPush) -> Result<String, SyncError> {
        tracing::info!(
            email = %contact.email,
            marketing_consent = contact.marketing_consent,
            "[mock squarespace] upsert_contact"
        );
        Ok(format!("mock_contact_{}", contact.id.simple()))
    }

    async fn create_order(&self, order: &OrderPush) -> Result<String, SyncError> {
        tracing::info!(
            order_id = %order.id,
            email = %order.email,
            amount = ?order.amount,
            "[mock squarespace] create_order ({})",
            order.description
        );
        Ok(format!("mock_order_{}", order.id.simple()))
    }

    async fn get_order(&self, squarespace_order_id: &str) -> Result<RemoteOrder, SyncError> {
        // A stand order is taken through the Squarespace POS, so the mock reports
        // it as paid and pending fulfilment — enough to drive reconciliation from
        // a local 'lead'/'paid' order to 'paid'. Never fails.
        tracing::info!(order_id = %squarespace_order_id, "[mock squarespace] get_order");
        Ok(RemoteOrder {
            fulfillment_status: "PENDING".to_string(),
            paid: true,
            grand_total: None,
        })
    }
}

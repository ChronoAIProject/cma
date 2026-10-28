//! Persist exact delivery receipts before acknowledging a provider cursor.
use super::{
    delivery_state::DeliveryStatus,
    storage,
    tool_reply_store::{ReplyClaim, ReplyStore},
    ExecutionError, Lease, RoutedDelivery,
};
use crate::delivery::{
    intents::{DeliveryOutcome, IntentReceipt},
    retained_receipt::INLINE_LIMIT,
};

impl RoutedDelivery {
    pub(super) async fn record_receipt(
        &self,
        lease: &mut Lease,
        operation: &str,
        automatic: Option<&ReplyClaim>,
        receipt: Option<&IntentReceipt>,
    ) -> Result<bool, ExecutionError> {
        let stored = match (automatic, receipt) {
            (Some(claim), Some(receipt)) => Some(
                ReplyStore(self.journal.clone())
                    .finish(claim, receipt)
                    .await?,
            ),
            (None, Some(receipt)) => Some(
                self.journal
                    .receipts(operation, &lease.state.owner, lease.state.retained_until)
                    .retain(receipt)
                    .await
                    .map_err(|_| ExecutionError::Unavailable)?,
            ),
            (_, None) => None,
        };
        let unresolved = receipt.is_some_and(|value| {
            matches!(value.delivery, DeliveryOutcome::Unknown { .. })
                || (value.message_parts.is_empty()
                    && storage::bytes(value).map_or(true, |bytes| bytes.len() > INLINE_LIMIT))
        });
        let status = if unresolved {
            match stored.filter(|value| value.has_parts()) {
                Some(receipt) => DeliveryStatus::Partial { receipt },
                None => DeliveryStatus::Unknown,
            }
        } else {
            DeliveryStatus::Settled { receipt: stored }
        };
        self.record(lease, status).await?;
        Ok(!unresolved)
    }
}

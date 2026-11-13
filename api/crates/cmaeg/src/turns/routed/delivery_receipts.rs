//! Persist exact delivery receipts before acknowledging a provider cursor.
use super::{
    delivery_state::DeliveryStatus,
    dispatcher::{admission_error, current_target},
    storage,
    tool_reply_store::{ReplyClaim, ReplyStore},
    ExecutionError, FrozenTurn, Lease, RoutedDelivery, ToolReplyStatus,
};
use crate::delivery::{
    intents::{DeliveryOutcome, IntentReceipt},
    retained_receipt::INLINE_LIMIT,
};

use crate::ingress::routing_store::EventIdentity;

impl RoutedDelivery {
    pub async fn tool_reply_status(
        &self,
        identity: &EventIdentity,
        target_id: &str,
        lease: &Lease,
        body: &FrozenTurn,
    ) -> Result<ToolReplyStatus, ExecutionError> {
        let plan = self
            .plans
            .load(identity)
            .await
            .map_err(admission_error)?
            .ok_or(ExecutionError::Invalid)?;
        let target = plan
            .targets
            .iter()
            .find(|target| target.id == target_id)
            .ok_or(ExecutionError::Invalid)?;
        if lease.state.id != target.id
            || lease.state.root != plan.admission_id
            || lease.state.owner != plan.identity.owner
            || lease.state.request_hash != plan.request_hash
        {
            return Err(ExecutionError::Invalid);
        }
        self.journal.guard(lease).await?;
        body.validate_settlement(&plan, target, &self.journal.keys, (self.journal.clock)())?;
        current_target(self.authority.as_ref(), &plan, target).await?;
        let status = ReplyStore(self.journal.clone())
            .status(&plan, target)
            .await?;
        self.journal.guard(lease).await?;
        Ok(status)
    }

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

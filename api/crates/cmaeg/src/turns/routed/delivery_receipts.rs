//! Persist exact delivery receipts before acknowledging a provider cursor.
use super::{
    answer_dispatch::AnswerDispatch,
    answer_identity::FrameOperation,
    delivery_state::DeliveryStatus,
    dispatcher::{admission_error, current_target},
    storage,
    tool_reply_store::{ReplyClaim, ReplyStore},
    ExecutionError, FrozenTurn, Lease, ReplayEffect, RoutedDelivery, ToolReplyStatus,
};
use crate::delivery::{
    intents::{DeliveryOutcome, IntentReceipt},
    retained_receipt::{StoredReceipt, INLINE_LIMIT},
};

use crate::{
    ingress::routing_store::{AdmittedPlan, EventIdentity, StoredTarget},
    providers::{OutputItem, RecoveryEvent, ResponsesEvent},
    turns::observer::{DeliveryStep, Destination},
};

pub(super) fn answer_response_id<'a>(
    frame: &'a RecoveryEvent,
    effect: &ReplayEffect,
) -> Option<&'a str> {
    let ReplayEffect::Reply(message) = effect else {
        return None;
    };
    (matches!(
        frame.event,
        ResponsesEvent::Completed { .. }
            | ResponsesEvent::OutputTextDone { .. }
            | ResponsesEvent::OutputItemDone {
                item: OutputItem::Message { .. },
                ..
            }
    ) && message
        .text
        .as_ref()
        .is_some_and(|text| !text.trim().is_empty()))
    .then_some(frame.response_id.as_str())
}

impl RoutedDelivery {
    pub(super) async fn recover_automatic_receipt(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        lease: &mut Lease,
        operation: &FrameOperation,
    ) -> Result<Option<DeliveryStatus>, ExecutionError> {
        let store = ReplyStore(self.journal.clone(), lease.partition.clone());
        let Some(stored) = operation.recover(&store, plan, target).await? else {
            return Ok(None);
        };
        let receipts = self.journal.receipts(
            &operation.id,
            &lease.state.owner,
            lease.state.retained_until,
        );
        let mut receipt = receipts
            .load(&stored)
            .await
            .map_err(|_| ExecutionError::Unavailable)?;
        let stored = match normalize_partial_refusal(&receipt) {
            Some(normalized) => {
                receipt = normalized;
                receipts
                    .retain(&receipt)
                    .await
                    .map_err(|_| ExecutionError::Unavailable)?
            }
            None => stored,
        };
        // The reply guard can settle before the cursor checkpoint is written.
        // The operation already binds the target, response and exact effect.
        let status = receipt_status(Some(stored), Some(&receipt));
        self.record(lease, status.clone()).await?;
        Ok(Some(status))
    }

    pub(super) async fn observe_replayed_answer(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        body: &FrozenTurn,
        response_id: &str,
        receipt: &IntentReceipt,
    ) {
        if !matches!(&receipt.delivery, DeliveryOutcome::Confirmed { platform_message_id: Some(id) } if !id.is_empty())
        {
            return;
        }
        let event = super::model::delivery_event(plan, target);
        let default = body.delivery.default_destination.as_ref();
        let change = crate::activity::facts::delivery(
            target.launch.consumer.as_str(),
            DeliveryStep::Answer,
            &receipt.delivery,
            if default.is_some() {
                Destination::Default
            } else {
                Destination::Origin
            },
            &event.reply,
            default,
        )
        .join_retained_answer(response_id);
        self.journal.observe(&plan.identity, change).await;
    }

    pub(super) async fn retained_reply_observation(
        &self,
        identity: &EventIdentity,
        target_id: &str,
    ) -> Result<(ToolReplyStatus, Option<i64>), ExecutionError> {
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
        let store = ReplyStore(self.journal.clone(), None);
        store.observation(&plan, target).await
    }

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
        let status = ReplyStore(self.journal.clone(), lease.partition.clone())
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
        dispatch: AnswerDispatch,
    ) -> Result<bool, ExecutionError> {
        let normalized = receipt.and_then(normalize_partial_refusal);
        let receipt = normalized.as_ref().or(receipt);
        let stored = match (automatic, receipt) {
            (Some(claim), Some(receipt)) => Some(
                ReplyStore(self.journal.clone(), lease.partition.clone())
                    .finish_observed(claim, receipt, dispatch)
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
        let status = receipt_status(stored, receipt);
        let settled = status.settled();
        self.record(lease, status).await?;
        Ok(settled)
    }
}

fn normalize_partial_refusal(receipt: &IntentReceipt) -> Option<IntentReceipt> {
    if receipt.message_parts.is_empty()
        || matches!(
            receipt.delivery,
            DeliveryOutcome::Confirmed { .. } | DeliveryOutcome::Unknown { .. }
        )
    {
        return None;
    }
    // Older checkpoint readers accept Unknown for an incomplete multipart send.
    let mut normalized = receipt.clone();
    normalized.delivery = DeliveryOutcome::Unknown {
        reason: "partial_delivery".into(),
    };
    Some(normalized)
}

fn receipt_status(
    stored: Option<StoredReceipt>,
    receipt: Option<&IntentReceipt>,
) -> DeliveryStatus {
    let unresolved = stored
        .as_ref()
        .is_some_and(StoredReceipt::requires_reconciliation)
        || receipt.is_some_and(|value| {
            value.message_parts.is_empty()
                && storage::bytes(value).map_or(true, |bytes| bytes.len() > INLINE_LIMIT)
        });
    if !unresolved {
        return DeliveryStatus::Settled { receipt: stored };
    }
    match stored.filter(StoredReceipt::has_parts) {
        Some(receipt) => DeliveryStatus::Partial { receipt },
        None => DeliveryStatus::Unknown,
    }
}

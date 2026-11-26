//! The operation of one provider-committed primary answer.
use super::{
    journal::valid_response,
    storage,
    tool_reply_store::{Admission, Kind, ReplyStore},
    DeliveryCheckpoint, ExecutionError, ReplayEffect,
};
use crate::{
    delivery::retained_receipt::StoredReceipt,
    ingress::routing_store::{AdmittedPlan, StoredTarget},
    providers::{OutputItem, RecoveryEvent, ResponsesEvent},
};
use cmaeg_core::{channel::ReplyChoice, message::OutboundMessage};
use serde::{Deserialize, Serialize};

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AcceptedAnswerIdentity {
    pub response_id: String,
    pub item_id: String,
}

impl AcceptedAnswerIdentity {
    pub(super) fn from_frame(
        frame: &RecoveryEvent,
        enabled: bool,
    ) -> Result<Option<Self>, ExecutionError> {
        let Some(marker) = frame.raw["item"].get("cmaeg_delivery") else {
            return Ok(None);
        };
        if !enabled || marker.as_str() != Some("primary_v1") {
            return Err(ExecutionError::Unsupported);
        }
        let ResponsesEvent::OutputItemDone {
            item: OutputItem::Message { id, .. },
            ..
        } = &frame.event
        else {
            return Err(ExecutionError::Invalid);
        };
        if frame.raw["item"]["role"].as_str() != Some("assistant")
            || !valid_response(&frame.response_id)
            || !valid_response(id)
        {
            return Err(ExecutionError::Invalid);
        }
        Ok(Some(Self {
            response_id: frame.response_id.clone(),
            item_id: id.clone(),
        }))
    }

    /// One routed target has one primary answer. Its response, item and message
    /// bind the slot without letting conflicting input allocate another row.
    pub(super) fn operation(
        &self,
        target: &StoredTarget,
        message: &OutboundMessage,
    ) -> Result<(String, String), ExecutionError> {
        let operation = self.operation_id(&target.id)?;
        let payload = self.content_hash(message)?;
        Ok((operation, payload))
    }

    pub(super) fn operation_id(&self, target_id: &str) -> Result<String, ExecutionError> {
        if !valid_response(&self.response_id) || !valid_response(&self.item_id) {
            return Err(ExecutionError::Invalid);
        }
        Ok(storage::hash(&storage::bytes(&(
            "accepted-answer-delivery-v1",
            target_id,
        ))?))
    }

    pub(super) fn content_hash(&self, message: &OutboundMessage) -> Result<String, ExecutionError> {
        Ok(storage::hash(&storage::bytes(&(
            &self.response_id,
            &self.item_id,
            message,
        ))?))
    }
}

pub(super) struct FrameOperation {
    pub id: String,
    payload: Option<(String, AcceptedAnswerIdentity)>,
}

impl FrameOperation {
    pub(super) fn new(
        target: &StoredTarget,
        frame: &RecoveryEvent,
        effect: &ReplayEffect,
        checkpoint: &DeliveryCheckpoint,
        destination: ReplyChoice,
        accepted_answer_receipts: bool,
    ) -> Result<Self, ExecutionError> {
        let answer = AcceptedAnswerIdentity::from_frame(frame, accepted_answer_receipts)?;
        if let (ReplayEffect::Reply(message), Some(answer)) = (effect, answer) {
            if !matches!(destination, ReplyChoice::Origin) {
                return Err(ExecutionError::Unsupported);
            }
            let (id, payload) = answer.operation(target, message)?;
            return Ok(Self {
                id,
                payload: Some((payload, answer)),
            });
        }
        let id = storage::hash(&storage::bytes(&(
            &target.id,
            &checkpoint.response_id,
            checkpoint.cursor,
            &checkpoint.frame_hash,
            &checkpoint.effect_hash,
        ))?);
        Ok(Self { id, payload: None })
    }

    pub(super) async fn admit(
        &self,
        store: &ReplyStore,
        plan: &AdmittedPlan,
        target: &StoredTarget,
    ) -> Result<Admission, ExecutionError> {
        match &self.payload {
            Some((payload, answer)) => {
                let admitted = store
                    .admit_keyed_answer(
                        plan,
                        target,
                        (self.id.clone(), payload.clone(), Some(answer)),
                    )
                    .await?;
                if matches!(admitted, Admission::Unknown) {
                    store
                        .recover_answer(plan, target, &self.id, payload)
                        .await?;
                }
                Ok(admitted)
            }
            None => {
                store
                    .admit(plan, target, Kind::Automatic, self.id.clone())
                    .await
            }
        }
    }

    pub(super) async fn recover(
        &self,
        store: &ReplyStore,
        plan: &AdmittedPlan,
        target: &StoredTarget,
    ) -> Result<Option<StoredReceipt>, ExecutionError> {
        match &self.payload {
            Some((hash, _)) => store.recover_answer(plan, target, &self.id, hash).await,
            None => store.automatic_receipt(plan, target, &self.id).await,
        }
    }
}

#[cfg(test)]
#[path = "answer_identity_tests.rs"]
mod tests;

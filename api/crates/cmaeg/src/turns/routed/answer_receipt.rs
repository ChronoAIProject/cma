//! Read the terminal delivery fact under the original admitted authority.
use super::{
    answer_dispatch::AnswerDispatch,
    dispatcher::{admission_error, current_target},
    tool_operation::OperationStore,
    tool_reply_store::ReplyStore,
    AcceptedAnswerIdentity, ExecutionError, RoutedToolReplies,
};
use crate::{
    auth::AgentCaller,
    channels::resolver::ResolvedChannel,
    delivery::{intents::DeliveryOutcome, retained_receipt::StoredReceipt},
    ingress::routing_store::EventIdentity,
};
use cmaeg_core::ids::{BindingId, ChannelId, EventId};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AnswerReceiptLookup {
    pub channel_id: ChannelId,
    pub binding_id: BindingId,
    pub event_id: EventId,
    pub response_id: String,
    pub item_id: String,
}

#[derive(Serialize, ToSchema)]
pub struct AnswerReceipt {
    pub answer: AnswerReceiptLookup,
    pub result: AnswerReceiptResult,
}

#[derive(Serialize, ToSchema)]
pub struct AnswerReceiptResult {
    pub delivery_id: String,
    pub settled_at: i64,
    pub dispatch: AnswerDispatch,
    pub outcome: AnswerOutcome,
    pub reason: AnswerReason,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AnswerOutcome {
    Confirmed,
    Rejected,
    Unknown,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AnswerReason {
    PlatformConfirmed,
    RefusedBeforeDispatch,
    PlatformRefused,
    PartialDelivery,
    DeliveryUnconfirmed,
}

impl AnswerReceiptResult {
    pub(super) fn new(
        delivery_id: String,
        receipt: &StoredReceipt,
        dispatch: AnswerDispatch,
        settled_at: i64,
    ) -> Self {
        use AnswerDispatch::{Attempted, Indeterminate, NotDispatched};
        use AnswerOutcome::{Confirmed, Rejected, Unknown};
        use AnswerReason::*;
        let (dispatch, outcome, reason) = match &receipt.delivery {
            DeliveryOutcome::Confirmed {
                platform_message_id: Some(id),
            } if !id.is_empty() => (Attempted, Confirmed, PlatformConfirmed),
            _ if receipt.has_parts() => (Attempted, Unknown, PartialDelivery),
            DeliveryOutcome::Confirmed { .. } => (dispatch, Unknown, DeliveryUnconfirmed),
            _ if dispatch == NotDispatched => (dispatch, Rejected, RefusedBeforeDispatch),
            DeliveryOutcome::Unknown { .. } => (dispatch, Unknown, DeliveryUnconfirmed),
            DeliveryOutcome::Rejected { .. } | DeliveryOutcome::RateLimited { .. }
                if dispatch == Attempted =>
            {
                (dispatch, Rejected, PlatformRefused)
            }
            _ => (Indeterminate, Unknown, DeliveryUnconfirmed),
        };
        Self {
            delivery_id,
            settled_at,
            dispatch,
            outcome,
            reason,
        }
    }
}

impl RoutedToolReplies {
    pub(crate) fn accepted_answer_receipts_enabled(&self) -> bool {
        self.journal.accepted_answer_receipts && self.journal.partitions.is_some()
    }

    pub(crate) async fn accepted_answer_receipt(
        &self,
        caller: AgentCaller,
        channel: &ResolvedChannel,
        request: AnswerReceiptLookup,
    ) -> Result<Option<AnswerReceipt>, ExecutionError> {
        if channel.id != request.channel_id {
            return Err(ExecutionError::AuthorityChanged);
        }
        let consumer = caller
            .consumer(channel)
            .map_err(|_| ExecutionError::AuthorityChanged)?;
        let identity = EventIdentity {
            owner: channel.definition.owner.subject.clone(),
            channel_id: channel.id.clone(),
            record_id: channel.pointer.record_id.clone(),
            event_id: request.event_id.clone(),
        };
        let Some(plan) = self.plans.load(&identity).await.map_err(admission_error)? else {
            return Ok(None);
        };
        let Some(target) = plan
            .targets
            .iter()
            .find(|target| target.launch.consumer == consumer)
        else {
            return Ok(None);
        };
        if target.launch.provider.binding_id != request.binding_id {
            return Err(ExecutionError::AuthorityChanged);
        }
        let current = current_target(self.authority.as_ref(), &plan, target).await?;
        if caller.consumer(&current).ok().as_ref() != Some(&consumer) {
            return Err(ExecutionError::AuthorityChanged);
        }
        let answer = AcceptedAnswerIdentity {
            response_id: request.response_id.clone(),
            item_id: request.item_id.clone(),
        };
        let record = ReplyStore(self.journal.clone(), None)
            .load(&plan, target)
            .await?;
        let result = OperationStore(self.journal.clone())
            .answer(&record, &answer)
            .await?;
        let current = current_target(self.authority.as_ref(), &plan, target).await?;
        if caller.consumer(&current).ok().as_ref() != Some(&consumer) {
            return Err(ExecutionError::AuthorityChanged);
        }
        if plan.retained_until <= (self.journal.clock)() {
            return Err(ExecutionError::Expired);
        }
        Ok(result.map(|result| AnswerReceipt {
            answer: request,
            result,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::super::{
        delivery_tests::{effect, lane},
        test_support::{supported, Setup},
        tool_reply_store::Admission,
        ReplayEffect,
    };
    use super::*;
    use crate::delivery::durable::receipt_for;

    #[tokio::test]
    async fn terminal_lookup_binds_the_original_channel_item_and_current_bearer() {
        let setup = Setup::new(supported()).await;
        let target = setup
            .plan
            .targets
            .iter()
            .find(|target| target.launch.consumer.is_default())
            .unwrap();
        let channel = setup.current.channel.lock().unwrap().clone();
        let caller = AgentCaller {
            key_hash: channel.agent_key_hash,
        };
        let answer = AcceptedAnswerIdentity {
            response_id: "resp_1".into(),
            item_id: "primary_answer".into(),
        };
        let request = |item_id: &str| AnswerReceiptLookup {
            channel_id: channel.id.clone(),
            binding_id: target.launch.provider.binding_id.clone(),
            event_id: setup.plan.identity.event_id.clone(),
            response_id: answer.response_id.clone(),
            item_id: item_id.into(),
        };
        let (delivery, mut effects) = lane(&setup);
        let tools = delivery.tool_replies();
        let store = ReplyStore(setup.journal().clone(), None);
        let ReplayEffect::Reply(message) = effect() else {
            panic!("fixture uses an outbound message");
        };
        let (operation, digest) = answer.operation(target, &message).unwrap();
        let Admission::Claimed(claim) = store
            .admit_keyed_answer(
                &setup.plan,
                target,
                (operation.clone(), digest, Some(&answer)),
            )
            .await
            .unwrap()
        else {
            panic!("expected first reservation");
        };
        assert!(tools
            .accepted_answer_receipt(caller, &channel, request(&answer.item_id))
            .await
            .unwrap()
            .is_none());
        store
            .finish_observed(
                &claim,
                &receipt_for(DeliveryOutcome::Rejected {
                    reason: "private_fixture_detail".into(),
                }),
                AnswerDispatch::NotDispatched,
            )
            .await
            .unwrap();
        let result = tools
            .accepted_answer_receipt(caller, &channel, request(&answer.item_id))
            .await
            .unwrap()
            .unwrap()
            .result;
        let rendered = serde_json::to_value(result).unwrap();
        assert_eq!(rendered["delivery_id"], operation);
        assert_eq!(rendered["dispatch"], "not_dispatched");
        assert_eq!(rendered["outcome"], "rejected");
        assert_eq!(rendered["reason"], "refused_before_dispatch");
        assert!(!rendered.to_string().contains("private_fixture_detail"));
        assert!(matches!(
            tools
                .accepted_answer_receipt(caller, &channel, request("wrong_item"))
                .await,
            Err(ExecutionError::IdempotencyConflict)
        ));
        let mut different = caller;
        different.key_hash[0] ^= 1;
        assert!(matches!(
            tools
                .accepted_answer_receipt(different, &channel, request(&answer.item_id))
                .await,
            Err(ExecutionError::AuthorityChanged)
        ));
        assert!(effects.try_recv().is_err());
    }
}

use super::{
    delivery_gate::SEND_WINDOW,
    dispatcher::{admission_error, current_target},
    model::delivery_event,
    storage,
    tool_reply_gate::{CancelReply, ReplyGate, ToolContext},
    tool_reply_store::{Admission, Kind, ReplyStore},
    Authority, ExecutionError, Journal, Phase, TargetState,
};
use crate::{
    auth::AgentCaller,
    channels::resolver::ResolvedChannel,
    delivery::{
        durable::{receipt_for, DeliveryCoordinator},
        intents::{DeliveryOutcome, IntentReceipt, ReplySource},
    },
    ingress::routing_store::PlanStore,
};
use cmaeg_core::{
    message::OutboundMessage,
    seal::{EventRefPayload, ReplyAddress},
};
use std::sync::Arc;

#[derive(Clone)]
pub struct RoutedToolReplies {
    pub(super) plans: PlanStore,
    pub(super) journal: Journal,
    pub(super) authority: Arc<dyn Authority>,
    pub(super) coordinator: DeliveryCoordinator,
    pub(super) replica_relay: Option<reqwest::Client>,
}
impl RoutedToolReplies {
    /// The sealed original reference is the sole capability. This read returns
    /// no event content and does not acquire or mutate an execution lease.
    pub async fn attest_event(
        &self,
        request: &super::EventAttestationRequest,
    ) -> Result<bool, ExecutionError> {
        super::event_attestation::attest(
            &self.plans,
            &self.journal,
            self.authority.as_ref(),
            request,
        )
        .await
    }

    pub fn new(
        plans: PlanStore,
        journal: Journal,
        authority: Arc<dyn Authority>,
        coordinator: DeliveryCoordinator,
    ) -> Self {
        Self {
            replica_relay: None,
            plans,
            journal,
            authority,
            coordinator,
        }
    }
    /// None means no durable routed admission exists, preserving the live lane.
    /// Any storage/authority error fails closed rather than falling back to it.
    pub async fn reply(
        &self,
        caller: AgentCaller,
        _channel: &ResolvedChannel,
        event: &EventRefPayload,
        message: OutboundMessage,
        delayed: bool,
    ) -> Result<Option<IntentReceipt>, ExecutionError> {
        self.reply_with_idempotency(caller, _channel, event, message, delayed, None)
            .await
    }
    pub async fn reply_with_idempotency(
        &self,
        caller: AgentCaller,
        _channel: &ResolvedChannel,
        event: &EventRefPayload,
        message: OutboundMessage,
        delayed: bool,
        idempotency_key: Option<&str>,
    ) -> Result<Option<IntentReceipt>, ExecutionError> {
        self.reply_inner(caller, event, message, delayed, idempotency_key, None)
            .await
    }
    pub(crate) fn requires_retained_binding(&self) -> bool {
        self.journal.partitions.is_some()
    }

    pub(super) async fn reply_inner(
        &self,
        caller: AgentCaller,
        event: &EventRefPayload,
        message: OutboundMessage,
        delayed: bool,
        idempotency_key: Option<&str>,
        expected: Option<crate::replicas::partition::PartitionFence>,
    ) -> Result<Option<IntentReceipt>, ExecutionError> {
        if message.structured_content.is_some() && (delayed || idempotency_key.is_none()) {
            return Err(ExecutionError::Invalid);
        }
        if idempotency_key.is_some_and(|key| {
            key.is_empty() || key.len() > 128 || !key.bytes().all(|byte| (33..=126).contains(&byte))
        }) {
            return Err(ExecutionError::Invalid);
        }
        let Some(binding) = self.journal.tool_reply_binding(event).await? else {
            return if self.journal.partitions.is_some() {
                Err(ExecutionError::Unsupported)
            } else {
                Ok(None)
            };
        };
        let identity = binding.identity;
        let plan = self
            .plans
            .load(&identity)
            .await
            .map_err(admission_error)?
            .ok_or(ExecutionError::Corrupt)?;
        if plan.admission_id != binding.root || plan.request_hash != binding.request_hash {
            return Err(ExecutionError::Corrupt);
        }
        let consumer = event
            .consumer
            .clone()
            .unwrap_or_else(cmaeg_core::ids::ConsumerId::default_consumer);
        let target = plan
            .targets
            .iter()
            .find(|target| target.id == binding.target && target.launch.consumer == consumer)
            .ok_or(ExecutionError::Invalid)?;
        let expected_event = delivery_event(&plan, target);
        if !same_event(&expected_event, event, delayed) {
            return Err(ExecutionError::Invalid);
        }
        self.journal.require_reply_open(&plan, target).await?;
        let current = current_target(self.authority.as_ref(), &plan, target).await?;
        if caller.consumer(&current).ok().as_ref() != Some(&target.launch.consumer) {
            return Err(ExecutionError::AuthorityChanged);
        }
        // Delayed capabilities may outlive original auto-reply authority. Such
        // sends cannot race an automatic reply, whose original expiry is fixed.
        if event.expires_at <= (self.journal.clock)() {
            return if delayed {
                Ok(None)
            } else {
                Err(ExecutionError::Expired)
            };
        }
        let Some((execution, _)) = self.journal.read::<TargetState>(&target.id).await? else {
            return Err(ExecutionError::Invalid);
        };
        if execution.root != plan.admission_id
            || execution.owner != identity.owner
            || execution.request_hash != plan.request_hash
            || execution.body.is_none()
            || !matches!(
                execution.phase,
                Phase::Starting | Phase::Accepted { .. } | Phase::Completed { .. } | Phase::Unknown
            )
        {
            return Err(ExecutionError::Invalid);
        }
        message.validate().map_err(|_| ExecutionError::Invalid)?;
        // Closed/expired delayed sends are admitted through their separate FIFO
        // effect API. Never fall through to the legacy unfenced send in this mode.
        if delayed && self.journal.partitions.is_some() {
            return Err(ExecutionError::Invalid);
        }
        let operation = match idempotency_key {
            Some(key) => storage::hash(&storage::bytes(&("reply-operation-v1", &target.id, key))?),
            None => storage::hash(&storage::bytes(&(&target.id, &message))?),
        };
        let content_hash = (idempotency_key.is_some() || self.journal.partitions.is_some())
            .then(|| storage::bytes(&message).map(|bytes| storage::hash(&bytes)))
            .transpose()?;
        let retained = ReplyStore(self.journal.clone(), None)
            .replay(&plan, target, &operation, content_hash.as_deref())
            .await?;
        let partition = if retained.is_none() || expected.is_some() {
            match self.journal.capture_partition(&plan, target).await {
                Ok(partition) => partition,
                Err(ExecutionError::Busy | ExecutionError::LostLease)
                    if expected.is_none() && self.replica_relay.is_some() =>
                {
                    return self
                        .forward_reply(caller, event, message, idempotency_key, &plan, target)
                        .await
                        .map(Some);
                }
                Err(error) => return Err(error),
            }
        } else {
            None
        };
        if let Some(expected) = expected {
            if expected.until_ms <= (self.journal.clock)().saturating_mul(1000)
                || !partition
                    .as_ref()
                    .is_some_and(|token| token.matches(&expected))
            {
                return Err(ExecutionError::LostLease);
            }
        }
        let store = ReplyStore(self.journal.clone(), partition);
        let admission = match retained {
            Some(retained) => retained,
            None => match content_hash {
                Some(content_hash) => {
                    store
                        .admit_keyed(&plan, target, operation, content_hash)
                        .await?
                }
                None => store.admit(&plan, target, Kind::Tool, operation).await?,
            },
        };
        let mut claim = match admission {
            Admission::Claimed(claim) => claim,
            Admission::Replay(receipt) => return Ok(Some(receipt)),
            Admission::Pending => return Err(ExecutionError::Busy),
            Admission::Unknown => {
                return Ok(Some(receipt_for(DeliveryOutcome::Unknown {
                    reason: "routed_reply_uncertain".into(),
                })))
            }
            Admission::Suppressed => return Err(ExecutionError::Corrupt),
            // The terminal barrier already excludes all automatic replay and
            // unresolved sends. Long-lived reply-target capabilities keep their
            // independent durable receipt path after that barrier.
            Admission::Closed if delayed => return Ok(None),
            Admission::Closed => return Err(ExecutionError::Invalid),
        };
        let sync = matches!(event.reply, ReplyAddress::Sync { .. });
        let network_only = sync
            && self.coordinator.supports_custom_stream()
            && current
                .definition
                .reply
                .sinks
                .iter()
                .any(|sink| matches!(sink, cmaeg_core::channel::Sink::ResultWebhook { .. }));
        let result = if sync && !network_only {
            receipt_for(DeliveryOutcome::Unsupported)
        } else {
            // A routed tool receipt proves webhook delivery only. The local
            // tool leg is refused in this slice; never enqueue then return Unsupported.
            let mut addressed = event.clone();
            if network_only {
                addressed.reply = ReplyAddress::None;
            }
            let scope = super::live_scope::Scope::new(
                self.journal.clone(),
                super::live_scope::Capture::reply((*claim).clone()),
            );
            let gate = ReplyGate::tool(
                store.clone(),
                &claim,
                ToolContext {
                    authority: self.authority.clone(),
                    plan: plan.clone(),
                    target: target.clone(),
                    channel_version: current.version,
                    caller,
                },
            )
            .with_scope(scope.clone());
            let cancel = CancelReply(gate.clone());
            let result = scope
                .run(
                    "immediate_tool",
                    tokio::time::timeout(
                        SEND_WINDOW,
                        self.coordinator.reply_from(
                            current,
                            addressed,
                            message,
                            gate,
                            ReplySource::Tool,
                        ),
                    ),
                )
                .await?;
            *claim = scope.reply();
            let result = result.unwrap_or_else(|_| {
                receipt_for(DeliveryOutcome::Unknown {
                    reason: "delivery_deadline".into(),
                })
            });
            drop(cancel);
            result
        };
        super::live_scope::receipt(store.finish(&claim, &result)).await?;
        Ok(Some(result))
    }
}
pub(super) fn same_event(
    expected: &EventRefPayload,
    supplied: &EventRefPayload,
    delayed: bool,
) -> bool {
    let mut expected = expected.clone();
    if delayed {
        if let ReplyAddress::NyxidRelay {
            reply_token,
            reply_token_expires_at,
            ..
        } = &mut expected.reply
        {
            *reply_token = cmaeg_core::channel::Secret::new("");
            *reply_token_expires_at = 1;
        }
    }
    &expected == supplied
}

//! Fenced delivery of one already authenticated provider replay frame.
//! Projection chooses the effect; observing a frame never implicitly sends.
use super::{
    custom_delivery::{effect_for, CustomFrame},
    delivery_gate::{CancelSend, DeliveryGate, SEND_WINDOW},
    delivery_receipts::answer_response_id,
    delivery_state::{DeliveryCheckpoint, DeliveryStatus},
    dispatcher::{admission_error, current_target},
    storage,
    tool_reply_gate::{CancelReply, ReplyGate},
    tool_reply_store::{Admission, Kind, ReplyStore},
    Authority, CustomCheckpoint, ExecutionError, Journal, Lease, Phase, RoutedToolReplies,
    ToolReplyStatus,
};
use crate::{
    delivery::{
        durable::{receipt_for, DeliveryCoordinator},
        intents::{DeliveryOutcome, IntentReceipt},
    },
    ingress::routing_store::{AdmittedPlan, EventIdentity, PlanStore, StoredTarget},
    providers::{RecoveryEvent, ResponsesEvent},
};
use cmaeg_core::{channel::ReplyChoice, message::OutboundMessage, seal::ReplyAddress};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "effect", content = "message")]
pub enum ReplayEffect {
    NoEffect,
    Reply(Box<OutboundMessage>),
    Custom(crate::delivery::local_stream::CustomStreamEffect),
}
pub enum FrameDelivery {
    Settled {
        cursor: u64,
        receipt: Option<IntentReceipt>,
    },
    /// No automatic resend or advance is safe; requires explicit reconciliation.
    Unknown,
}
pub struct RoutedDelivery {
    pub(super) stream_relay: Option<reqwest::Client>,
    pub(super) plans: PlanStore,
    pub(super) journal: Journal,
    pub(super) authority: Arc<dyn Authority>,
    pub(super) coordinator: DeliveryCoordinator,
}
impl RoutedDelivery {
    pub fn new(
        plans: PlanStore,
        journal: Journal,
        authority: Arc<dyn Authority>,
        coordinator: DeliveryCoordinator,
    ) -> Self {
        Self {
            stream_relay: None,
            plans,
            journal,
            authority,
            coordinator,
        }
    }

    pub fn tool_replies(&self) -> RoutedToolReplies {
        RoutedToolReplies::new(
            self.plans.clone(),
            self.journal.clone(),
            self.authority.clone(),
            self.coordinator.clone(),
        )
    }

    /// Intent is sealed and CAS-confirmed before enqueue; its actual outcome is
    /// sealed before cursor advancement. An abandoned intent becomes Unknown.
    /// Reusing a settled receipt never sends the same frame twice.
    pub async fn deliver_frame(
        &self,
        identity: &EventIdentity,
        target_id: &str,
        lease: &mut Lease,
        frame: &RecoveryEvent,
        effect: ReplayEffect,
    ) -> Result<FrameDelivery, ExecutionError> {
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
        self.journal.require_reply_open(&plan, target).await?;
        self.journal.guard(lease).await?;
        let current = current_target(self.authority.as_ref(), &plan, target).await?;
        let body = self.journal.body(lease).await?;
        let Phase::Accepted {
            response_id,
            durable_cursor,
        } = &lease.state.phase
        else {
            return Err(ExecutionError::Invalid);
        };
        if response_id != &frame.response_id
            || frame.raw["sequence_number"].as_u64() != Some(frame.sequence_number)
            || frame
                .raw
                .get("response_id")
                .is_some_and(|id| id.as_str() != Some(response_id.as_str()))
        {
            return Err(ExecutionError::Invalid);
        }
        // The SSE decoder bounds raw frames. Persist only their digest, using
        // the same encoding as ReplayProjection, so every accepted frame can
        // be acknowledged even when its snapshot exceeds the effect budget.
        let trace_contract = frame.trace_contract
            || lease.state.delivery.as_ref().is_some_and(|previous| {
                previous.cursor == frame.sequence_number && previous.trace_contract
            });
        let frame_hash = super::replay_frame::hash(frame, trace_contract)?;
        if !super::replay_frame::shape_matches(frame) {
            return Err(ExecutionError::Invalid);
        }
        let effect_hash = bounded_effect_hash(&effect)?;
        let answer_response_id = answer_response_id(frame, &effect);
        let custom_effect = effect_for(&body, frame, &effect)?;
        if custom_effect.is_some() && !self.coordinator.supports_custom_stream() {
            return Err(ExecutionError::Unsupported);
        }
        let checkpoint = DeliveryCheckpoint {
            trace_contract,
            response_id: response_id.clone(),
            cursor: frame.sequence_number,
            frame_hash,
            effect_hash,
            status: DeliveryStatus::Intent,
            execution_attribution: None,
            custom: custom_effect.as_ref().map(|effect| {
                CustomCheckpoint::new(
                    &super::model::delivery_event(&plan, target),
                    &body.delivery.policy.sinks,
                    effect.webhook(),
                )
            }),
            terminal: matches!(
                frame.event,
                ResponsesEvent::Completed { .. }
                    | ResponsesEvent::Failed { .. }
                    | ResponsesEvent::Incomplete { .. }
            ),
        };
        let operation = storage::hash(&storage::bytes(&(
            &target.id,
            &checkpoint.response_id,
            checkpoint.cursor,
            &checkpoint.frame_hash,
            &checkpoint.effect_hash,
        ))?);
        if let Some(previous) = &lease.state.delivery {
            if previous.cursor == checkpoint.cursor {
                if previous.response_id != checkpoint.response_id
                    || previous.frame_hash != checkpoint.frame_hash
                    || previous.effect_hash != checkpoint.effect_hash
                    || previous.terminal != checkpoint.terminal
                {
                    return Err(ExecutionError::Invalid);
                }
                match &previous.status {
                    DeliveryStatus::Settled { receipt } if previous.status.settled() => {
                        let receipt = match receipt {
                            Some(receipt) => Some(
                                self.journal
                                    .receipts(
                                        &operation,
                                        &lease.state.owner,
                                        lease.state.retained_until,
                                    )
                                    .load(receipt)
                                    .await
                                    .map_err(|_| ExecutionError::Unavailable)?,
                            ),
                            None => None,
                        };
                        if checkpoint.terminal && !self.close(&plan, target, lease).await? {
                            return Ok(FrameDelivery::Unknown);
                        }
                        self.journal
                            .acknowledge_frame(&plan, target, lease, frame)
                            .await?;
                        if let (Some(id), Some(receipt)) = (answer_response_id, &receipt) {
                            self.observe_replayed_answer(&plan, target, &body, id, receipt)
                                .await;
                        }
                        return Ok(FrameDelivery::Settled {
                            cursor: frame.sequence_number,
                            receipt,
                        });
                    }
                    DeliveryStatus::Intent => {
                        if let Some(effect) = &custom_effect {
                            body.validate_settlement(
                                &plan,
                                target,
                                &self.journal.keys,
                                (self.journal.clock)(),
                            )?;
                            return self
                                .deliver_custom(
                                    lease,
                                    CustomFrame {
                                        plan: &plan,
                                        target,
                                        frame,
                                        body: &body,
                                        effect,
                                    },
                                    true,
                                )
                                .await;
                        }
                        self.record(lease, DeliveryStatus::Unknown).await?;
                        return Ok(FrameDelivery::Unknown);
                    }
                    _ => return Ok(FrameDelivery::Unknown),
                }
            }
            if !previous.status.settled() {
                return Ok(FrameDelivery::Unknown);
            }
            if previous.terminal {
                return Err(ExecutionError::Invalid);
            }
            if Some(previous.cursor) != *durable_cursor {
                return Err(ExecutionError::Invalid);
            }
        }
        if durable_cursor.map_or(Some(0), |cursor| cursor.checked_add(1))
            != Some(frame.sequence_number)
        {
            return Err(ExecutionError::Invalid);
        }
        let event =
            body.settlement_event(&plan, target, &self.journal.keys, (self.journal.clock)())?;
        // Only a new, validated frame can establish an exact observation time.
        let produced = matches!(frame.event, ResponsesEvent::OutputTextDone { .. })
            || (matches!(frame.event, ResponsesEvent::Completed { .. })
                && matches!(
                    &effect,
                    ReplayEffect::Reply(_)
                        | ReplayEffect::Custom(
                            crate::delivery::local_stream::CustomStreamEffect::Completed {
                                final_text: Some(_)
                            }
                        )
                ));
        if produced {
            self.journal
                .observe(
                    &plan.identity,
                    crate::activity::Change::state(
                        target.launch.consumer.as_str(),
                        crate::activity::ConsumerState::Produced,
                        Some(frame.response_id.clone()),
                    ),
                )
                .await;
        }
        let terminal_state = match frame.event {
            ResponsesEvent::Completed { .. } => Some(crate::activity::ConsumerState::Completed),
            ResponsesEvent::Failed { .. } | ResponsesEvent::Incomplete { .. } => {
                Some(crate::activity::ConsumerState::Failed)
            }
            _ => None,
        };
        if let Some(state) = terminal_state {
            self.journal
                .observe(
                    &plan.identity,
                    crate::activity::Change::state(
                        target.launch.consumer.as_str(),
                        state,
                        Some(frame.response_id.clone()),
                    ),
                )
                .await;
        }
        if let Some(effect) = &custom_effect {
            let mut next = lease.state.clone();
            next.delivery = Some(checkpoint);
            self.journal.commit(lease, next).await?;
            return self
                .deliver_custom(
                    lease,
                    CustomFrame {
                        plan: &plan,
                        target,
                        frame,
                        body: &body,
                        effect,
                    },
                    false,
                )
                .await;
        }
        if let ReplayEffect::Reply(message) = &effect {
            message.validate().map_err(|_| ExecutionError::Invalid)?;
            if body
                .delivery
                .default_destination
                .as_ref()
                .is_some_and(|destination| !destination.authorized(&current))
            {
                return Err(ExecutionError::AuthorityChanged);
            }
        }
        // This shared CAS reservation closes the race between projection's
        // suppression lookup and a concurrently admitted tool reply.
        let store = ReplyStore(self.journal.clone(), lease.partition.clone());
        let mut automatic = None;
        let mut suppressed = false;
        let mut replayed = None;
        if matches!(effect, ReplayEffect::Reply(_)) {
            match store
                .admit(&plan, target, Kind::Automatic, operation.clone())
                .await?
            {
                Admission::Claimed(claim) => automatic = Some(claim),
                Admission::Replay(receipt) => replayed = Some(receipt),
                Admission::Suppressed => suppressed = true,
                Admission::Pending => return Err(ExecutionError::Busy),
                Admission::Unknown => return Ok(FrameDelivery::Unknown),
                Admission::Closed => return Err(ExecutionError::Invalid),
            }
        }
        self.journal.guard(lease).await?;
        let mut next = lease.state.clone();
        next.delivery = Some(checkpoint);
        self.journal.commit(lease, next).await?;
        let receipt = match effect {
            ReplayEffect::Custom(_) => return Err(ExecutionError::Corrupt),
            ReplayEffect::NoEffect => None,
            ReplayEffect::Reply(_) if suppressed => None,
            ReplayEffect::Reply(_) if replayed.is_some() => replayed,
            ReplayEffect::Reply(_) if target.launch.event.expires_at <= (self.journal.clock)() => {
                Some(receipt_for(DeliveryOutcome::Rejected {
                    reason: "event_ref_expired".into(),
                }))
            }
            ReplayEffect::Reply(message) => {
                // Ephemeral Sync waiters have no durable completion receipt.
                if body.delivery.destination == ReplyChoice::Origin
                    && matches!(event.reply, ReplyAddress::Sync { .. })
                {
                    Some(receipt_for(DeliveryOutcome::Unsupported))
                } else {
                    self.journal.renew(lease).await?;
                    let scope = super::live_scope::Scope::new(
                        self.journal.clone(),
                        super::live_scope::Capture::execution(
                            lease.clone(),
                            automatic.as_deref().cloned(),
                        ),
                    );
                    let gate = DeliveryGate::new(
                        self.journal.clone(),
                        lease,
                        self.authority.clone(),
                        plan.clone(),
                        target.clone(),
                        current.version,
                        body.delivery.default_destination.clone(),
                    )
                    .with_scope(scope.clone());
                    let cancel = CancelSend(gate.clone());
                    let reply_gate = ReplyGate::automatic(
                        store.clone(),
                        automatic.as_ref().ok_or(ExecutionError::Corrupt)?,
                        gate,
                        event.expires_at,
                    )
                    .with_scope(scope.clone());
                    let cancel_reply = CancelReply(reply_gate.clone());
                    let destination = body
                        .delivery
                        .default_destination
                        .clone()
                        .map(|destination| (destination, operation.clone()));
                    let send = self.coordinator.reply_for_response(
                        current,
                        event,
                        *message,
                        reply_gate,
                        destination,
                        answer_response_id.map(str::to_owned),
                    );
                    let result = scope
                        .run("automatic_reply", tokio::time::timeout(SEND_WINDOW, send))
                        .await?;
                    *lease = scope.execution();
                    automatic = Some(Box::new(scope.reply()));
                    let result = result.unwrap_or_else(|_| {
                        receipt_for(DeliveryOutcome::Unknown {
                            reason: "delivery_deadline".into(),
                        })
                    });
                    drop(cancel_reply);
                    drop(cancel);
                    Some(result)
                }
            }
        };
        if !super::live_scope::receipt(self.record_receipt(
            lease,
            &operation,
            automatic.as_deref(),
            receipt.as_ref(),
        ))
        .await?
        {
            return Ok(FrameDelivery::Unknown);
        }
        if lease
            .state
            .delivery
            .as_ref()
            .is_some_and(|value| value.terminal)
            && !self.close(&plan, target, lease).await?
        {
            return Ok(FrameDelivery::Unknown);
        }
        self.journal
            .acknowledge_frame(&plan, target, lease, frame)
            .await?;
        Ok(FrameDelivery::Settled {
            cursor: frame.sequence_number,
            receipt,
        })
    }

    pub(super) async fn close(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        lease: &Lease,
    ) -> Result<bool, ExecutionError> {
        match ReplyStore(self.journal.clone(), lease.partition.clone())
            .close(plan, target)
            .await?
        {
            ToolReplyStatus::Pending => Err(ExecutionError::Busy),
            ToolReplyStatus::Unknown => Ok(false),
            ToolReplyStatus::None | ToolReplyStatus::Confirmed => Ok(true),
        }
    }

    pub(super) async fn record(
        &self,
        lease: &mut Lease,
        status: DeliveryStatus,
    ) -> Result<(), ExecutionError> {
        self.journal.guard(lease).await?;
        let mut next = lease.state.clone();
        next.delivery
            .as_mut()
            .ok_or(ExecutionError::Corrupt)?
            .status = status;
        self.journal.commit(lease, next).await
    }
}
fn bounded_effect_hash(effect: &ReplayEffect) -> Result<String, ExecutionError> {
    let bytes = storage::bytes(effect)?;
    if bytes.len() > 256 * 1024 {
        return Err(ExecutionError::Invalid);
    }
    Ok(storage::hash(&bytes))
}

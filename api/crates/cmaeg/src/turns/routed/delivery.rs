//! Fenced delivery of one already authenticated provider replay frame.
//! Projection chooses the effect; observing a frame never implicitly sends.
use super::{
    delivery_gate::{CancelSend, DeliveryGate, SEND_WINDOW},
    delivery_state::{DeliveryCheckpoint, DeliveryStatus},
    dispatcher::{admission_error, current_target},
    storage, Authority, ExecutionError, Journal, Lease, Phase,
};
use crate::{
    delivery::{
        durable::{receipt_for, DeliveryCoordinator},
        intents::{DeliveryOutcome, IntentReceipt},
    },
    ingress::routing_store::{EventIdentity, PlanStore},
    providers::{parse_event, RecoveryEvent, ResponsesEvent, SseFrame},
};
use cmaeg_core::{channel::ReplyChoice, message::OutboundMessage, seal::ReplyAddress};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "effect", content = "message")]
pub enum ReplayEffect {
    NoEffect,
    Reply(OutboundMessage),
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
    plans: PlanStore,
    journal: Journal,
    authority: Arc<dyn Authority>,
    coordinator: DeliveryCoordinator,
}
impl RoutedDelivery {
    pub fn new(
        plans: PlanStore,
        journal: Journal,
        authority: Arc<dyn Authority>,
        coordinator: DeliveryCoordinator,
    ) -> Self {
        Self {
            plans,
            journal,
            authority,
            coordinator,
        }
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
        let frame_hash = bounded_hash(&frame.raw)?;
        if parse_event(&SseFrame {
            event: None,
            id: Some(frame.sequence_number.to_string()),
            data: frame.raw.to_string(),
            retry: None,
        })
        .ok()
        .as_ref()
            != Some(&frame.event)
        {
            return Err(ExecutionError::Invalid);
        }
        let effect_hash = bounded_hash(&effect)?;
        let checkpoint = DeliveryCheckpoint {
            response_id: response_id.clone(),
            cursor: frame.sequence_number,
            frame_hash,
            effect_hash,
            status: DeliveryStatus::Intent,
            terminal: matches!(
                frame.event,
                ResponsesEvent::Completed { .. }
                    | ResponsesEvent::Failed { .. }
                    | ResponsesEvent::Incomplete { .. }
            ),
        };
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
                        let receipt = receipt.clone();
                        self.journal
                            .acknowledge_delivery(lease, &frame.response_id, frame.sequence_number)
                            .await?;
                        return Ok(FrameDelivery::Settled {
                            cursor: frame.sequence_number,
                            receipt,
                        });
                    }
                    DeliveryStatus::Intent => {
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
        body.validate(&plan, target, &self.journal.keys, (self.journal.clock)())?;
        if let ReplayEffect::Reply(message) = &effect {
            message.validate().map_err(|_| ExecutionError::Invalid)?;
        }
        self.journal.guard(lease).await?;
        let mut next = lease.state.clone();
        next.delivery = Some(checkpoint);
        self.journal.commit(lease, next).await?;
        let receipt = match effect {
            ReplayEffect::NoEffect => None,
            ReplayEffect::Reply(message) => {
                let event = body
                    .event_context
                    .event_ref
                    .open(&self.journal.keys, (self.journal.clock)())
                    .map_err(|_| ExecutionError::Expired)?
                    .payload;
                // Ephemeral Sync waiters have no durable completion receipt.
                // Default-destination projection must be implemented explicitly;
                // never silently send a default-target message to the origin.
                if !body.delivery.auto_final
                    || body.delivery.destination != ReplyChoice::Origin
                    || matches!(event.reply, ReplyAddress::Sync { .. })
                {
                    Some(receipt_for(DeliveryOutcome::Unsupported))
                } else {
                    self.journal.renew(lease).await?;
                    let gate = DeliveryGate::new(
                        self.journal.clone(),
                        lease,
                        self.authority.clone(),
                        plan.clone(),
                        target.clone(),
                        current.version,
                    );
                    let cancel = CancelSend(gate.clone());
                    let result = tokio::time::timeout(
                        SEND_WINDOW,
                        self.coordinator.reply(current, event, message, gate),
                    )
                    .await
                    .unwrap_or_else(|_| {
                        receipt_for(DeliveryOutcome::Unknown {
                            reason: "delivery_deadline".into(),
                        })
                    });
                    drop(cancel);
                    Some(result)
                }
            }
        };
        if receipt.as_ref().is_some_and(|value| {
            matches!(value.delivery, DeliveryOutcome::Unknown { .. })
                || storage::bytes(value).map_or(true, |bytes| bytes.len() > 16 * 1024)
        }) {
            self.record(lease, DeliveryStatus::Unknown).await?;
            return Ok(FrameDelivery::Unknown);
        }
        self.record(
            lease,
            DeliveryStatus::Settled {
                receipt: receipt.clone(),
            },
        )
        .await?;
        self.journal
            .acknowledge_delivery(lease, &frame.response_id, frame.sequence_number)
            .await?;
        Ok(FrameDelivery::Settled {
            cursor: frame.sequence_number,
            receipt,
        })
    }

    async fn record(
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
fn bounded_hash(value: &impl Serialize) -> Result<String, ExecutionError> {
    let bytes = storage::bytes(value)?;
    if bytes.len() > 256 * 1024 {
        return Err(ExecutionError::Invalid);
    }
    Ok(storage::hash(&bytes))
}

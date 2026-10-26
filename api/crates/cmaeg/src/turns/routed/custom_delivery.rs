//! Per-leg settlement for an ephemeral primary stream plus frozen result webhooks.
use super::{
    custom_state::{CustomCheckpoint, WebhookStatus},
    delivery_gate::{CancelSend, DeliveryGate, SEND_WINDOW},
    dispatcher::current_target,
    DeliveryStatus, ExecutionError, FrameDelivery, FrozenTurn, Lease, ReplayEffect, RoutedDelivery,
};
use crate::{
    delivery::{
        gate::{SendGate, NO_CHAT_ROUTE},
        intents::DeliveryOutcome,
        local_stream::{webhook_authorized, CustomStreamEffect, LocalStreamOutcome},
    },
    ingress::routing_store::{AdmittedPlan, StoredTarget},
    providers::{RecoveryEvent, ResponsesEvent},
};
use cmaeg_core::{
    activity::SourceKind,
    channel::{ReplyChoice, Sink},
};

pub(super) struct CustomFrame<'a> {
    pub plan: &'a AdmittedPlan,
    pub target: &'a StoredTarget,
    pub frame: &'a RecoveryEvent,
    pub body: &'a FrozenTurn,
    pub effect: &'a CustomStreamEffect,
}
pub(super) fn is_custom_origin(body: &FrozenTurn) -> bool {
    body.event_context.activity.source == SourceKind::Custom
        && body.delivery.destination == ReplyChoice::Origin
        && body
            .delivery
            .policy
            .sinks
            .iter()
            .any(|sink| matches!(sink, Sink::Sync {} | Sink::ResultWebhook { .. }))
}
pub(super) fn effect_for(
    body: &FrozenTurn,
    frame: &RecoveryEvent,
    effect: &ReplayEffect,
) -> Result<Option<CustomStreamEffect>, ExecutionError> {
    if let ReplayEffect::Custom(effect) = effect {
        if !is_custom_origin(body)
            || effect.terminal()
                != matches!(
                    frame.event,
                    ResponsesEvent::Completed { .. }
                        | ResponsesEvent::Failed { .. }
                        | ResponsesEvent::Incomplete { .. }
                )
        {
            return Err(ExecutionError::Invalid);
        }
        return Ok(Some(effect.clone()));
    }
    // A terminal control is independent of whether projection produced an answer.
    if is_custom_origin(body) && matches!(effect, ReplayEffect::NoEffect) {
        return Ok(match frame.event {
            ResponsesEvent::Completed { .. } => {
                Some(CustomStreamEffect::Completed { final_text: None })
            }
            ResponsesEvent::Failed { .. } | ResponsesEvent::Incomplete { .. } => {
                Some(CustomStreamEffect::Failed)
            }
            _ => None,
        });
    }
    Ok(None)
}
impl RoutedDelivery {
    pub(super) async fn deliver_custom(
        &self,
        lease: &mut Lease,
        context: CustomFrame<'_>,
        recovering: bool,
    ) -> Result<FrameDelivery, ExecutionError> {
        let CustomFrame {
            plan,
            target,
            frame,
            body,
            effect,
        } = context;
        let custom = self
            .coordinator
            .custom
            .as_ref()
            .ok_or(ExecutionError::Unsupported)?;
        let event = body
            .event_context
            .event_ref
            .open(&self.journal.keys, (self.journal.clock)())
            .map_err(|_| ExecutionError::Expired)?
            .payload;
        let mut evidence = lease
            .state
            .delivery
            .as_ref()
            .and_then(|checkpoint| checkpoint.custom.clone())
            .ok_or(ExecutionError::Corrupt)?;
        if !evidence.valid_for(&CustomCheckpoint::new(&event, body, effect.webhook())) {
            return Err(ExecutionError::Corrupt);
        }
        if recovering {
            if evidence.local == Some(LocalStreamOutcome::Pending) {
                evidence.local = Some(LocalStreamOutcome::Lost);
            }
            let uncertain = evidence.webhooks.iter_mut().any(|leg| {
                if matches!(leg.status, WebhookStatus::Intent) {
                    leg.status = WebhookStatus::Unknown;
                }
                matches!(
                    leg.status,
                    WebhookStatus::Unknown
                        | WebhookStatus::Settled {
                            outcome: DeliveryOutcome::Unknown { .. }
                        }
                )
            });
            self.record_custom(lease, &evidence).await?;
            if evidence.local == Some(LocalStreamOutcome::Lost) {
                custom.lost_local_stream(&event);
            }
            if uncertain {
                self.record(lease, DeliveryStatus::Unknown).await?;
                return Ok(FrameDelivery::Unknown);
            }
        }
        if evidence.local == Some(LocalStreamOutcome::Pending) {
            let current = current_target(self.authority.as_ref(), plan, target).await?;
            if !current
                .definition
                .reply
                .sinks
                .iter()
                .any(|sink| matches!(sink, Sink::Sync {}))
            {
                return Err(ExecutionError::AuthorityChanged);
            }
            let gate = DeliveryGate::new(
                self.journal.clone(),
                lease,
                self.authority.clone(),
                plan.clone(),
                target.clone(),
                current.version,
                None,
            );
            let cancel = CancelSend(gate.clone());
            let _ticket = gate
                .enter_route(NO_CHAT_ROUTE)
                .await
                .map_err(|_| ExecutionError::LostLease)?;
            if !gate.commit() {
                return Err(ExecutionError::LostLease);
            }
            evidence.local = Some(custom.local(&event, effect));
            drop(cancel);
            self.record_custom(lease, &evidence).await?;
        }
        for index in 0..evidence.webhooks.len() {
            if !matches!(evidence.webhooks[index].status, WebhookStatus::NotStarted) {
                continue;
            }
            let destination = evidence.webhooks[index].destination.clone();
            let current = current_target(self.authority.as_ref(), plan, target).await?;
            if !webhook_authorized(&current, &destination) {
                return Err(ExecutionError::AuthorityChanged);
            }
            evidence.webhooks[index].status = WebhookStatus::Intent;
            self.record_custom(lease, &evidence).await?;
            self.journal.renew(lease).await?;
            let gate = DeliveryGate::new(
                self.journal.clone(),
                lease,
                self.authority.clone(),
                plan.clone(),
                target.clone(),
                current.version,
                None,
            )
            .with_origin_webhook(destination.clone());
            let cancel = CancelSend(gate.clone());
            let outcome = tokio::time::timeout(
                SEND_WINDOW,
                custom.send_webhook(&current, &event, effect, &destination, gate.as_ref()),
            )
            .await
            .unwrap_or_else(|_| DeliveryOutcome::Unknown {
                reason: "delivery_deadline".into(),
            });
            drop(cancel);
            let uncertain = matches!(outcome, DeliveryOutcome::Unknown { .. });
            evidence.webhooks[index].status = WebhookStatus::Settled { outcome };
            self.record_custom(lease, &evidence).await?;
            if uncertain {
                self.record(lease, DeliveryStatus::Unknown).await?;
                return Ok(FrameDelivery::Unknown);
            }
        }
        // Platform evidence belongs to each webhook leg; local buffering never
        // becomes an aggregate Confirmed receipt, even with zero webhooks.
        self.record(lease, DeliveryStatus::Settled { receipt: None })
            .await?;
        if effect.terminal() && !self.close(plan, target).await? {
            return Ok(FrameDelivery::Unknown);
        }
        self.journal
            .acknowledge_delivery(lease, &frame.response_id, frame.sequence_number)
            .await?;
        Ok(FrameDelivery::Settled {
            cursor: frame.sequence_number,
            receipt: None,
        })
    }
    async fn record_custom(
        &self,
        lease: &mut Lease,
        evidence: &CustomCheckpoint,
    ) -> Result<(), ExecutionError> {
        self.journal.guard(lease).await?;
        let mut next = lease.state.clone();
        next.delivery
            .as_mut()
            .ok_or(ExecutionError::Corrupt)?
            .custom = Some(evidence.clone());
        self.journal.commit(lease, next).await
    }
}

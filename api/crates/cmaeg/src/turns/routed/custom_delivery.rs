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
        let answer_response_id = match effect {
            CustomStreamEffect::Message { text }
            | CustomStreamEffect::Completed {
                final_text: Some(text),
            } if !text.trim().is_empty() => Some(frame.response_id.as_str()),
            _ => None,
        };
        let event =
            body.settlement_event(plan, target, &self.journal.keys, (self.journal.clock)())?;
        let mut evidence = lease
            .state
            .delivery
            .as_ref()
            .and_then(|checkpoint| checkpoint.custom.clone())
            .ok_or(ExecutionError::Corrupt)?;
        if !evidence.valid_for(&CustomCheckpoint::new(
            &event,
            &body.delivery.policy.sinks,
            effect.webhook(),
        )) {
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
            let outcome = if event.expires_at <= (self.journal.clock)() {
                // Only a content-free terminal control may close the original waiter.
                // Expiry still refuses answer text and every external delivery leg.
                self.journal.guard(lease).await?;
                let control = match effect {
                    CustomStreamEffect::Completed { .. } => {
                        Some(CustomStreamEffect::Completed { final_text: None })
                    }
                    CustomStreamEffect::Failed => Some(CustomStreamEffect::Failed),
                    _ => None,
                };
                control.map_or(LocalStreamOutcome::Unavailable, |control| {
                    if body.metadata.contains_key(super::media_schedule::KEY) {
                        custom.local_response(&event, &control, &frame.response_id)
                    } else {
                        custom.local(&event, &control)
                    }
                })
            } else {
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
                let outcome = if self.remote_waiter(&event) {
                    self.forward_stream(plan, target, lease, effect, current.version)
                        .await
                } else if body.metadata.contains_key(super::media_schedule::KEY) {
                    custom.local_response(&event, effect, &frame.response_id)
                } else {
                    custom.local(&event, effect)
                };
                drop(cancel);
                outcome
            };
            evidence.local = Some(outcome);
            self.record_custom(lease, &evidence).await?;
            if effect.terminal() {
                let mut change = crate::activity::facts::delivery(
                    target.launch.consumer.as_str(),
                    crate::turns::observer::DeliveryStep::Answer,
                    &DeliveryOutcome::Unknown {
                        reason: "local_stream".into(),
                    },
                    crate::turns::observer::Destination::Origin,
                    &event.reply,
                    None,
                );
                if let Some(delivery) = &mut change.delivery {
                    delivery.outcome = match evidence.local {
                        Some(LocalStreamOutcome::Buffered) => "buffered",
                        Some(LocalStreamOutcome::Unavailable) => "unavailable",
                        _ => "unknown",
                    }
                    .into();
                }
                self.journal.observe(&plan.identity, change).await;
            }
        }
        for index in 0..evidence.webhooks.len() {
            if !matches!(evidence.webhooks[index].status, WebhookStatus::NotStarted) {
                continue;
            }
            let destination = evidence.webhooks[index].destination.clone();
            if event.expires_at <= (self.journal.clock)() {
                let outcome = DeliveryOutcome::Rejected {
                    reason: "event_ref_expired".into(),
                };
                self.journal
                    .observe(
                        &plan.identity,
                        crate::activity::facts::delivery(
                            target.launch.consumer.as_str(),
                            crate::turns::observer::DeliveryStep::Answer,
                            &outcome,
                            crate::turns::observer::Destination::Origin,
                            &cmaeg_core::seal::ReplyAddress::ResultWebhook {
                                url: destination.url,
                                hmac_key_id: String::new(),
                            },
                            None,
                        ),
                    )
                    .await;
                evidence.webhooks[index].status = WebhookStatus::Settled { outcome };
                self.record_custom(lease, &evidence).await?;
                continue;
            }
            let current = current_target(self.authority.as_ref(), plan, target).await?;
            if !webhook_authorized(&current, &destination) {
                return Err(ExecutionError::AuthorityChanged);
            }
            evidence.webhooks[index].status = WebhookStatus::Intent;
            self.record_custom(lease, &evidence).await?;
            self.journal.renew(lease).await?;
            let scope = super::live_scope::Scope::new(
                self.journal.clone(),
                super::live_scope::Capture::execution(lease.clone(), None),
            );
            let gate = DeliveryGate::new(
                self.journal.clone(),
                lease,
                self.authority.clone(),
                plan.clone(),
                target.clone(),
                current.version,
                None,
            )
            .with_origin_webhook(destination.clone())
            .with_scope(scope.clone());
            let cancel = CancelSend(gate.clone());
            let outcome = scope
                .run(
                    "custom_webhook",
                    tokio::time::timeout(
                        SEND_WINDOW,
                        custom.send_webhook(&current, &event, effect, &destination, gate.as_ref()),
                    ),
                )
                .await?;
            *lease = scope.execution();
            let outcome = outcome.unwrap_or_else(|_| DeliveryOutcome::Unknown {
                reason: "delivery_deadline".into(),
            });
            drop(cancel);
            let uncertain = matches!(outcome, DeliveryOutcome::Unknown { .. });
            self.journal
                .observe(
                    &plan.identity,
                    crate::activity::facts::delivery(
                        target.launch.consumer.as_str(),
                        crate::turns::observer::DeliveryStep::Answer,
                        &outcome,
                        crate::turns::observer::Destination::Origin,
                        &cmaeg_core::seal::ReplyAddress::ResultWebhook {
                            url: destination.url.clone(),
                            hmac_key_id: String::new(),
                        },
                        None,
                    )
                    .with_answer_response(answer_response_id),
                )
                .await;
            evidence.webhooks[index].status = WebhookStatus::Settled { outcome };
            super::live_scope::receipt(self.record_custom(lease, &evidence)).await?;
            if uncertain {
                self.record(lease, DeliveryStatus::Unknown).await?;
                return Ok(FrameDelivery::Unknown);
            }
        }
        // Platform evidence belongs to each webhook leg; local buffering never
        // becomes an aggregate Confirmed receipt, even with zero webhooks.
        self.record(lease, DeliveryStatus::Settled { receipt: None })
            .await?;
        if effect.terminal() && !self.close(plan, target, lease).await? {
            return Ok(FrameDelivery::Unknown);
        }
        self.journal
            .acknowledge_frame(plan, target, lease, frame)
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

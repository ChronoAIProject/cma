#[path = "media_delivery_close.rs"]
mod close;
#[path = "media_delivery_projection.rs"]
mod projection;
use projection::response_text;
pub(super) use projection::summary;
// Terminal supplementary responses use their own journal and delivery identity.
// The original response's terminal barrier and cursor are never reopened.
use super::{
    adapter::credential,
    builder::provider_error,
    custom_delivery::is_custom_origin,
    custom_state::WebhookStatus,
    dispatcher::current_target,
    media_delivery_gate::{CancelMediaSend, MediaSendGate},
    media_response_store::{MediaResponseState, NativeOutcome},
    media_worker::MediaResponseDelivery,
    Authority, CustomCheckpoint, ExecutionError, MediaLease, MediaObligationStore, TargetState,
};
use crate::{
    activity::{ActivityMediaResponse, Change, ConsumerState},
    delivery::{
        durable::DeliveryCoordinator,
        gate::{SendGate, NO_CHAT_ROUTE},
        intents::DeliveryOutcome,
        local_stream::LocalStreamOutcome,
    },
    ingress::routing_store::{AdmittedPlan, StoredTarget},
    providers::{ProviderClient, RecoveryIdentity},
};
use cmaeg_core::{
    channel::ReplyChoice,
    media_continuation::MediaContinuationKind,
    message::OutboundMessage,
    problem::{Problem, ProblemCode},
};
use futures_util::future::BoxFuture;
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};
pub(crate) struct MediaDelivery {
    pub store: MediaObligationStore,
    pub authority: Arc<dyn Authority>,
    pub providers: Arc<ProviderClient>,
    pub coordinator: DeliveryCoordinator,
}
impl MediaDelivery {
    async fn run(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        lease: &mut MediaLease,
    ) -> Result<(), ExecutionError> {
        self.store.0.require_reply_open(plan, target).await?;
        let kinds: Vec<_> = lease
            .state()
            .supplements
            .iter()
            .filter(|s| s.admission.is_some() && !s.settled)
            .map(|s| s.artifact.kind)
            .collect();
        for kind in kinds {
            self.one(plan, target, lease, kind).await?;
        }
        Ok(())
    }
    async fn one(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        lease: &mut MediaLease,
        kind: MediaContinuationKind,
    ) -> Result<(), ExecutionError> {
        let receipt = lease
            .state()
            .supplements
            .iter()
            .find(|s| s.artifact.kind == kind)
            .and_then(|s| s.admission.clone())
            .ok_or(ExecutionError::Corrupt)?;
        let (parent, _) = self
            .store
            .0
            .read::<TargetState>(&target.id)
            .await?
            .ok_or(ExecutionError::Corrupt)?;
        super::journal::validate(&parent, plan, target)?;
        let body = self.store.0.retained_body(&parent).await?;
        body.validate(plan, target, &self.store.0.keys, (self.store.0.clock)())?;
        let mut recovered = true;
        let mut state = match self.store.media_response(lease, kind).await? {
            Some(state) => state,
            None => {
                self.store.renew(lease).await?;
                let current = current_target(self.authority.as_ref(), plan, target).await?;
                self.store.guard_effect(lease).await?;
                let cmaeg_core::channel::ProviderTarget::ExistingAgent { agent_id } =
                    &target.launch.provider.target
                else {
                    return Err(ExecutionError::Corrupt);
                };
                let snapshot = self
                    .providers
                    .response_snapshot(
                        &target.launch.provider.slug,
                        &RecoveryIdentity {
                            response_id: receipt.response_id.clone(),
                            native_agent_id: Some(agent_id.clone()),
                            metadata: receipt.metadata.clone(),
                        },
                        credential(&current, target)?,
                    )
                    .await
                    .map_err(provider_error)?;
                let Some(status) = snapshot.response["status"].as_str() else {
                    return Err(ExecutionError::Corrupt);
                };
                if !matches!(status, "completed" | "failed" | "incomplete" | "cancelled") {
                    self.observe(
                        plan,
                        target,
                        lease,
                        kind,
                        &receipt.response_id,
                        (status, "pending"),
                    )
                    .await;
                    return Ok(());
                }
                let text = if body.delivery.auto_final {
                    response_text(&snapshot.response)?
                } else {
                    String::new()
                };
                let custom = is_custom_origin(&body).then(|| {
                    CustomCheckpoint::new(
                        &super::model::delivery_event(plan, target),
                        &body.delivery.policy.sinks,
                        true,
                    )
                });
                recovered = false;
                self.store
                    .freeze_media_response(lease, kind, status, &text, custom)
                    .await?
            }
        };
        let text = self.store.media_response_text(lease, &state).await?;
        let event = super::model::delivery_event(plan, target);
        let data = json!({"event_id":event.event_id,"conversation_id":event.conversation_id,"response_id":state.response_id,"response_url":format!("/api/v2/responses/{}",state.response_id),"parent_response_id":receipt.metadata.get("cma_parent_response_id"),"delivery_id":state.id,"attachment_index":lease.state().attachment_index,"kind":kind.as_str(),"status":state.status,"text":text});
        if state.custom.is_some() {
            self.custom((plan, target), lease, &body, &mut state, &data, recovered)
                .await?;
        } else if body.delivery.auto_final && !text.is_empty() {
            match state.native {
                NativeOutcome::Intent if recovered => {
                    state.native = NativeOutcome::Unknown;
                    self.store.record_media_response(lease, &mut state).await?;
                }
                NativeOutcome::NotStarted => {
                    state.native = NativeOutcome::Intent;
                    self.store.record_media_response(lease, &mut state).await?;
                    self.store.renew(lease).await?;
                    let current = current_target(self.authority.as_ref(), plan, target).await?;
                    self.store.guard_effect(lease).await?;
                    let destination = body.delivery.default_destination.clone();
                    let scope = super::live_scope::Scope::new(
                        self.store.0.clone(),
                        super::live_scope::Capture::media(lease.clone()),
                    );
                    let gate = MediaSendGate::new(
                        self.store.clone(),
                        lease,
                        self.authority.clone(),
                        plan,
                        target,
                        current.version,
                        destination.clone(),
                    )
                    .with_scope(scope.clone());
                    let cancel = CancelMediaSend(gate.clone());
                    let message = OutboundMessage {
                        text: Some(text),
                        blocks: vec![],
                        reply_to: None,
                        structured_content: None,
                        reply_with_received_sticker: false,
                        clear_native_attachments: false,
                        native_media_update: None,
                        native_attachments: vec![],
                        whatsapp_template: None,
                        thread_reply: false,
                    };
                    let send = async {
                        match (body.delivery.destination, destination) {
                            (ReplyChoice::Default, Some(destination)) => {
                                self.coordinator
                                    .reply_default(
                                        current,
                                        event.clone(),
                                        message,
                                        gate,
                                        destination,
                                        state.id.clone(),
                                    )
                                    .await
                            }
                            (ReplyChoice::Origin, _) => {
                                self.coordinator
                                    .reply(current, event.clone(), message, gate)
                                    .await
                            }
                            _ => crate::delivery::durable::receipt_for(DeliveryOutcome::Rejected {
                                reason: "media_destination_missing".into(),
                            }),
                        }
                    };
                    let receipt = scope
                        .run(
                            "media_reply",
                            tokio::time::timeout(Duration::from_secs(20), send),
                        )
                        .await?;
                    *lease = scope.media();
                    let receipt = receipt.unwrap_or_else(|_| {
                        crate::delivery::durable::receipt_for(DeliveryOutcome::Unknown {
                            reason: "media_delivery_deadline".into(),
                        })
                    });
                    drop(cancel);
                    super::live_scope::receipt(async {
                        self.store.guard(lease).await?;
                        if lease.partition.is_some() {
                            state.native_receipt = Some(
                                self.store
                                    .0
                                    .receipts(
                                        &state.id,
                                        &lease.record.owner,
                                        lease.record.retained_until,
                                    )
                                    .retain(&receipt)
                                    .await
                                    .map_err(|_| ExecutionError::Unavailable)?,
                            );
                        }
                        state.native = NativeOutcome::Settled {
                            outcome: receipt.delivery,
                        };
                        self.store.record_media_response(lease, &mut state).await?;
                        Ok(())
                    })
                    .await?;
                }
                _ => {}
            }
        } else if matches!(state.native, NativeOutcome::NotStarted) {
            state.native = NativeOutcome::Settled {
                outcome: DeliveryOutcome::Rejected {
                    reason: "automatic_reply_disabled_or_empty".into(),
                },
            };
            self.store.record_media_response(lease, &mut state).await?;
        }
        let delivery = summary(&state);
        self.observe(
            plan,
            target,
            lease,
            kind,
            &state.response_id,
            (&state.status, delivery),
        )
        .await;
        if self.store.0.partitions.is_some() && matches!(delivery, "unknown" | "pending") {
            tracing::warn!(
                effect_sha256 = super::storage::hash(state.id.as_bytes()),
                outcome = delivery,
                "Media delivery retains FIFO custody pending reconciliation"
            );
            return Err(ExecutionError::Uncertain);
        }
        self.store.settle_media_response(lease, kind).await
    }
    async fn custom(
        &self,
        route: (&AdmittedPlan, &StoredTarget),
        lease: &mut MediaLease,
        body: &super::FrozenTurn,
        state: &mut MediaResponseState,
        data: &Value,
        recovered: bool,
    ) -> Result<(), ExecutionError> {
        let (plan, target) = route;
        let custom = self
            .coordinator
            .custom
            .as_ref()
            .ok_or(ExecutionError::Unsupported)?;
        let event = super::model::delivery_event(plan, target);
        let mut evidence = state.custom.clone().ok_or(ExecutionError::Corrupt)?;
        if !evidence.valid_for(&CustomCheckpoint::new(
            &event,
            &body.delivery.policy.sinks,
            true,
        )) {
            return Err(ExecutionError::Corrupt);
        }
        if recovered {
            if evidence.local == Some(LocalStreamOutcome::Pending) {
                evidence.local = Some(LocalStreamOutcome::Lost);
            }
            for leg in &mut evidence.webhooks {
                if matches!(leg.status, WebhookStatus::Intent) {
                    leg.status = WebhookStatus::Unknown;
                }
            }
            state.custom = Some(evidence.clone());
            super::live_scope::receipt(self.store.record_media_response(lease, state)).await?;
            if lease.partition.is_some() && summary(state) == "unknown" {
                return Err(ExecutionError::Uncertain);
            }
        }
        if evidence.local == Some(LocalStreamOutcome::Pending) {
            self.store.renew(lease).await?;
            let current = current_target(self.authority.as_ref(), plan, target).await?;
            if !current
                .definition
                .reply
                .sinks
                .iter()
                .any(|sink| matches!(sink, cmaeg_core::channel::Sink::Sync {}))
            {
                return Err(ExecutionError::AuthorityChanged);
            }
            let gate = MediaSendGate::new(
                self.store.clone(),
                lease,
                self.authority.clone(),
                plan,
                target,
                current.version,
                None,
            );
            let cancel = CancelMediaSend(gate.clone());
            let _ticket = gate
                .enter_route(NO_CHAT_ROUTE)
                .await
                .map_err(|_| ExecutionError::LostLease)?;
            if !gate.commit() {
                return Err(ExecutionError::LostLease);
            }
            evidence.local = Some(custom.local_media_response(&event, data));
            drop(cancel);
            state.custom = Some(evidence.clone());
            self.store.record_media_response(lease, state).await?;
        }
        for index in 0..evidence.webhooks.len() {
            if !matches!(evidence.webhooks[index].status, WebhookStatus::NotStarted) {
                continue;
            }
            let destination = evidence.webhooks[index].destination.clone();
            self.store.renew(lease).await?;
            let current = current_target(self.authority.as_ref(), plan, target).await?;
            if !crate::delivery::local_stream::webhook_authorized(&current, &destination) {
                return Err(ExecutionError::AuthorityChanged);
            }
            evidence.webhooks[index].status = WebhookStatus::Intent;
            state.custom = Some(evidence.clone());
            self.store.record_media_response(lease, state).await?;
            let scope = super::live_scope::Scope::new(
                self.store.0.clone(),
                super::live_scope::Capture::media(lease.clone()),
            );
            let gate = MediaSendGate::new(
                self.store.clone(),
                lease,
                self.authority.clone(),
                plan,
                target,
                current.version,
                None,
            )
            .with_scope(scope.clone());
            let cancel = CancelMediaSend(gate.clone());
            let outcome = scope
                .run(
                    "media_webhook",
                    tokio::time::timeout(
                        Duration::from_secs(20),
                        custom.send_media_webhook(
                            &current,
                            &event,
                            data.clone(),
                            &destination,
                            gate.as_ref(),
                        ),
                    ),
                )
                .await?;
            *lease = scope.media();
            let outcome = outcome.unwrap_or(DeliveryOutcome::Unknown {
                reason: "media_delivery_deadline".into(),
            });
            drop(cancel);
            evidence.webhooks[index].status = WebhookStatus::Settled { outcome };
            state.custom = Some(evidence.clone());
            super::live_scope::receipt(self.store.record_media_response(lease, state)).await?;
            if lease.partition.is_some() && summary(state) == "unknown" {
                return Err(ExecutionError::Uncertain);
            }
        }
        Ok(())
    }
    async fn observe(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        lease: &MediaLease,
        kind: MediaContinuationKind,
        response_id: &str,
        outcome: (&str, &str),
    ) {
        let (status, delivery) = outcome;
        let mut change = Change::state(
            target.launch.consumer.as_str(),
            ConsumerState::Received,
            None,
        );
        change.state = None;
        change.media_response = Some(ActivityMediaResponse {
            attachment_index: lease.state().attachment_index,
            kind: kind.as_str().into(),
            response_id: response_id.into(),
            response_url: format!("/api/v2/responses/{response_id}"),
            status: status.into(),
            delivery: delivery.into(),
            observed_at: crate::activity::now(),
        });
        self.store.0.observe(&plan.identity, change).await;
    }
}
impl MediaResponseDelivery for MediaDelivery {
    fn finish_partition<'a>(
        &'a self,
        plan: &'a AdmittedPlan,
        target: &'a StoredTarget,
        partition: &'a Option<crate::replicas::partition::PartitionLease>,
    ) -> BoxFuture<'a, Result<(), ExecutionError>> {
        Box::pin(self.finish_inner(plan, target, partition))
    }
    fn deliver<'a>(
        &'a self,
        plan: &'a AdmittedPlan,
        target: &'a StoredTarget,
        lease: &'a mut MediaLease,
    ) -> BoxFuture<'a, Result<(), ExecutionError>> {
        Box::pin(self.run(plan, target, lease))
    }
    fn finish<'a>(
        &'a self,
        plan: &'a AdmittedPlan,
        target: &'a StoredTarget,
    ) -> BoxFuture<'a, Result<(), ExecutionError>> {
        Box::pin(async move { self.finish_inner(plan, target, &None).await })
    }
}

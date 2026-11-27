//! One progress acknowledgement for a chat event held at conversation readiness.
use super::super::{
    delivery_gate::{CancelSend, DeliveryGate},
    model::delivery_event,
    storage, FrozenTurn, Lease,
};
use super::Dispatcher;
use crate::{
    channels::resolver::ResolvedChannel,
    delivery::{durable::DeliveryCoordinator, intents::ReplySource},
    ingress::routing_store::{AdmittedPlan, StoredTarget},
};
use cmaeg_core::{
    activity::ActivityKind, channel::Sink, message::OutboundMessage, seal::ReplyAddress,
};
use std::{sync::Arc, time::Duration};

const KIND: &str = "routed_readiness_acknowledgement_v1";
const BUDGET: Duration = Duration::from_secs(2);

pub(super) struct Acknowledgement {
    delivery: DeliveryCoordinator,
    text: String,
}

impl Dispatcher {
    pub(in crate::turns::routed) fn with_readiness_acknowledgement(
        mut self,
        delivery: DeliveryCoordinator,
        text: &str,
    ) -> Self {
        self.acknowledgement = Some(Acknowledgement {
            delivery,
            text: text.to_owned(),
        });
        self
    }

    pub(super) async fn acknowledge_readiness_wait(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        lease: &Lease,
        channel: Arc<ResolvedChannel>,
        body: &FrozenTurn,
    ) {
        let Some(ack) = &self.acknowledgement else {
            return;
        };
        if plan
            .targets
            .first()
            .is_none_or(|primary| primary.id != target.id)
            || !matches!(
                plan.activity.kind,
                ActivityKind::Message { .. } | ActivityKind::Command { .. }
            )
            || !body.delivery.destination.is_origin()
            || !allows_acknowledgement(&body.delivery.policy)
            || !allows_acknowledgement(&channel.definition.reply)
        {
            return;
        }
        let event = delivery_event(plan, target);
        if matches!(
            event.reply,
            ReplyAddress::None | ReplyAddress::Sync { .. } | ReplyAddress::ResultWebhook { .. }
        ) {
            return;
        }
        let id = storage::identifier(&(KIND, &target.id));
        // A different nonce on every attempt lets the journal confirm a lost
        // CAS response without mistaking another worker's admission for ours.
        let marker = (KIND, uuid::Uuid::new_v4());
        let _ = tokio::time::timeout(BUDGET, async {
            if !matches!(self.journal.read::<serde_json::Value>(&id).await, Ok(None))
                || self.journal.guard(lease).await.is_err()
                || self
                    .journal
                    .write(
                        &id,
                        &plan.identity.owner,
                        plan.retained_until,
                        &marker,
                        None,
                    )
                    .await
                    .is_err()
            {
                return;
            }
            let gate = DeliveryGate::new(
                self.journal.clone(),
                lease,
                self.authority.clone(),
                plan.clone(),
                target.clone(),
                channel.version,
                None,
            );
            let _cancel = CancelSend(gate.clone());
            let message = OutboundMessage {
                structured_content: None,
                reply_with_received_sticker: false,
                clear_native_attachments: false,
                native_media_update: None,
                whatsapp_template: None,
                native_attachments: vec![],
                thread_reply: matches!(
                    event.reply,
                    ReplyAddress::NyxidRelay {
                        thread_reply: true,
                        ..
                    }
                ),
                text: Some(ack.text.clone()),
                blocks: vec![],
                reply_to: None,
            };
            // A confirmed, rejected or uncertain acknowledgement leaves the
            // original provider body and its answer delivery state intact.
            ack.delivery
                .reply_from(channel, event, message, gate, ReplySource::Acknowledgement)
                .await;
        })
        .await;
    }
}

fn allows_acknowledgement(policy: &cmaeg_core::channel::ReplyPolicy) -> bool {
    policy.ack_after_ms != 0
        && policy
            .sinks
            .iter()
            .any(|sink| matches!(sink, Sink::Source {}))
}

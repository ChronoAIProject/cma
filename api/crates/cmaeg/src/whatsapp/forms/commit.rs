//! An explicit commit can attempt one customer request; recovery only binds receipts.
use super::{
    hash,
    registry::{Asset, Phase as AssetPhase},
    service::{card_error, receipt, Forms},
    Error,
};
use crate::{
    channels::resolver::ResolvedChannel,
    delivery::intents::IntentReceipt,
    interactions::whatsapp_flow::{self, Phase},
};
use cmaeg_core::{message::OutboundMessage, seal::EventRefPayload};
use std::time::Duration;
use tokio::time::Instant;

impl Forms {
    pub(crate) async fn commit(
        &self,
        channel: &ResolvedChannel,
        event: &EventRefPayload,
        message: &OutboundMessage,
        selector: &str,
        key: &str,
        deadline: Instant,
    ) -> Result<IntentReceipt, Error> {
        let id = whatsapp_flow::slot(channel, event);
        let (mut card, mut version) = self
            .provisioner
            .cards
            .load_whatsapp_flow(&id)
            .await
            .map_err(card_error)?;
        self.provisioner
            .cards
            .whatsapp_flow_current(&card, channel)
            .await
            .map_err(card_error)?;
        let a = card.native_whatsapp_flow.as_ref().ok_or(Error::Invalid)?;
        if a.original != *event || a.preparation_ref != selector || card.message != *message {
            return Err(Error::Conflict);
        }
        let key = hash(&key);
        match &a.phase {
            Phase::Prepared => {}
            Phase::MessageAttempted { key: old, .. }
            | Phase::Bound { key: old, .. }
            | Phase::Declined { key: old, .. } => {
                if old != &key {
                    return Err(Error::Conflict);
                }
                return receipt(&card);
            }
            Phase::ConfirmedUnbound { key: old, .. } => {
                if old != &key {
                    return Err(Error::Conflict);
                }
                self.provisioner
                    .cards
                    .bind_whatsapp_flow(&mut card, version)
                    .await
                    .map_err(card_error)?;
                return receipt(&card);
            }
        }
        if !self.issuance_enabled
            || event.expires_at <= time::OffsetDateTime::now_utc().unix_timestamp()
            || a.service_until <= time::OffsetDateTime::now_utc().unix_timestamp()
        {
            return Err(Error::Revoked);
        }
        let (asset, _) = self
            .provisioner
            .store
            .load::<Asset>(&a.asset_registry)
            .await?
            .ok_or(Error::Unavailable)?;
        let AssetPhase::Ready { id: flow_id } = &asset.phase else {
            return Err(Error::Pending);
        };
        if !asset.current(channel) || asset.digest != a.asset_digest {
            return Err(Error::Revoked);
        }
        let credentials = channel
            .definition
            .secrets
            .whatsapp
            .as_ref()
            .ok_or(Error::Revoked)?;
        self.provisioner
            .manager
            .verify(&asset.source, credentials)
            .await
            .map_err(|_| Error::Revoked)?;
        let proof = self
            .provisioner
            .manager
            .inspect(&asset.source, credentials, flow_id, &asset.name)
            .await
            .map_err(|_| Error::Unavailable)?;
        if !proof.published || proof.id != *flow_id || proof.digest != asset.digest {
            return Err(Error::Revoked);
        }
        let current = self
            .provisioner
            .cards
            .refresh_whatsapp_flow(&card)
            .await
            .map_err(card_error)?;
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        if event.expires_at <= now || a.service_until <= now {
            return Err(Error::Revoked);
        }
        // Never persist a message attempt after slow preflight consumed the request budget.
        if deadline.saturating_duration_since(Instant::now()) < Duration::from_millis(500) {
            return Err(Error::Unavailable);
        }
        let credentials = current
            .definition
            .secrets
            .whatsapp
            .as_ref()
            .ok_or(Error::Revoked)?;
        card.native_whatsapp_flow.as_mut().unwrap().phase = Phase::MessageAttempted {
            key: key.clone(),
            attempt: uuid::Uuid::new_v4().to_string(),
        };
        version = self
            .provisioner
            .cards
            .save_whatsapp_flow(&card, version)
            .await
            .map_err(card_error)?;
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        if Instant::now() >= deadline
            || event.expires_at <= now
            || card.native_whatsapp_flow.as_ref().unwrap().service_until <= now
        {
            return receipt(&card);
        }
        let outcome = self
            .sender
            .send_flow(credentials, event, &card, flow_id)
            .await;
        match outcome {
            Ok(message_id) => {
                let a = card.native_whatsapp_flow.as_ref().unwrap();
                let confirmed_at = time::OffsetDateTime::now_utc().unix_timestamp();
                let native = crate::whatsapp::receipts::Receipt {
                    kind: "native_whatsapp_receipt_v1".into(),
                    group: None,
                    original: crate::ingress::routing_store::EventIdentity {
                        owner: current.definition.owner.subject.clone(),
                        channel_id: current.id.clone(),
                        record_id: current.pointer.record_id.clone(),
                        event_id: event.event_id.clone(),
                    },
                    source: a.activity.source.clone(),
                    recipient: a.activity.actor.id.clone(),
                    message_id,
                    consumer: event
                        .consumer
                        .clone()
                        .unwrap_or_else(cmaeg_core::ids::ConsumerId::default_consumer),
                    provider: current
                        .consumer(event.consumer.as_ref())
                        .ok_or(Error::Revoked)?
                        .provider
                        .clone(),
                    partition_key: event.partition_key.clone(),
                    conversation_id: event.conversation_id.clone(),
                    confirmed_at,
                    retained_until: confirmed_at + crate::whatsapp::receipts::RETENTION,
                };
                card.native_whatsapp_flow.as_mut().unwrap().phase = Phase::ConfirmedUnbound {
                    key,
                    receipt: native,
                };
                version = self
                    .provisioner
                    .cards
                    .save_whatsapp_flow(&card, version)
                    .await
                    .map_err(card_error)?;
                self.provisioner
                    .cards
                    .bind_whatsapp_flow(&mut card, version)
                    .await
                    .map_err(card_error)?;
            }
            Err(
                crate::whatsapp::Error::Rejected
                | crate::whatsapp::Error::Invalid
                | crate::whatsapp::Error::CustodyUnproved,
            ) => {
                card.native_whatsapp_flow.as_mut().unwrap().phase = Phase::Declined {
                    key,
                    reason: "whatsapp_form_send_declined".into(),
                };
                self.provisioner
                    .cards
                    .save_whatsapp_flow(&card, version)
                    .await
                    .map_err(card_error)?;
            }
            Err(_) => {}
        }
        receipt(&card)
    }
}

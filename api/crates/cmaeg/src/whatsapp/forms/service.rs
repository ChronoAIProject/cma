//! Short agent requests admit preparation or explicitly commit one ready form.
use super::{
    compile,
    management::ManagementClient,
    provision::Provisioner,
    registry::{Asset, Phase as AssetPhase, Store},
    Error,
};
use crate::{
    channels::resolver::{ChannelResolver, ResolvedChannel},
    delivery::intents::{DeliveryOutcome, IntentReceipt},
    ingress::routing_store::{EventIdentity, PlanStore},
    interactions::{
        whatsapp_flow::{self, Phase},
        Card, InteractionStore,
    },
    records::RecordClient,
};
use cmaeg_core::{
    message::OutboundMessage,
    seal::{EventRefPayload, MasterKeyRing, ReplyAddress},
};
use serde::Serialize;
use std::sync::Arc;
use utoipa::ToSchema;

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct FormPreparationReceipt {
    pub preparation_id: String,
    pub expires_at: i64,
    pub customer_message: FormCustomerMessage,
    #[serde(flatten)]
    pub state: FormPreparationState,
}
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum FormCustomerMessage {
    NotSent,
    Attempted,
}
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum FormPreparationState {
    Pending { retry_after_ms: u64 },
    Ready { preparation_ref: String },
    Declined { reason: String },
    Uncertain { reason: String },
    Revoked { reason: String },
    Delivery { receipt: IntentReceipt },
}

#[derive(Clone)]
pub struct Forms {
    pub(crate) provisioner: Provisioner,
    pub(crate) issuance_enabled: bool,
    pub(crate) sender: crate::whatsapp::Client,
    plans: PlanStore,
}
impl Forms {
    pub(crate) fn new(
        records: RecordClient,
        keys: Arc<MasterKeyRing>,
        resolver: ChannelResolver,
        cards: InteractionStore,
        client: crate::whatsapp::Client,
        issuance_enabled: bool,
        writes_enabled: bool,
    ) -> Self {
        Self {
            provisioner: Provisioner {
                store: Store::new(records.clone(), keys.clone()),
                resolver,
                cards,
                manager: Arc::new(ManagementClient::new(client.clone())),
                writes_enabled,
            },
            issuance_enabled,
            sender: client,
            plans: PlanStore::new(records, keys),
        }
    }
    pub(crate) async fn prepare(
        &self,
        channel: &ResolvedChannel,
        event: &EventRefPayload,
        message: &OutboundMessage,
        key: &str,
    ) -> Result<FormPreparationReceipt, Error> {
        if !self.issuance_enabled {
            return Err(Error::Revoked);
        }
        compile::compile(message)?;
        let identity = EventIdentity {
            owner: channel.definition.owner.subject.clone(),
            channel_id: channel.id.clone(),
            record_id: channel.pointer.record_id.clone(),
            event_id: event.event_id.clone(),
        };
        let plan = self
            .plans
            .load(&identity)
            .await
            .map_err(|_| Error::Unavailable)?
            .ok_or(Error::Invalid)?;
        if !plan.targets.iter().any(|t| {
            t.launch.event == *event
                && channel
                    .consumer(event.consumer.as_ref())
                    .is_some_and(|c| c.provider == &t.launch.provider)
        }) || !whatsapp_flow::eligible(channel, event, &plan.activity)
        {
            return Err(Error::Revoked);
        }
        match self.card(channel, event).await {
            Ok(card) => {
                let a = card.native_whatsapp_flow.as_ref().ok_or(Error::Invalid)?;
                if card.message != *message || a.preparation_key != super::hash(&key) {
                    return Err(Error::Conflict);
                }
                return self.project(&card, channel).await;
            }
            Err(Error::Missing) => {}
            Err(error) => return Err(error),
        }
        let card = self
            .provisioner
            .cards
            .prepare_whatsapp_flow(
                channel,
                event,
                message,
                plan.activity,
                key,
                &self.provisioner.store,
            )
            .await;
        let card = match card {
            Ok(card) => card,
            Err(crate::interactions::Error::Denied) => {
                // A different request may have won the shared slot after our read.
                let existing = self.card(channel, event).await?;
                let a = existing
                    .native_whatsapp_flow
                    .as_ref()
                    .ok_or(Error::Invalid)?;
                if existing.message != *message || a.preparation_key != super::hash(&key) {
                    return Err(Error::Conflict);
                }
                existing
            }
            Err(error) => return Err(card_error(error)),
        };
        self.project(&card, channel).await
    }
    pub(crate) async fn status(
        &self,
        channel: &ResolvedChannel,
        event: &EventRefPayload,
    ) -> Result<FormPreparationReceipt, Error> {
        let card = self.card(channel, event).await?;
        self.project(&card, channel).await
    }
    pub(crate) async fn card(
        &self,
        channel: &ResolvedChannel,
        event: &EventRefPayload,
    ) -> Result<Card, Error> {
        let id = whatsapp_flow::slot(channel, event);
        let records = &self.provisioner.store.records;
        let url = records.record_url(&id).map_err(|_| Error::Invalid)?;
        match records.get(url.as_str()).await {
            Err(crate::records::RecordError::NotFound) => return Err(Error::Missing),
            Err(_) => return Err(Error::Unavailable),
            Ok(_) => {}
        }
        let (card, _) = self
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
        if card
            .native_whatsapp_flow
            .as_ref()
            .is_none_or(|a| a.original != *event)
        {
            return Err(Error::Revoked);
        }
        Ok(card)
    }
    async fn project(
        &self,
        card: &Card,
        channel: &ResolvedChannel,
    ) -> Result<FormPreparationReceipt, Error> {
        let a = card.native_whatsapp_flow.as_ref().ok_or(Error::Invalid)?;
        let (customer_message, state) = if !matches!(a.phase, Phase::Prepared) {
            (
                FormCustomerMessage::Attempted,
                FormPreparationState::Delivery {
                    receipt: receipt(card)?,
                },
            )
        } else {
            let (asset, _) = self
                .provisioner
                .store
                .load::<Asset>(&a.asset_registry)
                .await?
                .ok_or(Error::Unavailable)?;
            if !asset.current(channel) || asset.digest != a.asset_digest {
                return Err(Error::Revoked);
            }
            let state = match asset.phase {
                AssetPhase::Ready { .. } => FormPreparationState::Ready {
                    preparation_ref: a.preparation_ref.clone(),
                },
                AssetPhase::Declined { reason } => FormPreparationState::Declined { reason },
                AssetPhase::Revoked => FormPreparationState::Revoked {
                    reason: "whatsapp_flow_source_revoked".into(),
                },
                AssetPhase::Uncertain { stage, .. } => FormPreparationState::Uncertain {
                    reason: format!("whatsapp_flow_{stage}_unconfirmed"),
                },
                AssetPhase::CreateAttempted { .. } => FormPreparationState::Uncertain {
                    reason: "whatsapp_flow_create_unconfirmed".into(),
                },
                AssetPhase::PublishAttempted { .. } => FormPreparationState::Uncertain {
                    reason: "whatsapp_flow_publish_unconfirmed".into(),
                },
                _ => FormPreparationState::Pending {
                    retry_after_ms: 1000,
                },
            };
            (FormCustomerMessage::NotSent, state)
        };
        Ok(FormPreparationReceipt {
            preparation_id: card.id.clone(),
            expires_at: a.original.expires_at.min(a.service_until),
            customer_message,
            state,
        })
    }
}
pub(crate) fn card_error(error: crate::interactions::Error) -> Error {
    match error {
        crate::interactions::Error::Denied => Error::Revoked,
        crate::interactions::Error::Unavailable => Error::Unavailable,
    }
}
pub(crate) fn receipt(card: &Card) -> Result<IntentReceipt, Error> {
    let a = card.native_whatsapp_flow.as_ref().ok_or(Error::Invalid)?;
    let delivery = match &a.phase {
        Phase::Bound { receipt, .. } => DeliveryOutcome::Confirmed {
            platform_message_id: Some(receipt.message_id.clone()),
        },
        Phase::Declined { reason, .. } => DeliveryOutcome::Rejected {
            reason: reason.clone(),
        },
        _ => DeliveryOutcome::Unknown {
            reason: "whatsapp_form_send_unconfirmed".into(),
        },
    };
    Ok(IntentReceipt {
        message_ref: card.reference.clone(),
        rendering: compile::compile(&card.message)?.rendering,
        delivery,
    })
}

impl crate::whatsapp::Client {
    pub(crate) async fn send_flow(
        &self,
        credentials: &cmaeg_core::channel::WhatsappCredentials,
        event: &EventRefPayload,
        card: &Card,
        flow_id: &str,
    ) -> Result<String, crate::whatsapp::Error> {
        let ReplyAddress::Whatsapp {
            phone_number_id,
            recipient_id,
            message_id,
            ..
        } = &event.reply
        else {
            return Err(crate::whatsapp::Error::Invalid);
        };
        if !cmaeg_core::channel::whatsapp::valid_meta_id(flow_id) {
            return Err(crate::whatsapp::Error::Invalid);
        }
        let body = compile::compile(&card.message)
            .map_err(|_| crate::whatsapp::Error::Invalid)?
            .body;
        let result=self.call(reqwest::Method::POST,Self::endpoint(&format!("{phone_number_id}/messages"))?,credentials.access_token.as_str(),Some(serde_json::json!({
            "messaging_product":"whatsapp","recipient_type":"individual","to":recipient_id,
            "context":{"message_id":message_id},"type":"interactive","interactive":{"type":"flow","body":{"text":body},
            "action":{"name":"flow","parameters":{"flow_message_version":"3","flow_token":whatsapp_flow::token(card),"flow_id":flow_id,
            "flow_cta":"Open form","mode":"published","flow_action":"navigate","flow_action_payload":{"screen":super::SCREEN}}}}
        }))).await?;
        Self::message_identity(&result, recipient_id)
    }
}

//! Exact source generation, account, original event and selected-consumer custody.
use super::super::{owner::DispatchOwner, Card};
use crate::{
    channels::resolver::ResolvedChannel,
    whatsapp::forms::{compile, hash},
};
use cmaeg_core::{
    activity::{Activity, ActivityKind, ActorKind, ConversationKind, SourceKind},
    channel::Source,
    routing::{Resolution, RouteInput},
    seal::{EventRefPayload, ReplyAddress},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Authority {
    pub(crate) original: EventRefPayload,
    pub(crate) activity: Activity,
    pub(crate) source_version: u64,
    pub(crate) owner: DispatchOwner,
    pub(crate) custody: String,
    pub(crate) issued_at: i64,
    pub(crate) service_until: i64,
    pub(crate) provider: cmaeg_core::channel::ProviderBinding,
    pub(crate) preparation_key: String,
    pub(crate) preparation_ref: String,
    pub(crate) asset_registry: String,
    pub(crate) asset_digest: String,
    pub(crate) phase: super::Phase,
    pub(crate) pending: Option<super::Pending>,
}

pub(crate) fn source<'a>(
    channel: &'a ResolvedChannel,
    event: &EventRefPayload,
) -> Option<&'a Source> {
    let ReplyAddress::Whatsapp {
        app_id,
        waba_id,
        phone_number_id,
        ..
    } = &event.reply
    else {
        return None;
    };
    channel.definition.sources.iter().find(|source| {
        matches!(source,
        Source::Whatsapp { forms_enabled: true, app_id: a, waba_id: w, phone_number_id: p, .. }
        if a == app_id && w == waba_id && p == phone_number_id)
    })
}

pub(crate) fn route(event: &EventRefPayload) -> Option<String> {
    let ReplyAddress::Whatsapp {
        app_id,
        waba_id,
        phone_number_id,
        recipient_id: recipient,
        ..
    } = &event.reply
    else {
        return None;
    };
    Some(format!(
        "native-whatsapp-flow:{app_id}:{waba_id}:{phone_number_id}:{recipient}"
    ))
}

pub(crate) fn eligible(
    channel: &ResolvedChannel,
    event: &EventRefPayload,
    activity: &Activity,
) -> bool {
    let ReplyAddress::Whatsapp {
        app_id,
        waba_id,
        phone_number_id,
        recipient_id: recipient,
        message_id,
    } = &event.reply
    else {
        return false;
    };
    let source_kind = SourceKind::Whatsapp {
        app_id: app_id.clone(),
        waba_id: waba_id.clone(),
        phone_number_id: phone_number_id.clone(),
    };
    event.validate().is_ok()
        && event.lineage.is_none()
        && event.verifies_activity(activity)
        && event.channel_id == channel.id
        && event.record_id == channel.pointer.record_id
        && source(channel, event).is_some()
        && activity.source == source_kind
        && activity.actor.kind == ActorKind::Unknown
        && activity.actor.id == *recipient
        && activity.conversation.kind == ConversationKind::Private
        && activity.conversation.id == *recipient
        && activity.conversation.thread_id.is_none()
        && activity.thread.is_none()
        && matches!(
            activity.kind,
            ActivityKind::Message { .. } | ActivityKind::Interaction(_)
        )
        && cmaeg_core::channel::whatsapp::valid_message_id(message_id)
        && crate::whatsapp::credentials(&channel.definition, &event.reply).is_some()
        && channel
            .consumer(event.consumer.as_ref())
            .is_some_and(|c| !c.pending && c.credential.is_some())
}

impl Authority {
    pub(crate) fn prepare(
        channel: &ResolvedChannel,
        event: &EventRefPayload,
        activity: Activity,
        key: &str,
        registry: String,
        digest: String,
    ) -> Result<Self, super::Error> {
        if !eligible(channel, event, &activity) {
            return Err(super::Error::Denied);
        }
        let Resolution::Routed(decision) = cmaeg_core::routing::resolve(
            &channel.definition,
            &RouteInput {
                activity: &activity,
                chat_source: source(channel, event),
                card_consumer: Some(event.consumer.as_ref()),
                published: None,
                non_human_providers: &Default::default(),
                attributes_signed: true,
            },
            true,
        )
        .map_err(|_| super::Error::Denied)?
        else {
            return Err(super::Error::Denied);
        };
        let occurred = time::OffsetDateTime::parse(
            &activity.occurred_at,
            &time::format_description::well_known::Rfc3339,
        )
        .map_err(|_| super::Error::Denied)?
        .unix_timestamp();
        let issued_at = super::super::now();
        let service_until = occurred.saturating_add(super::super::TTL);
        if service_until <= issued_at || event.expires_at <= issued_at || occurred > issued_at + 300
        {
            return Err(super::Error::Denied);
        }
        Ok(Self {
            original: event.clone(),
            activity,
            source_version: channel.version,
            owner: DispatchOwner {
                consumer: event.consumer.clone(),
                conversation_id: event.conversation_id.clone(),
                partition_key: event.partition_key.clone(),
                routing: Some(decision.routing),
            },
            custody: String::new(),
            issued_at,
            service_until,
            provider: channel
                .consumer(event.consumer.as_ref())
                .ok_or(super::Error::Denied)?
                .provider
                .clone(),
            preparation_key: hash(&key),
            preparation_ref: uuid::Uuid::new_v4().to_string(),
            asset_registry: registry,
            asset_digest: digest,
            phase: super::Phase::Prepared,
            pending: None,
        })
    }

    pub(crate) fn digest(&self, card: &Card, channel: &ResolvedChannel) -> Option<String> {
        let credentials = crate::whatsapp::credentials(&channel.definition, &self.original.reply)?;
        let consumer = channel
            .consumer(card.consumer.as_ref())
            .filter(|c| !c.pending)?;
        Some(hash(
            &serde_json::json!({"kind":"native_whatsapp_flow_custody_v1", "original":self.original,
            "activity":self.activity,"source_version":channel.version,"source":crate::whatsapp::without_message_events(source(channel,&self.original)?),
            "source_credentials":crate::whatsapp::group_revision(source(channel,&self.original)?,credentials),
            "channel":channel.id,"record":channel.pointer.record_id,"owner":channel.definition.owner,
            "consumer":self.owner,"provider":consumer.provider,"key":channel.consumer_key_hash(card.consumer.as_ref())?,
            "credential":crate::channels::definition::hash_secret(consumer.credential?.as_str()),
            "routing":channel.definition.routing,"id":card.id,"revision":card.revision,"message":card.message,
            "issued_at":self.issued_at,"service_until":self.service_until,"issued_provider":self.provider,"expires_at":card.expires_at,"preparation_key":self.preparation_key,
            "preparation_ref":self.preparation_ref,"registry":self.asset_registry,"asset":self.asset_digest}),
        ))
    }

    pub(crate) fn current(&self, card: &Card, channel: &ResolvedChannel) -> bool {
        self.valid(card)
            && channel.version == self.source_version
            && channel.definition.owner.subject == card.owner
            && eligible(channel, &self.original, &self.activity)
            && self.digest(card, channel).as_ref() == Some(&self.custody)
    }

    pub(crate) fn valid(&self, card: &Card) -> bool {
        card.lark.is_none()
            && card.lark_delete.is_none()
            && card.whatsapp.is_none()
            && card.native_whatsapp.is_none()
            && card.native_lark.is_none()
            && card.discord.is_none()
            && card.discord_edit.is_none()
            && card.discord_effect.is_none()
            && card.discord_form_launch.is_none()
            && card.callback_owner.is_none()
            && card.destination.is_none()
            && self.original.validate().is_ok()
            && self.original.verifies_activity(&self.activity)
            && self.original.channel_id == card.channel_id
            && self.original.event_id == card.event_id
            && self.original.consumer == card.consumer
            && self.owner.consumer == card.consumer
            && self.owner.conversation_id == self.original.conversation_id
            && self.owner.partition_key == self.original.partition_key
            && route(&self.original).as_ref() == Some(&card.route_id)
            && card.actor == self.activity.actor.id
            && card.chat == self.activity.conversation.id
            && card.thread.is_none()
            && card.revision == 1
            && self.issued_at > 0
            && card.expires_at > self.issued_at
            && card.expires_at == self.issued_at.saturating_add(super::super::TTL)
            && self.service_until > self.issued_at
            && self.original.expires_at > self.issued_at
            && compile::compile(&card.message).is_ok_and(|c| c.digest == self.asset_digest)
            && self.custody.len() == 64
            && self.preparation_key.len() == 64
            && uuid::Uuid::parse_str(&self.preparation_ref)
                .is_ok_and(|id| id.to_string() == self.preparation_ref)
            && self.source_version > 0
            && self.original.lineage.is_none()
            && self.activity.actor.kind == ActorKind::Unknown
            && self.activity.conversation.kind == ConversationKind::Private
            && self.activity.conversation.thread_id.is_none()
            && self.activity.thread.is_none()
            && matches!(&self.original.reply, ReplyAddress::Whatsapp { app_id, waba_id, phone_number_id, recipient_id, .. }
                if self.activity.source == (SourceKind::Whatsapp { app_id: app_id.clone(), waba_id: waba_id.clone(), phone_number_id: phone_number_id.clone() })
                && recipient_id == &card.actor && recipient_id == &card.chat)
            && self.phase_valid(card)
            && self.owner.routing.as_ref().is_some_and(|r| {
                r.targets.len() == 1
                    && r.targets[0].id
                        == card
                            .consumer
                            .clone()
                            .unwrap_or_else(cmaeg_core::ids::ConsumerId::default_consumer)
                    && r.source_id == self.activity.source.source_id()
                    && r.resolution == cmaeg_core::activity::RouteResolution::Interaction
                    && r.rule_id.is_none()
                    && r.causation.is_none()
                    && r.addressing_bytes == 0
            })
    }
}

impl Authority {
    fn phase_valid(&self, card: &Card) -> bool {
        let key_valid = |key: &str| key.len() == 64 && key.bytes().all(|b| b.is_ascii_hexdigit());
        let phase = match &self.phase {
            super::Phase::Prepared => card.reference.is_none(),
            super::Phase::MessageAttempted { key, attempt } => {
                key_valid(key)
                    && uuid::Uuid::parse_str(attempt).is_ok_and(|id| id.to_string() == *attempt)
                    && card.reference.is_none()
            }
            super::Phase::Declined { key, reason } => {
                key_valid(key)
                    && !reason.is_empty()
                    && reason.len() <= 128
                    && card.reference.is_none()
            }
            super::Phase::ConfirmedUnbound { key, receipt }
            | super::Phase::Bound { key, receipt } => {
                key_valid(key)
                    && receipt.kind == "native_whatsapp_receipt_v1"
                    && receipt.group.is_none()
                    && receipt.original.owner == card.owner
                    && receipt.original.channel_id == card.channel_id
                    && receipt.original.record_id == self.original.record_id
                    && receipt.original.event_id == card.event_id
                    && receipt.source == self.activity.source
                    && receipt.recipient == card.actor
                    && receipt.consumer
                        == card
                            .consumer
                            .clone()
                            .unwrap_or_else(cmaeg_core::ids::ConsumerId::default_consumer)
                    && receipt.provider == self.provider
                    && receipt.partition_key == self.original.partition_key
                    && receipt.conversation_id == self.original.conversation_id
                    && receipt.confirmed_at >= self.issued_at
                    && receipt.confirmed_at < card.expires_at
                    && receipt.retained_until
                        == receipt
                            .confirmed_at
                            .saturating_add(crate::whatsapp::receipts::RETENTION)
                    && cmaeg_core::channel::whatsapp::valid_message_id(&receipt.message_id)
            }
        };
        phase
            && matches!(
                (&self.phase, &self.pending, &card.consumed),
                (super::Phase::Bound { .. }, Some(_), Some(_)) | (_, None, None)
            )
    }
}

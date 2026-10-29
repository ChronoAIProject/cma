use super::super::{owner::DispatchOwner, Card, Error};
use crate::{channels::resolver::ResolvedChannel, native_lark::receipts::Receipt};
use cmaeg_core::{
    activity::{Activity, ActivityKind, ActorKind, ConversationKind, SourceKind},
    channel::NativeLarkReply,
    routing::{Resolution, RouteInput},
    seal::{EventRefPayload, ReplyAddress},
};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Authority {
    pub(crate) original: EventRefPayload,
    pub(crate) issued_at: i64,
    pub(crate) owner: DispatchOwner,
    pub(crate) conversation: ConversationKind,
    pub(crate) form: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) image: Option<super::Image>,
    /// Ordered multiple-image authority; never emitted for historical scalar cards.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) images: Option<Vec<super::Image>>,
    pub(crate) custody: String,
    pub(crate) destination: Option<Receipt>,
    pub(crate) replacement: Option<super::replacement::Patch>,
    pub(crate) pending: Option<super::click::Pending>,
}
pub(crate) fn route(reply: &NativeLarkReply) -> String {
    format!(
        "native-lark-card:{}:{}:{}:{}",
        reply.identity.region.as_str(),
        reply.identity.app_id,
        reply.identity.tenant_key,
        reply.chat_id
    )
}
pub(crate) fn eligible(
    channel: &ResolvedChannel,
    event: &EventRefPayload,
    activity: &Activity,
) -> bool {
    let (ReplyAddress::NativeLark(reply), SourceKind::NativeLark(identity)) =
        (&event.reply, &activity.source)
    else {
        return false;
    };
    event.validate().is_ok()
        && event.lineage.is_none()
        && event.verifies_activity(activity)
        && event.channel_id == channel.id
        && event.record_id == channel.pointer.record_id
        && reply.identity == *identity
        && activity.actor.kind == ActorKind::Human
        && activity.actor.id != identity.bot_open_id
        && activity.actor.id.starts_with("ou_")
        && matches!(
            activity.kind,
            ActivityKind::Message { .. } | ActivityKind::Interaction(_)
        )
        && matches!(
            activity.conversation.kind,
            ConversationKind::Private | ConversationKind::Group
        )
        && activity.conversation.id == reply.chat_id
        && activity.conversation.thread_id == reply.thread_id
        && activity.thread.is_none()
        && (reply.thread_id.is_none() || activity.conversation.kind == ConversationKind::Group)
        && crate::native_lark::credentials(&channel.definition, &event.reply).is_some()
        && channel
            .consumer(event.consumer.as_ref())
            .is_some_and(|c| !c.pending && c.credential.is_some())
}
pub(crate) fn hash(value: &impl Serialize) -> String {
    use sha2::{Digest, Sha256};
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(value).expect("native card data serializes"))
    )
}
impl Authority {
    pub(super) fn prepare(
        channel: &ResolvedChannel,
        event: &EventRefPayload,
        activity: &Activity,
        form: bool,
    ) -> Result<Self, Error> {
        if !eligible(channel, event, activity) {
            return Err(Error::Denied);
        }
        let empty = Default::default();
        let Resolution::Routed(decision) = cmaeg_core::routing::resolve(
            &channel.definition,
            &RouteInput {
                activity,
                chat_source: channel.definition.source_of(&activity.source),
                card_consumer: Some(event.consumer.as_ref()),
                published: None,
                non_human_providers: &empty,
                attributes_signed: true,
            },
            true,
        )
        .map_err(|_| Error::Denied)?
        else {
            return Err(Error::Denied);
        };
        Ok(Self {
            original: event.clone(),
            issued_at: super::super::now(),
            owner: DispatchOwner {
                consumer: event.consumer.clone(),
                conversation_id: event.conversation_id.clone(),
                partition_key: event.partition_key.clone(),
                routing: Some(decision.routing),
            },
            conversation: activity.conversation.kind,
            form,
            image: None,
            images: None,
            custody: String::new(),
            destination: None,
            replacement: None,
            pending: None,
        })
    }
    pub(crate) fn reply(&self) -> &NativeLarkReply {
        match &self.original.reply {
            ReplyAddress::NativeLark(reply) => reply,
            _ => unreachable!("validated native card"),
        }
    }
    pub(crate) fn images(&self) -> Result<&[super::Image], Error> {
        match (&self.image, &self.images) {
            (None, None) => Ok(&[]),
            (Some(image), None) => Ok(std::slice::from_ref(image)),
            (None, Some(images))
                if (2..=cmaeg_core::message::MAX_NATIVE_ATTACHMENTS).contains(&images.len()) =>
            {
                Ok(images)
            }
            _ => Err(Error::Denied),
        }
    }
    pub(super) fn digest(&self, card: &Card, channel: &ResolvedChannel) -> Option<String> {
        crate::native_lark::credentials(&channel.definition, &self.original.reply)?;
        let consumer = channel
            .consumer(card.consumer.as_ref())
            .filter(|c| !c.pending)?;
        let mut facts = json!({"domain":"cmaeg.native-lark-card.v1", "channel":channel.id,"record":channel.pointer.record_id,
            "owner":channel.definition.owner,"consumer":self.owner,"provider":consumer.provider,
            "key":channel.consumer_key_hash(card.consumer.as_ref())?,"credential":crate::channels::definition::hash_secret(consumer.credential?.as_str()),
            "routing":channel.definition.routing,"original":self.original,"actor":card.actor,"conversation":self.conversation,
            "thread":card.thread,"id":card.id,"revision":card.revision,"message":card.message,"replacement":self.replacement.as_ref().map(|p| &p.original_card),"issued_at":self.issued_at,"expires_at":card.expires_at,"form":self.form});
        // Omit the field entirely for historical cards, preserving their custody digest.
        if let Some(image) = &self.image {
            facts["image"] = serde_json::to_value(image).ok()?;
        }
        if let Some(images) = &self.images {
            facts["images"] = serde_json::to_value(images).ok()?;
        }
        Some(hash(&facts))
    }
    pub(crate) fn current(&self, card: &Card, channel: &ResolvedChannel) -> bool {
        self.valid(card)
            && channel.id == card.channel_id
            && channel.pointer.record_id == self.original.record_id
            && channel.definition.owner.subject == card.owner
            && self.destination.as_ref().is_none_or(|receipt| {
                channel
                    .consumer(card.consumer.as_ref())
                    .is_some_and(|consumer| receipt.provider == *consumer.provider)
            })
            && self.digest(card, channel).as_ref() == Some(&self.custody)
    }
    pub(super) fn valid(&self, card: &Card) -> bool {
        let ReplyAddress::NativeLark(reply) = &self.original.reply else {
            return false;
        };
        let target = card
            .consumer
            .clone()
            .unwrap_or_else(cmaeg_core::ids::ConsumerId::default_consumer);
        card.native_whatsapp_flow.is_none()
            && card.lark.is_none()
            && card.whatsapp.is_none()
            && card.native_whatsapp.is_none()
            && card.discord.is_none()
            && card.discord_edit.is_none()
            && card.discord_effect.is_none()
            && card.discord_form_launch.is_none()
            && card.lark_delete.is_none()
            && card.destination.is_none()
            && self.destination.is_some() == card.reference.is_some()
            && card.callback_owner.is_none()
            && self.original.validate().is_ok()
            && self.original.activity_sha256.is_some()
            && self.original.lineage.is_none()
            && self.original.channel_id == card.channel_id
            && self.original.event_id == card.event_id
            && self.original.consumer == card.consumer
            && card.route_id == route(reply)
            && card.chat == reply.chat_id
            && card.thread == reply.thread_id
            && self.owner.consumer == card.consumer
            && self.owner.conversation_id == self.original.conversation_id
            && self.owner.partition_key == self.original.partition_key
            && self.owner.routing.as_ref().is_some_and(|r| {
                r.resolution == cmaeg_core::activity::RouteResolution::Interaction
                    && r.targets.len() == 1
                    && r.targets[0].id == target
                    && r.source_id == SourceKind::NativeLark(reply.identity.clone()).source_id()
                    && r.rule_id.is_none()
                    && r.causation.is_none()
                    && r.addressing_bytes == 0
            })
            && matches!(
                self.conversation,
                ConversationKind::Private | ConversationKind::Group
            )
            && (card.thread.is_none() || self.conversation == ConversationKind::Group)
            && card.revision > 0
            && self.custody.len() == 64
            && self.issued_at > 0
            && card.expires_at > self.issued_at
            && card.expires_at <= self.issued_at.saturating_add(super::super::TTL)
            && super::markup(card).is_ok()
            && self.destination.as_ref().is_none_or(|r| {
                r.original.channel_id == card.channel_id
                    && r.original.event_id == card.event_id
                    && r.original.record_id == self.original.record_id
                    && r.original.owner == card.owner
                    && r.consumer == target
                    && r.source == SourceKind::NativeLark(reply.identity.clone())
                    && r.recipient == card.chat
                    && r.message_id == r.reply.message_id
                    && r.reply.chat_id == card.chat
                    && r.reply.identity == reply.identity
                    && r.reply.revision == reply.revision
                    && r.reply.thread_id == card.thread
                    && r.partition_key == self.owner.partition_key
                    && r.conversation_id == self.owner.conversation_id
            })
            && self.replacement.as_ref().is_none_or(|p| {
                uuid::Uuid::parse_str(&p.original_card).is_ok()
                    && p.original_card != card.id
                    && card.revision > 1
                    && (self.destination.is_none() || p.confirmed)
            })
            && (self.pending.is_some() == card.consumed.is_some())
            && self
                .pending
                .as_ref()
                .is_none_or(|p| super::click::valid_pending(card, p))
    }
}

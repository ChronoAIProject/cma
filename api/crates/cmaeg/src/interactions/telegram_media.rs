//! Compact custody for one native upload and its inline keyboard.
//! Upload bytes/URLs never enter the bounded interaction record.
use super::{owner, Card, Error};
use crate::{channels::resolver::ResolvedChannel, media::outbound::PreparedFile};
use cmaeg_core::{
    activity::{Activity, ActorKind, RouteResolution, SourceKind},
    channel::{Platform, ProviderBinding},
    ids::RecordId,
    message::{Block, NativeMediaKind},
    routing::{Resolution, RouteInput},
    seal::{EventRefPayload, ReplyAddress},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Authority {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) snapshot: Option<crate::native_media::model::Snapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) edit: Option<super::telegram_media_edit::Edit>,
    pub(crate) owner: owner::DispatchOwner,
    provider: ProviderBinding,
    source: SourceKind,
    source_revision: String,
    record_id: RecordId,
    pub(crate) kind: NativeMediaKind,
    sha256: String,
    bytes: u64,
    filename: String,
    mime: String,
    pub(crate) caption: String,
}

pub(crate) fn eligible(
    channel: &ResolvedChannel,
    event: &EventRefPayload,
    activity: &Activity,
) -> bool {
    channel.definition.routing.is_some()
        && owner::eligible(channel, event, activity)
        && activity.actor.kind == ActorKind::Human
        && activity.thread.is_none()
        && matches!((&event.reply, &activity.source),
            (ReplyAddress::Telegram {bot_id,chat_id,thread_id,..},
             SourceKind::Telegram {platform:Platform::Telegram,bot_id:source})
            if bot_id == source && chat_id == &activity.conversation.id
                && thread_id == &activity.conversation.thread_id
                && positive(bot_id) && chat_id.parse::<i64>().is_ok_and(|id| id != 0 && id.to_string() == *chat_id)
                && thread_id.as_ref().is_none_or(|id| positive(id)))
}
fn positive(id: &str) -> bool {
    id.parse::<i64>()
        .is_ok_and(|n| n > 0 && n.to_string() == id)
}
fn revision(channel: &ResolvedChannel, source: &SourceKind) -> Result<String, Error> {
    let source = channel.definition.source_of(source).ok_or(Error::Denied)?;
    let value = serde_json::to_value(source).map_err(|_| Error::Denied)?;
    Ok(crate::channels::definition::hash_secret(
        &cmaeg_core::profile::canonical_json(value),
    ))
}
impl Authority {
    pub(super) fn same_custody(&self, other: &Self) -> bool {
        self.owner == other.owner
            && self.provider == other.provider
            && self.source == other.source
            && self.source_revision == other.source_revision
            && self.record_id == other.record_id
            && self.kind == other.kind
            && self.sha256 == other.sha256
            && self.bytes == other.bytes
            && self.filename == other.filename
            && self.mime == other.mime
    }
    pub(super) fn prepare(
        channel: &ResolvedChannel,
        event: &EventRefPayload,
        activity: &Activity,
        kind: NativeMediaKind,
        file: &PreparedFile,
    ) -> Result<Self, Error> {
        if !eligible(channel, event, activity)
            || matches!(
                kind,
                NativeMediaKind::Sticker | NativeMediaKind::Voice | NativeMediaKind::Animation
            )
        {
            return Err(Error::Denied);
        }
        let consumer = channel
            .consumer(event.consumer.as_ref())
            .ok_or(Error::Denied)?;
        let input = RouteInput {
            activity,
            chat_source: channel.definition.source_of(&activity.source),
            card_consumer: Some(event.consumer.as_ref()),
            published: None,
            non_human_providers: &Default::default(),
            attributes_signed: true,
        };
        let Resolution::Routed(decision) =
            cmaeg_core::routing::resolve(&channel.definition, &input, true)
                .map_err(|_| Error::Denied)?
        else {
            return Err(Error::Denied);
        };
        Ok(Self {
            snapshot: None,
            edit: None,
            owner: owner::DispatchOwner {
                consumer: event.consumer.clone(),
                conversation_id: event.conversation_id.clone(),
                partition_key: event.partition_key.clone(),
                routing: Some(decision.routing),
            },
            provider: consumer.provider.clone(),
            source: activity.source.clone(),
            source_revision: revision(channel, &activity.source)?,
            record_id: event.record_id.clone(),
            kind,
            sha256: format!("sha256:{:x}", Sha256::digest(&file.bytes)),
            bytes: file.bytes.len() as u64,
            filename: file.filename.clone(),
            mime: file.mime.clone(),
            caption: file.caption.clone().unwrap_or_default(),
        })
    }
    pub(super) fn current(&self, card: &Card, channel: &ResolvedChannel) -> bool {
        let SourceKind::Telegram { bot_id, .. } = &self.source else {
            return false;
        };
        channel.definition.routing.is_some()
            && card.owner == channel.definition.owner.subject
            && card.channel_id == channel.id
            && self.record_id == channel.definition.record_id
            && self.record_id == channel.pointer.record_id
            && revision(channel, &self.source).ok().as_ref() == Some(&self.source_revision)
            && channel
                .definition
                .secrets
                .telegram_bot_token
                .as_ref()
                .is_some_and(|token| crate::telegram::media_token_matches(token, bot_id))
            && channel
                .consumer(self.owner.consumer.as_ref())
                .is_some_and(|current| {
                    !current.pending
                        && current.credential.is_some()
                        && current.provider == &self.provider
                })
    }
    pub(super) fn admitted(
        &self,
        card: &Card,
        channel: &ResolvedChannel,
        activity: &Activity,
    ) -> bool {
        self.current(card, channel)
            && self.source == activity.source
            && activity.actor.kind == ActorKind::Human
            && activity.thread.is_none()
    }
}
pub(super) fn valid(card: &Card) -> bool {
    let Some(a) = &card.native_telegram_media else {
        return true;
    };
    let SourceKind::Telegram {
        platform: Platform::Telegram,
        bot_id,
    } = &a.source
    else {
        return false;
    };
    let Some(routing) = &a.owner.routing else {
        return false;
    };
    let target = a
        .owner
        .consumer
        .clone()
        .unwrap_or_else(cmaeg_core::ids::ConsumerId::default_consumer);
    card.lark.is_none()
        && card.whatsapp.is_none()
        && card.native_whatsapp.is_none()
        && card.native_whatsapp_flow.is_none()
        && card.native_lark.is_none()
        && card.discord.is_none()
        && card.consumer == a.owner.consumer
        && !card.consumer.as_ref().is_some_and(|id| id.is_default())
        && card.message.validate().is_ok()
        && card.message.native_attachments.is_empty()
        && !card.message.thread_reply
        && card.message.reply_to.is_none()
        && card.message.whatsapp_template.is_none()
        && card.message.blocks.iter().all(|block| {
            matches!(
                block,
                Block::Text { .. }
                    | Block::Buttons { .. }
                    | Block::Choice { .. }
                    | Block::Link { .. }
            )
        })
        && (card.revision > 1 || super::has_controls(&card.message))
        && card.revision > 0
        && super::telegram_media_edit::valid(card)
        && card.thread.as_deref().is_none_or(positive)
        && a.owner.conversation_id.is_wellformed()
        && !a.owner.partition_key.is_empty()
        && a.owner.partition_key.len() <= 4096
        && positive(bot_id)
        && card.route_id == crate::telegram::route(bot_id, &card.chat)
        && a.bytes > 0
        && a.bytes <= crate::media::outbound::MAX_OUTBOUND_BYTES
        && a.sha256.len() == 71
        && a.sha256.starts_with("sha256:")
        && a.sha256[7..].bytes().all(|b| b.is_ascii_hexdigit())
        && a.source_revision.len() == 71
        && a.source_revision.starts_with("sha256:")
        && a.source_revision[7..]
            .bytes()
            .all(|b| b.is_ascii_hexdigit())
        && !a.filename.is_empty()
        && a.filename.len() <= 255
        && !a.mime.is_empty()
        && a.mime.len() <= 255
        && a.caption.encode_utf16().count() <= 1024
        && !matches!(
            a.kind,
            NativeMediaKind::Sticker | NativeMediaKind::Voice | NativeMediaKind::Animation
        )
        && routing.resolution == RouteResolution::Interaction
        && routing.targets.len() == 1
        && routing.targets[0].id == target
        && routing.source_id == a.source.source_id()
        && routing.rule_id.is_none()
        && routing.causation.is_none()
        && routing.addressing_bytes == 0
        && match (&card.callback_owner, &a.owner.consumer) {
            (Some(owner), Some(_)) => {
                owner::DispatchOwner::from(owner.clone()) == a.owner
                    && owner.provider == a.provider
                    && owner.source == a.source
            }
            (None, None) => true,
            _ => false,
        }
}

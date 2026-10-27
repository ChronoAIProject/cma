//! A named card retains its original conversation and exact provider binding.
//! Callback dispatch uses that immutable owner; it never re-routes or fans out.
use super::{Card, Error};
use crate::channels::resolver::ResolvedChannel;
use cmaeg_core::{
    activity::{Activity, ActivityRouting, SourceKind},
    channel::ProviderBinding,
    ids::{ConsumerId, ConversationId},
    routing::{Resolution, RouteInput},
    seal::{EventRefPayload, ReplyAddress},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallbackOwner {
    pub consumer: ConsumerId,
    pub provider: ProviderBinding,
    pub conversation_id: ConversationId,
    pub partition_key: String,
    pub routing: ActivityRouting,
    pub source: SourceKind,
}

/// Dispatch identity also represents a form issued to the default consumer.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DispatchOwner {
    pub consumer: Option<ConsumerId>,
    pub conversation_id: ConversationId,
    pub partition_key: String,
    pub routing: Option<ActivityRouting>,
}
impl From<CallbackOwner> for DispatchOwner {
    fn from(owner: CallbackOwner) -> Self {
        Self {
            consumer: Some(owner.consumer),
            conversation_id: owner.conversation_id,
            partition_key: owner.partition_key,
            routing: Some(owner.routing),
        }
    }
}

pub(crate) fn eligible(
    channel: &ResolvedChannel,
    event: &EventRefPayload,
    activity: &Activity,
) -> bool {
    event.lineage.is_none()
        && reply_matches(event, activity)
        && channel
            .consumer(event.consumer.as_ref())
            .is_some_and(|c| !c.pending && c.credential.is_some())
}

pub(super) fn prepare(
    channel: &ResolvedChannel,
    event: &EventRefPayload,
    activity: &Activity,
) -> Result<Option<CallbackOwner>, Error> {
    let Some(id) = &event.consumer else {
        return Ok(None);
    };
    // A hand-off has no persisted original human callback audience in this
    // protocol. A publishing agent's identity cannot substitute for one.
    if !eligible(channel, event, activity) {
        return Err(Error::Denied);
    }
    let consumer = channel
        .consumer(Some(id))
        .filter(|c| !c.pending && c.credential.is_some())
        .ok_or(Error::Denied)?;
    let empty = Default::default();
    let input = RouteInput {
        activity,
        chat_source: channel.definition.source_of(&activity.source),
        card_consumer: Some(Some(id)),
        published: None,
        non_human_providers: &empty,
        attributes_signed: true,
    };
    let Resolution::Routed(decision) =
        cmaeg_core::routing::resolve(&channel.definition, &input, true)
            .map_err(|_| Error::Denied)?
    else {
        return Err(Error::Denied);
    };
    Ok(Some(CallbackOwner {
        consumer: id.clone(),
        provider: consumer.provider.clone(),
        conversation_id: event.conversation_id.clone(),
        partition_key: event.partition_key.clone(),
        routing: decision.routing,
        source: activity.source.clone(),
    }))
}

pub(super) fn valid(card: &Card) -> bool {
    let Some(owner) = &card.callback_owner else {
        return true;
    };
    card.consumer.as_ref() == Some(&owner.consumer)
        && !owner.consumer.is_default()
        && owner.routing.resolution == cmaeg_core::activity::RouteResolution::Interaction
        && owner.routing.targets.len() == 1
        && owner.routing.targets[0].id == owner.consumer
        && owner.routing.source_id == owner.source.source_id()
        && owner.routing.causation.is_none()
        && owner.routing.rule_id.is_none()
        && owner.routing.addressing_bytes == 0
        && !owner.partition_key.is_empty()
        && owner.partition_key.len() <= 4096
}

/// Before consuming, verify today's owner still denotes the same binding.
/// A credential rotation is allowed; removal, pending state or rebinding is not.
pub(super) fn current(card: &Card, channel: &ResolvedChannel) -> bool {
    match (&card.consumer, &card.callback_owner) {
        (None, None) => true,
        (Some(id), Some(owner)) if id == &owner.consumer => {
            channel.consumer(Some(id)).is_some_and(|current| {
                !current.pending
                    && current.credential.is_some()
                    && current.provider == &owner.provider
            })
        }
        // Legacy named cards never had executable callback ownership.
        _ => false,
    }
}

pub(super) fn admitted(card: &Card, channel: &ResolvedChannel, activity: &Activity) -> bool {
    current(card, channel)
        && card
            .callback_owner
            .as_ref()
            .is_none_or(|owner| owner.source == activity.source)
}

fn reply_matches(event: &EventRefPayload, activity: &Activity) -> bool {
    match (&event.reply, &activity.source) {
        (
            ReplyAddress::NyxidRelay {
                platform, route_id, ..
            },
            SourceKind::NyxidRelay {
                platform: source_platform,
                route_id: source_route,
            },
        ) => platform == source_platform && route_id == source_route,
        (
            ReplyAddress::Discord {
                bot_id, channel_id, ..
            },
            SourceKind::Discord { bot_id: source_bot },
        ) => bot_id == source_bot && channel_id == &activity.conversation.id,
        (
            ReplyAddress::Telegram {
                bot_id,
                chat_id,
                thread_id,
                ..
            },
            SourceKind::Telegram {
                platform: cmaeg_core::channel::Platform::Telegram,
                bot_id: source_bot,
            },
        ) => {
            bot_id == source_bot
                && chat_id == &activity.conversation.id
                && thread_id == &activity.conversation.thread_id
        }
        (
            ReplyAddress::Whatsapp {
                app_id,
                waba_id,
                phone_number_id,
                recipient_id,
                ..
            },
            SourceKind::Whatsapp {
                app_id: app,
                waba_id: account,
                phone_number_id: phone,
            },
        ) => {
            app_id == app
                && waba_id == account
                && phone_number_id == phone
                && recipient_id == &activity.actor.id
                && recipient_id == &activity.conversation.id
                && super::whatsapp::numeric(recipient_id)
                && activity.actor.kind == cmaeg_core::activity::ActorKind::Unknown
                && activity.conversation.kind == cmaeg_core::activity::ConversationKind::Private
                && activity.conversation.thread_id.is_none()
                && activity.thread.is_none()
        }
        _ => false,
    }
}

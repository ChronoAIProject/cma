//! Immutable upload custody binds original admission, provider credential and exact consumer.
use super::{
    denied,
    model::{Current, Original},
    now,
};
use crate::{
    auth::AgentCaller, channels::resolver::ResolvedChannel, tools::message_effects::mutation::hash,
};
use cmaeg_core::{
    activity::{Activity, ActorKind, ConversationKind, SourceKind},
    channel::{Secret, Source},
    ids::MessageRef,
    problem::{Problem, ProblemCode},
    seal::{
        native_media_edit::{
            NativeMediaDestination as Destination, NativeMediaEditAuthority, MAX_AGE,
        },
        EventRefPayload, MasterKeyRing, MessageRefPayload, ReplyAddress,
    },
};

pub(crate) fn destination(event: &EventRefPayload) -> Option<Destination> {
    match &event.reply {
        ReplyAddress::Telegram {
            bot_id,
            chat_id,
            thread_id,
            ..
        } => Some(Destination::Telegram {
            bot_id: bot_id.clone(),
            chat_id: chat_id.clone(),
            thread_id: thread_id.clone(),
        }),
        ReplyAddress::Discord {
            bot_id, channel_id, ..
        } => Some(Destination::Discord {
            bot_id: bot_id.clone(),
            channel_id: channel_id.clone(),
        }),
        _ => None,
    }
}
pub(crate) fn custody(
    channel: &ResolvedChannel,
    event: &EventRefPayload,
    activity: &Activity,
) -> Option<String> {
    let destination = destination(event)?;
    let consumer = channel.consumer(event.consumer.as_ref())?;
    let source = channel.definition.sources.iter().find(|s| matches!((s,&destination),
        (Source::Telegram {bot_id,..}, Destination::Telegram {bot_id:expected,..}) |
        (Source::Discord {bot_id,..}, Destination::Discord {bot_id:expected,..}) if bot_id == expected))?;
    if channel.id != event.channel_id
        || channel.pointer.record_id != event.record_id
        || event.validate().is_err()
        || event.lineage.is_some()
        || !event.verifies_activity(activity)
        || consumer.pending
        || activity.actor.kind != ActorKind::Human
    {
        return None;
    }
    match (&destination, &activity.source) {
        (
            Destination::Telegram {
                bot_id,
                chat_id,
                thread_id,
            },
            SourceKind::Telegram { bot_id: actual, .. },
        ) => {
            if actual != bot_id
                || chat_id != &activity.conversation.id
                || thread_id != &activity.conversation.thread_id
                || !matches!(
                    activity.conversation.kind,
                    ConversationKind::Private | ConversationKind::Group
                )
                || !crate::tools::message_effects::authority::event_supported(
                    channel, event, activity,
                )
            {
                return None;
            }
        }
        (
            Destination::Discord { bot_id, channel_id },
            SourceKind::Discord { bot_id: actual, .. },
        ) => {
            if actual != bot_id || channel_id != &activity.conversation.id {
                return None;
            }
            let ReplyAddress::Discord { thread_id, .. } = &event.reply else {
                return None;
            };
            if thread_id != &activity.conversation.thread_id {
                return None;
            }
        }
        _ => return None,
    }
    token(channel, &destination)?;
    Some(hash(
        &serde_json::json!({"domain":"cmaeg.native-media-custody.v1",
        "channel":channel.id,"record":channel.pointer.record_id,"owner":channel.definition.owner,
        "source":source,"routing":channel.definition.routing,"consumer":event.consumer,
        "provider":consumer.provider,"consumer_key":channel.consumer_key_hash(event.consumer.as_ref())?,
        "provider_credential":crate::channels::definition::hash_secret(consumer.credential?.as_str()),
        "event":event,"activity":activity}),
    ))
}
pub(crate) fn token<'a>(
    channel: &'a ResolvedChannel,
    destination: &Destination,
) -> Option<&'a Secret> {
    match destination {
        Destination::Telegram { bot_id, .. } => channel
            .definition
            .secrets
            .telegram_bot_token
            .as_ref()
            .filter(|t| crate::telegram::media_token_matches(t, bot_id)),
        Destination::Discord { bot_id, .. } => crate::discord::rest::token(channel, bot_id),
    }
}
pub(crate) fn check(
    channel: &ResolvedChannel,
    message: &MessageRefPayload,
    original: &Original,
    caller: AgentCaller,
) -> Result<(), Problem> {
    let a = message.native_media_edit.as_ref().ok_or_else(denied)?;
    if message.validate().is_err()
        || a.issued_at > now()
        || message.channel_id != channel.id
        || original.owner != channel.definition.owner.subject
        || original.message_id != message.platform_message_id
        || original.event.event_id != message.event_id
        || original.event.consumer != message.consumer
        || a.record_id != channel.pointer.record_id
        || a.original_sha256 != hash(original)
        || destination(&original.event).as_ref() != Some(&a.destination)
        || a.kind != original.snapshot.kind
        || a.issued_at != original.confirmed_at
        || a.expires_at != original.expires_at
        || a.authority_sha256 != original.authority_sha256
        || custody(channel, &original.event, &original.activity).as_ref()
            != Some(&a.authority_sha256)
    {
        return Err(denied());
    }
    crate::tools::state::owner(
        caller,
        channel,
        message.consumer.as_ref(),
        "nativeMediaUpdate",
    )?;
    if a.expires_at <= now() {
        return Err(Problem::new(
            ProblemCode::EventRefExpired,
            410,
            "native_media_authority_expired",
        ));
    }
    Ok(())
}
pub(crate) fn reference(
    original: &Original,
    current: &Current,
    keys: &MasterKeyRing,
    base: Option<&MessageRef>,
) -> Option<MessageRef> {
    let destination = destination(&original.event)?;
    let mut payload = match base {
        Some(reference) => reference.open(keys).ok()?.payload,
        None => MessageRefPayload {
            native_media_edit: None,
            received_reaction: None,
            native_lark_effect: None,
            whatsapp_reaction: None,
            discord_message: None,
            telegram_effect: None,
            lark_effect: None,
            channel_id: original.event.channel_id.clone(),
            event_id: original.event.event_id.clone(),
            platform_message_id: original.message_id.clone(),
            route_id: destination.route(),
            interaction_record_id: None,
            relay_message: None,
            consumer: original.event.consumer.clone(),
        },
    };
    // All revisions share the original validity window and exact sibling destination.
    if let Some(a) = &mut payload.telegram_effect {
        a.issued_at = original.confirmed_at;
        a.expires_at = original.expires_at;
    }
    if let Some(a) = &mut payload.discord_message {
        if let Some(w) = &mut a.effects {
            w.issued_at = original.confirmed_at;
            w.expires_at = original.expires_at;
        }
    }
    payload.native_media_edit = Some(Box::new(NativeMediaEditAuthority {
        destination,
        record_id: original.event.record_id.clone(),
        original_sha256: hash(original),
        authority_sha256: original.authority_sha256.clone(),
        kind: original.snapshot.kind,
        revision: current.revision,
        issued_at: original.confirmed_at,
        expires_at: original
            .expires_at
            .min(original.confirmed_at.saturating_add(MAX_AGE)),
    }));
    payload.validate().ok()?;
    MessageRef::seal(&payload, keys).ok()
}

//! Current custody of a reaction-only received Telegram/Discord message scope.
use super::{receipts::hash, ToolsState};
use crate::{auth::AgentCaller, channels::resolver::ResolvedChannel};
use cmaeg_core::{
    activity::{Activity, ActivityKind, ActorKind, SourceKind},
    channel::Source,
    ids::MessageRef,
    problem::{Problem, ProblemCode},
    seal::{
        received_reaction::{
            ReceivedReactionAuthority, ReceivedReactionPlatform as Platform, MAX_AGE,
        },
        EventRefPayload, MasterKeyRing, MessageRefPayload, ReplyAddress,
    },
};

fn revision(channel: &ResolvedChannel, message: &MessageRefPayload) -> Option<String> {
    let a = message.received_reaction.as_ref()?;
    let source = channel
        .definition
        .sources
        .iter()
        .find(|source| match (a.platform, source) {
            (Platform::Telegram, Source::Telegram { bot_id, .. })
            | (Platform::Discord, Source::Discord { bot_id, .. }) => *bot_id == a.bot_id,
            _ => false,
        })?;
    let token = match a.platform {
        Platform::Telegram => channel
            .definition
            .secrets
            .telegram_bot_token
            .as_ref()
            .filter(|t| {
                t.as_str()
                    .split_once(':')
                    .is_some_and(|(id, _)| id == a.bot_id)
            })?,
        Platform::Discord => crate::discord::rest::token(channel, &a.bot_id)?,
    };
    let consumer = channel
        .consumer(message.consumer.as_ref())
        .filter(|c| !c.pending && c.credential.is_some())?;
    Some(hash(&serde_json::json!({
        "domain":"cmaeg.received-reaction-custody.v1",
        "source":crate::telegram::messaging_source(source),"channel":channel.id,"record":channel.pointer.record_id,
        "event":message.event_id,"message":message.platform_message_id,"original_event":a.event_sha256,
        "platform":a.platform,"bot":a.bot_id,"chat":a.chat_id,"actor":a.actor_id,"thread":a.thread_id,
        "received_at":a.received_at,"expires_at":a.expires_at,
        "owner":channel.definition.owner,"consumer":message.consumer,"provider":consumer.provider,
        "agent_key":channel.consumer_key_hash(message.consumer.as_ref())?,
        "consumer_credential":crate::channels::definition::hash_secret(consumer.credential?.as_str()),
        "token":crate::channels::definition::hash_secret(token.as_str()),
        "telegram_webhook":channel.definition.secrets.telegram_webhook_secret.as_ref()
            .filter(|_| a.platform == Platform::Telegram)
            .map(|s| crate::channels::definition::hash_secret(s.as_str())),
        "routing":channel.definition.routing,
    })))
}

/// Called only after `current_received_message` proved the original retained plan.
pub(super) fn issue(
    channel: &ResolvedChannel,
    event: &EventRefPayload,
    activity: &Activity,
    keys: &MasterKeyRing,
) -> Option<MessageRef> {
    if event.lineage.is_some()
        || event.validate().is_err()
        || !event.verifies_activity(activity)
        || event.channel_id != channel.id
        || event.record_id != channel.pointer.record_id
        || activity.actor.kind != ActorKind::Human
        || !matches!(activity.kind, ActivityKind::Message { album_id: None, .. })
    {
        return None;
    }
    let (platform, bot, chat, original, thread) = match (&activity.source, &event.reply) {
        (
            SourceKind::Telegram {
                bot_id: observed, ..
            },
            ReplyAddress::Telegram {
                bot_id,
                chat_id,
                message_id,
                thread_id,
            },
        ) if observed == bot_id && super::authority::event_supported(channel, event, activity) => {
            (Platform::Telegram, bot_id, chat_id, message_id, thread_id)
        }
        (
            SourceKind::Discord { bot_id: observed },
            ReplyAddress::Discord {
                bot_id,
                channel_id,
                message_id,
                thread_id,
            },
        ) if observed == bot_id && super::discord::event_supported(channel, event, activity) => {
            (Platform::Discord, bot_id, channel_id, message_id, thread_id)
        }
        _ => return None,
    };
    if activity.conversation.id != *chat || activity.conversation.thread_id != *thread {
        return None;
    }
    let received_at = time::OffsetDateTime::parse(
        &activity.occurred_at,
        &time::format_description::well_known::Rfc3339,
    )
    .ok()?
    .unix_timestamp();
    let now = super::now();
    let expires_at = event.expires_at.min(received_at.saturating_add(MAX_AGE));
    if received_at <= 0 || received_at > now || expires_at <= now {
        return None;
    }
    let a = ReceivedReactionAuthority {
        platform,
        bot_id: bot.clone(),
        chat_id: chat.clone(),
        actor_id: activity.actor.id.clone(),
        thread_id: thread.clone(),
        event_sha256: hash(event),
        authority_sha256: String::new(),
        received_at,
        expires_at,
    };
    let mut message = MessageRefPayload {
        route_id: a.route(),
        native_lark_effect: None,
        native_media_edit: None,
        received_reaction: Some(Box::new(a)),
        channel_id: event.channel_id.clone(),
        event_id: event.event_id.clone(),
        platform_message_id: original.clone(),
        consumer: event.consumer.clone(),
        whatsapp_reaction: None,
        discord_message: None,
        telegram_effect: None,
        lark_effect: None,
        interaction_record_id: None,
        relay_message: None,
    };
    let stamp = revision(channel, &message)?;
    message.received_reaction.as_mut()?.authority_sha256 = stamp;
    message.validate().ok()?;
    MessageRef::seal(&message, keys).ok()
}

pub(super) fn check(
    message: &MessageRefPayload,
    channel: &ResolvedChannel,
    caller: AgentCaller,
) -> Result<(), Problem> {
    let denied = || {
        Problem::new(
            ProblemCode::Forbidden,
            403,
            "received_message_reaction_revoked",
        )
    };
    let a = message.received_reaction.as_ref().ok_or_else(denied)?;
    if message.validate().is_err()
        || a.received_at > super::now()
        || message.channel_id != channel.id
        || revision(channel, message).as_ref() != Some(&a.authority_sha256)
    {
        return Err(denied());
    }
    super::super::state::owner(
        caller,
        channel,
        message.consumer.as_ref(),
        "setMessageReaction",
    )?;
    if a.expires_at <= super::now() {
        return Err(Problem::new(
            ProblemCode::EventRefExpired,
            410,
            "received_message_reaction_expired",
        ));
    }
    Ok(())
}

impl ToolsState {
    pub(crate) fn received_reaction_enabled(&self, platform: Platform) -> bool {
        match platform {
            Platform::Telegram => self
                .telegram_effects
                .as_ref()
                .is_some_and(|c| c.received_issuance_enabled),
            Platform::Discord => self
                .discord_effects
                .as_ref()
                .is_some_and(|c| c.received_issuance_enabled),
        }
    }
}

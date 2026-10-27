//! Issue authority only from the authenticated original context and confirmed send.
use crate::{auth::AgentCaller, channels::resolver::ResolvedChannel};
use cmaeg_core::{
    activity::{Activity, SourceKind},
    channel::{LarkMessageEffectsBinding, Source},
    ids::MessageRef,
    problem::{Problem, ProblemCode},
    seal::{
        lark_effect::{LarkEffectAuthority, LARK_EFFECT_TTL},
        EventRefPayload, MasterKeyRing, MessageRefPayload, ReplyAddress,
    },
};
use sha2::{Digest, Sha256};

fn source<'a>(
    channel: &'a ResolvedChannel,
    route: &str,
) -> Option<(&'a Source, &'a str, &'a LarkMessageEffectsBinding)> {
    let source @ Source::NyxidRelay {
        bot_id: Some(bot),
        lark_message_effects: Some(binding),
        trust_normalized: false,
        ..
    } = channel.definition.relay_source_for_route(route)?
    else {
        return None;
    };
    Some((source, bot, binding))
}

fn revision(channel: &ResolvedChannel, message: &MessageRefPayload) -> Option<String> {
    let (source, _, _) = source(channel, &message.route_id)?;
    let consumer = channel
        .consumer(message.consumer.as_ref())
        .filter(|c| !c.pending && c.credential.is_some())?;
    let key = channel.definition.secrets.channel_key.as_ref()?;
    Some(super::receipts::hash(&serde_json::json!({
        "source":source, "channel":channel.id, "record":channel.pointer.record_id,
        "owner":channel.definition.owner, "consumer":message.consumer,
        "provider":consumer.provider, "agent_key":channel.consumer_key_hash(message.consumer.as_ref())?,
        "provider_credential":format!("{:x}", Sha256::digest(consumer.credential?.as_str().as_bytes())),
        "routing":channel.definition.routing, "credential":format!("{:x}", Sha256::digest(key.as_str().as_bytes()))
    })))
}

pub(crate) fn event_supported(
    channel: &ResolvedChannel,
    event: &EventRefPayload,
    activity: &Activity,
) -> bool {
    let (
        ReplyAddress::NyxidRelay {
            platform,
            route_id,
            thread_reply: false,
            ..
        },
        SourceKind::NyxidRelay {
            platform: observed,
            route_id: route,
        },
    ) = (&event.reply, &activity.source)
    else {
        return false;
    };
    event.lineage.is_none()
        && event.verifies_activity(activity)
        && event.channel_id == channel.id
        && event.record_id == channel.pointer.record_id
        && platform == observed
        && route_id == route
        && matches!(
            platform,
            cmaeg_core::channel::Platform::Lark | cmaeg_core::channel::Platform::Feishu
        )
        && activity.conversation.thread_id.is_none()
        && activity.thread.is_none()
        && source(channel, route_id).is_some_and(|(source, _, _)| matches!(source, Source::NyxidRelay { platform: configured, .. } if configured == platform))
}

pub(crate) fn enrich(
    reference: &MessageRef,
    event: &EventRefPayload,
    activity: &Activity,
    channel: &ResolvedChannel,
    keys: &MasterKeyRing,
    outbound: &str,
) -> Option<MessageRef> {
    if !event_supported(channel, event, activity) {
        return None;
    }
    let mut message = reference.open(keys).ok()?.payload;
    if message.event_id != event.event_id
        || message.channel_id != event.channel_id
        || message.consumer != event.consumer
    {
        return None;
    }
    let (Source::NyxidRelay { platform, .. }, bot, binding) = source(channel, &message.route_id)?
    else {
        return None;
    };
    let issued_at = super::now();
    message.lark_effect = Some(Box::new(LarkEffectAuthority {
        platform: *platform,
        binding: binding.clone(),
        bot_id: bot.into(),
        chat_id: activity.conversation.id.clone(),
        thread_id: activity.conversation.thread_id.clone(),
        outbound_message_id: outbound.into(),
        authority_sha256: revision(channel, &message)?,
        issued_at,
        expires_at: issued_at.saturating_add(LARK_EFFECT_TTL),
    }));
    message.validate().ok()?;
    MessageRef::seal(&message, keys).ok()
}

pub(crate) fn check(
    message: &MessageRefPayload,
    channel: &ResolvedChannel,
    caller: AgentCaller,
) -> Result<(), Problem> {
    let denied = || Problem::new(ProblemCode::Forbidden, 403, "message_effect_revoked");
    let authority = message.lark_effect.as_ref().ok_or_else(denied)?;
    if message.validate().is_err()
        || authority.issued_at > super::now()
        || revision(channel, message).as_ref() != Some(&authority.authority_sha256)
        || source(channel, &message.route_id).is_none_or(|(source, bot, binding)| {
            bot != authority.bot_id || binding != &authority.binding || !matches!(source, Source::NyxidRelay { platform, .. } if *platform == authority.platform)
        })
    {
        return Err(denied());
    }
    super::super::state::owner(caller, channel, message.consumer.as_ref(), "messageEffect")?;
    if authority.expires_at <= super::now() {
        return Err(Problem::new(
            ProblemCode::EventRefExpired,
            410,
            "message_effect_expired",
        ));
    }
    Ok(())
}

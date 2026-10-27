//! Authority from persisted separate-app receipts or the exact retained routed event.
use super::receipts::hash;
use crate::{
    auth::AgentCaller, channels::resolver::ResolvedChannel, native_lark::receipts::Receipt,
};
use cmaeg_core::{
    activity::{Activity, ActivityKind, ActorKind, SourceKind},
    ids::MessageRef,
    problem::{Problem, ProblemCode},
    seal::{
        native_lark_effect::{NativeLarkEffectAuthority, NativeLarkEffectTarget, MAX_AGE},
        EventRefPayload, MasterKeyRing, MessageRefPayload, ReplyAddress,
    },
};

fn revision(channel: &ResolvedChannel, message: &MessageRefPayload) -> Option<String> {
    let a = message.native_lark_effect.as_ref()?;
    crate::native_lark::credentials(
        &channel.definition,
        &ReplyAddress::NativeLark(a.reply.clone()),
    )?;
    let consumer = channel
        .consumer(message.consumer.as_ref())
        .filter(|c| !c.pending && c.credential.is_some())?;
    Some(hash(&serde_json::json!({
        "domain":"cmaeg.native-lark-effect-custody.v1",
        "channel":channel.id,"record":channel.pointer.record_id,"owner":channel.definition.owner,
        "consumer":message.consumer,"provider":consumer.provider,
        "agent_key":channel.consumer_key_hash(message.consumer.as_ref())?,
        "consumer_credential":crate::channels::definition::hash_secret(consumer.credential?.as_str()),
        "routing":channel.definition.routing,"reply":a.reply,"target":a.target,
        "event":message.event_id,"original":a.original_sha256,
        "observed_at":a.observed_at,"expires_at":a.expires_at,
    })))
}
fn seal(
    channel: &ResolvedChannel,
    mut message: MessageRefPayload,
    keys: &MasterKeyRing,
) -> Option<MessageRef> {
    let digest = revision(channel, &message)?;
    message.native_lark_effect.as_mut()?.authority_sha256 = digest;
    message.validate().ok()?;
    MessageRef::seal(&message, keys).ok()
}
fn message(
    a: NativeLarkEffectAuthority,
    channel_id: cmaeg_core::ids::ChannelId,
    event_id: cmaeg_core::ids::EventId,
    consumer: Option<cmaeg_core::ids::ConsumerId>,
) -> MessageRefPayload {
    MessageRefPayload {
        route_id: a.route(),
        platform_message_id: a.reply.message_id.clone(),
        received_reaction: None,
        received_reaction: None,
        native_lark_effect: Some(Box::new(a)),
        channel_id,
        event_id,
        consumer,
        whatsapp_reaction: None,
        discord_message: None,
        telegram_effect: None,
        lark_effect: None,
        relay_message: None,
        interaction_record_id: None,
    }
}
/// The caller must persist this exact receipt successfully before calling this function.
pub(crate) fn confirmed(
    channel: &ResolvedChannel,
    receipt: &Receipt,
    editable: bool,
    keys: &MasterKeyRing,
) -> Option<MessageRef> {
    let consumer = if receipt.consumer.as_str() == "default" {
        None
    } else {
        Some(receipt.consumer.clone())
    };
    let selected = channel.consumer(consumer.as_ref())?;
    if receipt.original.channel_id != channel.id
        || receipt.original.record_id != channel.pointer.record_id
        || receipt.original.owner != channel.definition.owner.subject
        || selected.provider != &receipt.provider
    {
        return None;
    }
    let a = NativeLarkEffectAuthority {
        reply: receipt.reply.clone(),
        record_id: receipt.original.record_id.clone(),
        target: NativeLarkEffectTarget::Confirmed { editable },
        original_sha256: hash(receipt),
        authority_sha256: String::new(),
        observed_at: receipt.confirmed_at,
        expires_at: receipt
            .confirmed_at
            .saturating_add(MAX_AGE)
            .min(receipt.retained_until),
    };
    seal(
        channel,
        message(
            a,
            channel.id.clone(),
            receipt.original.event_id.clone(),
            consumer,
        ),
        keys,
    )
}
/// Called only after `current_received_message` proves the selected retained admission.
pub(super) fn received(
    channel: &ResolvedChannel,
    event: &EventRefPayload,
    activity: &Activity,
    keys: &MasterKeyRing,
) -> Option<MessageRef> {
    let (ReplyAddress::NativeLark(reply), SourceKind::NativeLark(identity)) =
        (&event.reply, &activity.source)
    else {
        return None;
    };
    let now = super::now();
    let received_at = time::OffsetDateTime::parse(
        &activity.occurred_at,
        &time::format_description::well_known::Rfc3339,
    )
    .ok()?
    .unix_timestamp();
    let expires_at = event.expires_at.min(received_at.saturating_add(MAX_AGE));
    if event.lineage.is_some()
        || event.validate().is_err()
        || !event.verifies_activity(activity)
        || event.channel_id != channel.id
        || event.record_id != channel.pointer.record_id
        || identity != &reply.identity
        || activity.actor.kind != ActorKind::Human
        || !matches!(activity.kind, ActivityKind::Message { album_id: None, .. })
        || activity.conversation.id != reply.chat_id
        || activity.conversation.thread_id != reply.thread_id
        || crate::native_lark::message::original_id(identity, &reply.message_id)
            .ok()
            .as_ref()
            != Some(&event.event_id)
        || received_at <= 0
        || received_at > now
        || expires_at <= now
    {
        return None;
    }
    let a = NativeLarkEffectAuthority {
        reply: reply.clone(),
        record_id: event.record_id.clone(),
        target: NativeLarkEffectTarget::Received {
            sender_id: activity.actor.id.clone(),
        },
        original_sha256: hash(event),
        authority_sha256: String::new(),
        observed_at: received_at,
        expires_at,
    };
    seal(
        channel,
        message(
            a,
            event.channel_id.clone(),
            event.event_id.clone(),
            event.consumer.clone(),
        ),
        keys,
    )
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
            "native_lark_message_authority_revoked",
        )
    };
    let a = message.native_lark_effect.as_ref().ok_or_else(denied)?;
    if message.validate().is_err()
        || a.observed_at > super::now()
        || message.channel_id != channel.id
        || a.record_id != channel.pointer.record_id
        || revision(channel, message).as_ref() != Some(&a.authority_sha256)
    {
        return Err(denied());
    }
    super::super::state::owner(
        caller,
        channel,
        message.consumer.as_ref(),
        "nativeLarkMessageEffect",
    )?;
    if a.expires_at <= super::now() {
        return Err(Problem::new(
            ProblemCode::EventRefExpired,
            410,
            "native_lark_message_authority_expired",
        ));
    }
    Ok(())
}
pub(super) async fn retained(
    state: &super::ToolsState,
    message: &MessageRefPayload,
) -> Result<(), Problem> {
    let a = message
        .native_lark_effect
        .as_ref()
        .expect("checked authority");
    if a.confirmed() {
        let receipt = crate::native_lark::receipts::Store::new(
            state.resolver.record_client(),
            state.keys.clone(),
        )
        .load(
            &message.channel_id,
            &SourceKind::NativeLark(a.reply.identity.clone()),
            &message.platform_message_id,
            super::now(),
        )
        .await
        .ok()
        .flatten();
        if receipt
            .as_ref()
            .is_none_or(|r| hash(r) != a.original_sha256)
        {
            return Err(Problem::new(
                ProblemCode::Forbidden,
                403,
                "native_lark_confirmed_receipt_unavailable",
            ));
        }
    }
    Ok(())
}

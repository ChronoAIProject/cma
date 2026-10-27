//! Preserve incoming-message custody separately from outgoing bot-message effects.
use crate::{auth::AgentCaller, channels::resolver::ResolvedChannel};
use cmaeg_core::{
    activity::{Activity, ActivityKind, ConversationKind, SourceKind},
    channel::Source,
    ids::MessageRef,
    problem::{Problem, ProblemCode},
    seal::{
        whatsapp_reaction::{WhatsappReactionAuthority, MAX_REACTION_AGE},
        EventRefPayload, MasterKeyRing, MessageRefPayload, ReplyAddress,
    },
};
use sha2::{Digest, Sha256};

fn source<'a>(
    channel: &'a ResolvedChannel,
    authority: &WhatsappReactionAuthority,
) -> Option<&'a Source> {
    channel.definition.sources.iter().find(|source| matches!(source, Source::Whatsapp { app_id, waba_id, phone_number_id, .. }
        if *app_id == authority.app_id && *waba_id == authority.waba_id && *phone_number_id == authority.phone_number_id))
}
fn revision(channel: &ResolvedChannel, message: &MessageRefPayload) -> Option<String> {
    let authority = message.whatsapp_reaction.as_ref()?;
    let source = source(channel, authority)?;
    let consumer = channel
        .consumer(message.consumer.as_ref())
        .filter(|consumer| !consumer.pending && consumer.credential.is_some())?;
    let credentials = channel.definition.secrets.whatsapp.as_ref()?;
    credentials.validate().ok()?;
    Some(super::receipts::hash(&serde_json::json!({
        "source":source,"channel":channel.id,"record":channel.pointer.record_id,
        "owner":channel.definition.owner,"consumer":message.consumer,"provider":consumer.provider,
        "agent_key":channel.consumer_key_hash(message.consumer.as_ref())?,"routing":channel.definition.routing,
        "consumer_credential":format!("{:x}",Sha256::digest(consumer.credential?.as_str().as_bytes())),
        "access_token":format!("{:x}",Sha256::digest(credentials.access_token.as_str().as_bytes())),
        "app_secret":format!("{:x}",Sha256::digest(credentials.app_secret.as_str().as_bytes())),
        "verify_token":format!("{:x}",Sha256::digest(credentials.verify_token.as_str().as_bytes())),
    })))
}

pub(crate) fn issue(
    channel: &ResolvedChannel,
    event: &EventRefPayload,
    activity: &Activity,
    keys: &MasterKeyRing,
) -> Option<MessageRef> {
    let (
        ReplyAddress::Whatsapp {
            app_id,
            waba_id,
            phone_number_id,
            recipient_id,
            message_id,
        },
        SourceKind::Whatsapp {
            app_id: observed_app,
            waba_id: observed_waba,
            phone_number_id: observed_phone,
        },
    ) = (&event.reply, &activity.source)
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
    let expires_at = event
        .expires_at
        .min(received_at.saturating_add(MAX_REACTION_AGE));
    if event.lineage.is_some()
        || !event.verifies_activity(activity)
        || event.channel_id != channel.id
        || event.record_id != channel.pointer.record_id
        || (app_id, waba_id, phone_number_id) != (observed_app, observed_waba, observed_phone)
        || !matches!(activity.kind, ActivityKind::Message { .. })
        || activity.actor.id != *recipient_id
        || activity.conversation.id != *recipient_id
        || activity.conversation.kind != ConversationKind::Private
        || activity.conversation.thread_id.is_some()
        || activity.thread.is_some()
        || received_at <= 0
        || received_at > now
        || expires_at <= now
    {
        return None;
    }
    let authority = WhatsappReactionAuthority {
        app_id: app_id.clone(),
        waba_id: waba_id.clone(),
        phone_number_id: phone_number_id.clone(),
        recipient_id: recipient_id.clone(),
        authority_sha256: String::new(),
        received_at,
        expires_at,
    };
    let mut message = MessageRefPayload {
        route_id: authority.route(),
        native_lark_effect: None,
        whatsapp_reaction: Some(Box::new(authority)),
        channel_id: event.channel_id.clone(),
        event_id: event.event_id.clone(),
        platform_message_id: message_id.clone(),
        consumer: event.consumer.clone(),
        discord_message: None,
        telegram_effect: None,
        lark_effect: None,
        interaction_record_id: None,
        relay_message: None,
    };
    let stamp = revision(channel, &message)?;
    message.whatsapp_reaction.as_mut()?.authority_sha256 = stamp;
    message.validate().ok()?;
    MessageRef::seal(&message, keys).ok()
}

pub(crate) fn check(
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
    let authority = message.whatsapp_reaction.as_ref().ok_or_else(denied)?;
    if message.validate().is_err()
        || authority.received_at > super::now()
        || revision(channel, message).as_ref() != Some(&authority.authority_sha256)
    {
        return Err(denied());
    }
    super::super::state::owner(
        caller,
        channel,
        message.consumer.as_ref(),
        "setMessageReaction",
    )?;
    if authority.expires_at <= super::now() {
        return Err(Problem::new(
            ProblemCode::EventRefExpired,
            410,
            "received_message_reaction_expired",
        ));
    }
    Ok(())
}

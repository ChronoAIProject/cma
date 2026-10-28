//! Native effect authority is issued only for a confirmed CMA outbound message.
use crate::{
    auth::AgentCaller,
    channels::resolver::ResolvedChannel,
    ingress::admission::{admit, AdmissionFacts},
};
use cmaeg_core::{
    activity::{ActorKind, ConversationKind},
    channel::{AdmissionPolicy, GroupAdmission, Source},
    ids::MessageRef,
    problem::{Problem, ProblemCode},
    profile::canonical_json,
    seal::{
        EventRefPayload, MasterKeyRing, MessageRefPayload, ReplyAddress, TelegramAdmissionProof,
        TelegramEffectAuthority, TELEGRAM_EFFECT_TTL,
    },
};
use sha2::{Digest, Sha256};

pub(crate) fn event_supported(
    channel: &ResolvedChannel,
    event: &EventRefPayload,
    activity: &cmaeg_core::activity::Activity,
) -> bool {
    let (
        Some(proof),
        ReplyAddress::Telegram {
            bot_id,
            chat_id,
            thread_id,
            ..
        },
    ) = (&event.telegram_admission, &event.reply)
    else {
        return false;
    };
    let Some(Source::Telegram { admission, .. }) = channel
        .definition
        .sources
        .iter()
        .find(|s| matches!(s, Source::Telegram { bot_id: current, .. } if current == bot_id))
    else {
        return false;
    };
    event.verifies_activity(activity)
        && proof.actor_id == activity.actor.id
        && proof.actor_kind == activity.actor.kind
        && proof.conversation_kind == activity.conversation.kind
        && chat_id == &activity.conversation.id
        && thread_id == &activity.conversation.thread_id
        && matches!(&activity.source, cmaeg_core::activity::SourceKind::Telegram { bot_id: current, .. } if current == bot_id)
        && admission_check(admission, proof, bot_id, chat_id).is_ok()
}

pub(crate) fn enrich(
    reference: &MessageRef,
    event: &EventRefPayload,
    channel: &ResolvedChannel,
    keys: &MasterKeyRing,
    issued_at: i64,
    effects_only: bool,
) -> Option<MessageRef> {
    let proof = event.telegram_admission.as_ref()?;
    if event.validate().is_err() || event.lineage.is_some() {
        return None;
    }
    let ReplyAddress::Telegram {
        bot_id,
        chat_id,
        thread_id,
        ..
    } = &event.reply
    else {
        return None;
    };
    let mut message = reference.open(keys).ok()?.payload;
    if message.channel_id != event.channel_id
        || message.event_id != event.event_id
        || message.consumer != event.consumer
        || channel.id != event.channel_id
        || channel.pointer.record_id != event.record_id
    {
        return None;
    }
    let consumer = channel.consumer(event.consumer.as_ref())?;
    let authority = TelegramEffectAuthority {
        effects_only,
        record_id: event.record_id.clone(),
        owner: channel.definition.owner.clone(),
        provider: consumer.provider.clone(),
        agent_key_hash: *channel.consumer_key_hash(event.consumer.as_ref())?,
        routing_sha256: routing_hash(channel),
        bot_id: bot_id.clone(),
        chat_id: chat_id.clone(),
        thread_id: thread_id.clone(),
        admission: proof.clone(),
        issued_at,
        expires_at: issued_at.saturating_add(TELEGRAM_EFFECT_TTL),
    };
    message.telegram_effect = Some(authority);
    let caller = AgentCaller {
        key_hash: *channel.consumer_key_hash(event.consumer.as_ref())?,
    };
    check(&message, channel, caller, issued_at).ok()?;
    MessageRef::seal(&message, keys).ok()
}

pub(super) fn check(
    message: &MessageRefPayload,
    channel: &ResolvedChannel,
    caller: AgentCaller,
    now: i64,
) -> Result<(), Problem> {
    let authority = message.telegram_effect.as_ref().ok_or_else(denied)?;
    if message.validate().is_err() || authority.issued_at > now {
        return Err(denied());
    }
    if now >= authority.expires_at {
        return Err(Problem::new(
            ProblemCode::EventRefExpired,
            410,
            "message_effect_expired",
        ));
    }
    let consumer = channel
        .consumer(message.consumer.as_ref())
        .ok_or_else(denied)?;
    if message.channel_id != channel.id
        || authority.record_id != channel.pointer.record_id
        || authority.owner != channel.definition.owner
        || authority.provider != *consumer.provider
        || !caller.matches(&authority.agent_key_hash)
        || !routing_matches(channel, message, &authority.routing_sha256)
    {
        return Err(denied());
    }
    super::super::state::owner(caller, channel, message.consumer.as_ref(), "messageEffect")?;
    let Some(Source::Telegram { admission, .. }) = channel
        .definition
        .sources
        .iter()
        .find(|s| matches!(s, Source::Telegram { bot_id, .. } if bot_id == &authority.bot_id))
    else {
        return Err(denied());
    };
    if !channel
        .definition
        .secrets
        .telegram_bot_token
        .as_ref()
        .is_some_and(|token| {
            token
                .as_str()
                .split_once(':')
                .is_some_and(|(id, _)| id == authority.bot_id)
        })
    {
        return Err(denied());
    }
    admission_check(
        admission,
        &authority.admission,
        &authority.bot_id,
        &authority.chat_id,
    )
}

pub(crate) fn admission_check(
    policy: &AdmissionPolicy,
    proof: &TelegramAdmissionProof,
    bot: &str,
    chat: &str,
) -> Result<(), Problem> {
    let addressed = match policy {
        AdmissionPolicy::Scoped {
            groups: GroupAdmission::Mention,
            ..
        } => proof.mentioned,
        AdmissionPolicy::Scoped {
            groups: GroupAdmission::ReplyToBot,
            ..
        } => proof.replied_to_bot,
        _ => match (proof.mentioned, proof.replied_to_bot) {
            (Some(true), _) | (_, Some(true)) => Some(true),
            (Some(false), Some(false)) => Some(false),
            _ => None,
        },
    };
    admit(
        policy,
        &AdmissionFacts {
            receiving_bot_id: Some(bot),
            mentioned: proof.mentioned,
            sender_id: &proof.actor_id,
            chat_id: chat,
            is_group: proof.conversation_kind != ConversationKind::Private,
            is_bot_sender: proof.actor_kind == ActorKind::Bot,
            is_system: proof.actor_kind == ActorKind::System,
            addressed,
        },
    )
    .map_err(|_| denied())
}
pub(crate) fn routing_hash(channel: &ResolvedChannel) -> String {
    format!(
        "{:x}",
        Sha256::digest(
            canonical_json(
                serde_json::to_value(&channel.definition.routing).expect("routing serializes")
            )
            .as_bytes()
        )
    )
}
fn routing_matches(channel: &ResolvedChannel, message: &MessageRefPayload, original: &str) -> bool {
    original == routing_hash(channel)
        || (message.telegram_effect.as_ref().is_some_and(|authority| {
            message.route_id == crate::telegram::route(&authority.bot_id, &authority.chat_id)
        }) && crate::telegram::messaging_compat::accepts_unrouted_default(
            &channel.definition,
            message.consumer.as_ref(),
        ) && original == format!("{:x}", Sha256::digest(b"null")))
}
fn denied() -> Problem {
    Problem::new(ProblemCode::Forbidden, 403, "message_effect_revoked")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::message_effects::received_fixture::{Fixture, Native};

    #[tokio::test]
    async fn legacy_routing_fallback_requires_exact_telegram_authority_and_destination() {
        let fixture = Fixture::new(Native::Telegram, false, false).await;
        let channel = fixture
            .app
            .state
            .resolver
            .resolve_current(fixture.app.event.channel_id.as_str())
            .await
            .unwrap();
        let mut legacy = (*channel).clone();
        legacy.definition.routing = None;
        let mut base = fixture
            .reference()
            .await
            .open(&fixture.app.state.keys)
            .unwrap()
            .payload;
        base.received_reaction = None;
        let base = MessageRef::seal(&base, &fixture.app.state.keys).unwrap();
        let message = enrich(
            &base,
            &fixture.app.event,
            &legacy,
            &fixture.app.state.keys,
            crate::tools::message_effects::now(),
            false,
        )
        .unwrap()
        .open(&fixture.app.state.keys)
        .unwrap()
        .payload;
        let original = routing_hash(&legacy);
        assert_ne!(original, routing_hash(&channel));
        assert!(routing_matches(&channel, &message, &original));

        // Even a retained Telegram authority cannot transfer its compatibility
        // exception to another platform or a different Telegram destination.
        for route in [
            "discord:77:42",
            "native-lark:lark:app:42",
            "whatsapp:app:waba:77:42",
            "telegram:77:43",
        ] {
            let mut other = message.clone();
            other.route_id = route.into();
            assert!(!routing_matches(&channel, &other, &original), "{route}");
            other.telegram_effect = None;
            assert!(!routing_matches(&channel, &other, &original), "{route}");
        }
        let mut absent = message;
        absent.telegram_effect = None;
        assert!(!routing_matches(&channel, &absent, &original));
        assert_eq!(fixture.writes(), 0);
    }
}

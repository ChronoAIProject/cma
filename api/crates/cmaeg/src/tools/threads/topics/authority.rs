//! Authority comes from one immutable admitted native Telegram message and its original target.
use super::*;
use crate::ingress::routing_store::{EventIdentity, PlanStore};
use cmaeg_core::activity::{ActivityKind, ActorKind, ConversationKind, SourceKind};
use cmaeg_core::channel::{Platform, Secret};

pub(super) struct Authority {
    pub event: EventRefPayload,
    pub bot: String,
    pub chat: String,
    pub topic: crate::telegram::topics::Scope,
    pub custody: String,
}

pub(super) async fn load(
    state: &ToolsState,
    caller: AgentCaller,
    event: &EventRefPayload,
    channel: &ResolvedChannel,
) -> Result<(Arc<ResolvedChannel>, Authority), Problem> {
    let ReplyAddress::Telegram {
        bot_id,
        chat_id,
        message_id,
        thread_id,
    } = &event.reply
    else {
        return Err(unsupported());
    };
    if event.validate().is_err()
        || event.lineage.is_some()
        || event.expires_at <= now()
        || event.channel_id != channel.id
        || event.record_id != channel.pointer.record_id
        || !crate::telegram::topics::valid_chat(chat_id)
        || thread_id
            .as_deref()
            .is_some_and(|topic| !crate::telegram::topics::valid_topic(topic))
    {
        return Err(unsupported());
    }
    let identity = EventIdentity {
        owner: channel.definition.owner.subject.clone(),
        channel_id: channel.id.clone(),
        record_id: event.record_id.clone(),
        event_id: event.event_id.clone(),
    };
    let plan = PlanStore::new(state.resolver.record_client(), state.keys.clone())
        .load(&identity)
        .await
        .map_err(|_| unavailable())?
        .ok_or_else(unsupported)?;
    let current = crate::turns::routed::current_received_message(
        state.resolver.record_client(),
        state.keys.clone(),
        &state.resolver,
        caller,
        event,
        &plan.activity,
    )
    .await
    .map_err(|_| denied())?;
    let source = current
        .definition
        .source_of(&plan.activity.source)
        .ok_or_else(denied)?;
    if !matches!(source, Source::Telegram { bot_id: configured, topic_lifecycle: true, .. } if configured == bot_id)
        || plan.source != *source
        || plan.identity != identity
        || !event.verifies_activity(&plan.activity)
        || event.expires_at > plan.retained_until
        || !matches!(plan.activity.kind, ActivityKind::Message { .. })
        || plan.activity.actor.kind != ActorKind::Human
        || plan.activity.actor.id == *bot_id
        || plan.activity.source
            != (SourceKind::Telegram {
                platform: Platform::Telegram,
                bot_id: bot_id.clone(),
            })
        || plan.activity.conversation.kind != ConversationKind::Group
        || plan.activity.conversation.id != *chat_id
        || plan.activity.conversation.thread_id != *thread_id
    {
        return Err(denied());
    }
    // The native webhook's exact verified bytes, never relay-normalized facts or caller JSON.
    let raw: serde_json::Value =
        serde_json::from_slice(&plan.source_body).map_err(|_| unsupported())?;
    let message = raw.get("message").ok_or_else(unsupported)?;
    if raw.get("callback_query").is_some()
        || raw.get("edited_message").is_some()
        || raw.get("channel_post").is_some()
        || raw.get("edited_channel_post").is_some()
        || message.get("sender_chat").is_some()
        || message.get("business_connection_id").is_some()
        || message.get("guest_query_id").is_some()
        || message["chat"]["type"] != "supergroup"
        || numeric(&message["chat"]["id"]).as_deref() != Some(chat_id)
        || numeric(&message["message_id"]).as_deref() != Some(message_id)
        || numeric(&message["from"]["id"]).as_deref() != Some(&plan.activity.actor.id)
        || message["from"]["is_bot"] != false
    {
        return Err(unsupported());
    }
    let topic = match thread_id {
        Some(topic)
            if message["is_topic_message"] == true
                && numeric(&message["message_thread_id"]).as_deref() == Some(topic) =>
        {
            crate::telegram::topics::Scope::Numbered(topic.clone())
        }
        None if state.topic_client()?.general_enabled
            && message["chat"]["is_forum"] == true
            && message.get("message_thread_id").is_none()
            && message.get("is_topic_message").is_none()
            && message.get("direct_messages_topic").is_none()
            && message["chat"]["is_direct_messages"] != true =>
        {
            // Telegram emits positive forum chat metadata, but omits both topic
            // markers for General. Keep the admitted event/reply thread unset.
            crate::telegram::topics::Scope::General
        }
        _ => return Err(unsupported()),
    };
    let consumer = current
        .consumer(event.consumer.as_ref())
        .ok_or_else(denied)?;
    let credential = consumer.credential.ok_or_else(denied)?;
    let bot_token = token(&current, bot_id).ok_or_else(denied)?;
    let custody = hash(&serde_json::json!({
        "domain":"cmaeg.telegram-topic-custody.v1", "channel":current.id,"record":event.record_id,
        "owner":current.definition.owner,"source":source,"event":event,"actor":plan.activity.actor,
        "bot":bot_id,"chat":chat_id,"topic":topic.id(),"consumer":event.consumer,
        "provider":consumer.provider,"agent_key":consumer.agent_key_hash,
        "credential":crate::channels::definition::hash_secret(credential.as_str()),
        "bot_token":crate::channels::definition::hash_secret(bot_token.as_str()),
        "webhook":current.definition.secrets.telegram_webhook_secret.as_ref().map(|v| crate::channels::definition::hash_secret(v.as_str()))
    }));
    Ok((
        current,
        Authority {
            event: event.clone(),
            bot: bot_id.clone(),
            chat: chat_id.clone(),
            topic,
            custody,
        },
    ))
}
fn numeric(value: &serde_json::Value) -> Option<String> {
    value.as_i64().map(|n| n.to_string())
}
pub(super) fn token<'a>(channel: &'a ResolvedChannel, bot: &str) -> Option<&'a Secret> {
    channel
        .definition
        .secrets
        .telegram_bot_token
        .as_ref()
        .filter(|t| crate::telegram::media_token_matches(t, bot))
}
pub(super) fn denied() -> Problem {
    Problem::new(ProblemCode::Forbidden, 403, "topic_authority_revoked")
}
pub(super) fn unsupported() -> Problem {
    Problem::new(ProblemCode::InvalidRequest, 400, "topic_scope_unsupported")
}

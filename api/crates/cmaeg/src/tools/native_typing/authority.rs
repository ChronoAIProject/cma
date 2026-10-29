//! Original retained native message and selected consumer; no caller-selected target.
use super::*;
use crate::{
    channels::definition::hash_secret,
    ingress::routing_store::{EventIdentity, PlanStore},
};
use cmaeg_core::activity::{ActivityKind, ConversationKind, SourceKind};
use std::sync::Arc;

pub(super) struct Authorized {
    pub channel: Arc<ResolvedChannel>,
    pub activity: Activity,
    pub custody: String,
}

pub(super) async fn load(
    state: &ToolsState,
    caller: AgentCaller,
    channel: &ResolvedChannel,
    event: &EventRefPayload,
    expected: Option<&Activity>,
) -> Result<Authorized, Problem> {
    if event.lineage.is_some() || event.activity_sha256.is_none() {
        return Err(denied());
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
        .ok_or_else(denied)?;
    if expected.is_some_and(|activity| activity != &plan.activity)
        || !matches!(plan.activity.kind, ActivityKind::Message { .. })
        || !event.verifies_activity(&plan.activity)
    {
        return Err(denied());
    }
    let current = crate::turns::routed::current_received_message(
        state.resolver.record_client(),
        state.keys.clone(),
        &state.resolver,
        caller,
        event,
        &plan.activity,
    )
    .await
    .map_err(super::super::routed_reply_error)?;
    let source = current
        .definition
        .source_of(&plan.activity.source)
        .ok_or_else(denied)?;
    let credential = match (&event.reply, &plan.activity.source) {
        (
            ReplyAddress::Whatsapp {
                app_id,
                waba_id,
                phone_number_id,
                recipient_id,
                message_id,
            },
            SourceKind::Whatsapp {
                app_id: app,
                waba_id: account,
                phone_number_id: phone,
            },
        ) => {
            if (app_id, waba_id, phone_number_id) != (app, account, phone)
                || plan.activity.conversation.kind != ConversationKind::Private
                || plan.activity.conversation.id != *recipient_id
                || plan.activity.actor.id != *recipient_id
                || plan.activity.conversation.thread_id.is_some()
                || plan.activity.thread.is_some()
                || !crate::whatsapp::webhook::extract(source, &plan.source_body).is_ok_and(
                    |messages| {
                        messages.iter().any(|message| {
                            message.id == *message_id
                                && message.sender == *recipient_id
                                && message.group_id.is_none()
                                && message.timestamp <= super::now()
                                && message.timestamp > super::now().saturating_sub(86_400)
                                && time::OffsetDateTime::parse(
                                    &plan.activity.occurred_at,
                                    &time::format_description::well_known::Rfc3339,
                                )
                                .is_ok_and(|at| at.unix_timestamp() == message.timestamp)
                                && matches!(
                                    message.content,
                                    crate::whatsapp::webhook::Content::Text { .. }
                                        | crate::whatsapp::webhook::Content::Media { .. }
                                )
                        })
                    },
                )
            {
                return Err(denied());
            }
            let credentials = crate::whatsapp::credentials(&current.definition, &event.reply)
                .filter(|credentials| credentials.validate().is_ok())
                .ok_or_else(denied)?;
            serde_json::json!({
                "access": hash_secret(credentials.access_token.as_str()),
                "app": hash_secret(credentials.app_secret.as_str()),
                "verify": hash_secret(credentials.verify_token.as_str()),
            })
        }
        (
            ReplyAddress::Discord {
                bot_id,
                channel_id,
                message_id,
                thread_id,
            },
            SourceKind::Discord { bot_id: bot },
        ) => {
            let raw: serde_json::Value =
                serde_json::from_slice(&plan.source_body).map_err(|_| denied())?;
            let dispatch = &raw["dispatch"];
            if bot_id != bot
                || plan.activity.actor.id == *bot_id
                || !matches!(
                    plan.activity.actor.kind,
                    cmaeg_core::activity::ActorKind::Human
                )
                || ["bot", "system"].iter().any(|flag| {
                    dispatch["author"]
                        .get(*flag)
                        .is_some_and(|value| value.as_bool() != Some(false))
                })
                || dispatch
                    .get("webhook_id")
                    .is_some_and(|value| !value.is_null())
                || plan.activity.conversation.id != *channel_id
                || plan.activity.conversation.thread_id != *thread_id
                || thread_id
                    .as_ref()
                    .is_some_and(|thread| thread != channel_id)
                || dispatch["id"].as_str() != Some(message_id)
                || dispatch["channel_id"].as_str() != Some(channel_id)
                || dispatch["author"]["id"].as_str() != Some(&plan.activity.actor.id)
                || !matches!(dispatch["type"].as_u64(), Some(0 | 19))
            {
                return Err(denied());
            }
            let token = crate::discord::rest::token(&current, bot_id).ok_or_else(denied)?;
            serde_json::json!({"bot": hash_secret(token.as_str())})
        }
        _ => return Err(denied()),
    };
    let consumer = current
        .consumer(event.consumer.as_ref())
        .ok_or_else(denied)?;
    let consumer_credential = consumer.credential.ok_or_else(denied)?;
    let custody = hash_secret(&cmaeg_core::profile::canonical_json(serde_json::json!({
        "domain":"cmaeg.native-typing.v1", "channel": current.id, "record": event.record_id,
        "owner": current.definition.owner, "source": source, "event": event,
        "consumer": event.consumer, "provider": consumer.provider,
        "agent_key": consumer.agent_key_hash, "credential": hash_secret(consumer_credential.as_str()),
        "native_credentials": credential,
    })));
    Ok(Authorized {
        channel: current,
        activity: plan.activity,
        custody,
    })
}

pub(super) fn denied() -> Problem {
    Problem::new(
        ProblemCode::Forbidden,
        403,
        "native_typing_authority_unavailable",
    )
}

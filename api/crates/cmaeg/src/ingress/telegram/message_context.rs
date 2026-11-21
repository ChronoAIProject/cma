//! A small encrypted locator; the committed routing manifest owns all authority.
use super::events::Error;
use crate::{
    channels::{definition::hash_secret, resolver::ResolvedChannel},
    ingress::{
        launch::LaunchRequest,
        routing_store::{AdmittedPlan, EventIdentity, PlanStore},
        IngressState,
    },
    records::RecordError,
};
use cmaeg_core::{
    activity::{ActivityKind, SourceKind},
    ids::RecordId,
    seal::{open_interaction, seal_interaction, ReplyAddress},
};
use serde::{Deserialize, Serialize};

const TTL: i64 = 7 * 86400;
#[derive(PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Context {
    kind: String,
    original: EventIdentity,
    bot: String,
    chat: String,
    message: String,
    revision: String,
    expires_at: i64,
}
fn revision(channel: &ResolvedChannel, bot: &str) -> Result<String, Error> {
    let source = channel
        .definition
        .source_of(&SourceKind::Telegram {
            platform: cmaeg_core::channel::Platform::Telegram,
            bot_id: bot.into(),
        })
        .ok_or(Error::Invalid)?;
    let secrets = &channel.definition.secrets;
    let token = secrets.telegram_bot_token.as_ref().ok_or(Error::Invalid)?;
    let secret = secrets
        .telegram_webhook_secret
        .as_ref()
        .ok_or(Error::Invalid)?;
    Ok(hash_secret(&cmaeg_core::profile::canonical_json(
        serde_json::json!({
            "source":crate::telegram::messaging_source(source),"owner":channel.definition.owner.subject,
            "token":hash_secret(token.as_str()),"webhook":hash_secret(secret.as_str())
        }),
    )))
}
fn id(channel: &ResolvedChannel, bot: &str, chat: &str, message: &str) -> String {
    crate::connections::store::index_id(
        "telegram-message-context-v1",
        &[channel.definition.record_id.as_str(), bot, chat, message],
    )
}
impl Context {
    pub(crate) fn capture(state: &IngressState, request: &LaunchRequest, now: i64) -> Option<Self> {
        if !matches!(
            request
                .channel
                .definition
                .source_of(&request.activity.source),
            Some(cmaeg_core::channel::Source::Telegram {
                message_events: true,
                ..
            })
        ) || !state
            .routed
            .as_ref()
            .is_some_and(|routing| routing.telegram_selected(request))
            || request.channel.definition.routing.is_none()
            || !matches!(request.activity.kind, ActivityKind::Message { .. })
        {
            return None;
        }
        let event = request
            .event_ref
            .open(&state.master_keys, now)
            .ok()?
            .payload;
        let ReplyAddress::Telegram {
            bot_id,
            chat_id,
            message_id,
            ..
        } = event.reply
        else {
            return None;
        };
        let created = time::OffsetDateTime::parse(
            &request.activity.occurred_at,
            &time::format_description::well_known::Rfc3339,
        )
        .ok()?
        .unix_timestamp();
        let expires_at = created.checked_add(TTL)?;
        if created > now || expires_at <= now {
            return None;
        }
        Some(Self {
            kind: "telegram_message_context_v1".into(),
            original: EventIdentity {
                owner: request.channel.definition.owner.subject.clone(),
                channel_id: request.channel.id.clone(),
                record_id: request.channel.definition.record_id.clone(),
                event_id: request.activity.event_id.clone(),
            },
            revision: revision(&request.channel, &bot_id).ok()?,
            bot: bot_id,
            chat: chat_id,
            message: message_id,
            expires_at,
        })
    }
    /// Opted-in webhooks acknowledge only a confirmed locator. Retrying the
    /// original update reuses its committed admission before repairing this row.
    pub(crate) async fn remember(
        self,
        state: &IngressState,
        channel: &ResolvedChannel,
    ) -> Result<(), Error> {
        let work = async {
            let id = id(channel, &self.bot, &self.chat, &self.message);
            let bytes = serde_json::to_vec(&self).map_err(|_| Error::Unavailable)?;
            let blob = seal_interaction(&state.master_keys, &id, &bytes)
                .map_err(|_| Error::Unavailable)?;
            let records = state.resolver.record_client().interactions();
            match records
                .put_expiring(
                    &RecordId::new(&id).map_err(|_| Error::Unavailable)?,
                    &self.original.owner,
                    &blob,
                    None,
                    Some(self.expires_at),
                )
                .await
            {
                Ok(1) => Ok(()),
                Err(RecordError::Conflict { .. }) => {
                    let row = records
                        .get(
                            records
                                .record_url(&id)
                                .map_err(|_| Error::Unavailable)?
                                .as_str(),
                        )
                        .await
                        .map_err(|_| Error::Unavailable)?;
                    let opened = open_interaction(&state.master_keys, &id, &row.blob)
                        .map_err(|_| Error::Unavailable)?;
                    let saved: Self =
                        serde_json::from_slice(&opened.payload).map_err(|_| Error::Unavailable)?;
                    if row.version != 1 || saved != self {
                        return Err(Error::Unavailable);
                    }
                    Ok(())
                }
                _ => Err(Error::Unavailable),
            }
        };
        tokio::time::timeout(std::time::Duration::from_millis(250), work)
            .await
            .map_err(|_| Error::Unavailable)?
    }
}
pub(super) async fn original(
    state: &IngressState,
    channel: &ResolvedChannel,
    bot: &str,
    chat: &str,
    message: &str,
    now: i64,
) -> Result<AdmittedPlan, Error> {
    let records = state.resolver.record_client().interactions();
    let id = id(channel, bot, chat, message);
    let row = records
        .get(
            records
                .record_url(&id)
                .map_err(|_| Error::Unavailable)?
                .as_str(),
        )
        .await
        .map_err(|error| match error {
            RecordError::NotFound => Error::Context,
            _ => Error::Unavailable,
        })?;
    let bytes = open_interaction(&state.master_keys, &id, &row.blob).map_err(|_| Error::Invalid)?;
    let saved: Context = serde_json::from_slice(&bytes.payload).map_err(|_| Error::Invalid)?;
    if row.version != 1
        || saved.kind != "telegram_message_context_v1"
        || saved.bot != bot
        || saved.chat != chat
        || saved.message != message
        || saved.expires_at <= now
        || saved.original.owner != channel.definition.owner.subject
        || saved.original.channel_id != channel.id
        || saved.original.record_id != channel.definition.record_id
        || saved.revision != revision(channel, bot)?
    {
        return Err(Error::Context);
    }
    let plan = PlanStore::new(state.resolver.record_client(), state.master_keys.clone())
        .load(&saved.original)
        .await
        .map_err(|_| Error::Unavailable)?
        .ok_or(Error::Unavailable)?;
    if plan.activity.conversation.id != chat || !matches!(plan.activity.kind, ActivityKind::Message { .. })
        || plan.activity.source != (SourceKind::Telegram { platform:cmaeg_core::channel::Platform::Telegram, bot_id:bot.into() })
        || !plan.targets.iter().all(|target| matches!(&target.launch.event.reply,
            ReplyAddress::Telegram { bot_id, chat_id, message_id, thread_id }
                if bot_id == bot && chat_id == chat && message_id == message && thread_id == &plan.activity.conversation.thread_id)) {
        return Err(Error::Invalid);
    }
    Ok(plan)
}

//! Explicit bounded reads of the original native Discord thread, never new work.
use super::{context, ContextResponse, ToolsState};
use crate::{
    auth::AgentCaller,
    channels::resolver::ResolvedChannel,
    discord::{rest::token, DiscordRest},
};
use cmaeg_core::{
    activity::{Activity, ActivityKind, SourceKind},
    channel::Source,
    problem::{Problem, ProblemCode},
    seal::{EventRef, EventRefPayload, ReplyAddress},
};
use serde::Serialize;
use std::time::Duration;
use tokio::sync::Semaphore;
use utoipa::ToSchema;

/// A current text-only window, with no pagination or transferable message authority.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ThreadHistory {
    pub thread_id: String,
    pub before_message_id: String,
    pub messages: Vec<ThreadHistoryMessage>,
    /// True when text was clipped to the 32 KiB total content budget.
    pub text_truncated: bool,
}
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ThreadHistoryMessage {
    pub message_id: String,
    pub author: cmaeg_core::activity::Actor,
    pub text: String,
    pub occurred_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edited_at: Option<String>,
    /// Historical attachments have no download or effect authority in this result.
    pub attachment_count: u8,
}

pub struct DiscordThreadHistory {
    api: DiscordRest,
    slots: Semaphore,
}
impl DiscordThreadHistory {
    pub fn new(api: DiscordRest) -> Self {
        Self {
            api,
            slots: Semaphore::new(8),
        }
    }
}

pub(super) fn eligible(
    channel: &ResolvedChannel,
    event: &EventRefPayload,
    activity: &Activity,
) -> bool {
    let (
        SourceKind::Discord { bot_id },
        ReplyAddress::Discord {
            bot_id: receiving,
            channel_id,
            thread_id: Some(thread),
            ..
        },
    ) = (&activity.source, &event.reply)
    else {
        return false;
    };
    event.lineage.is_none()
        && event.verifies_activity(activity)
        && matches!(activity.kind, ActivityKind::Message { .. })
        && bot_id == receiving
        && thread == channel_id
        && activity.conversation.id == *channel_id
        && activity.conversation.thread_id.as_ref() == Some(thread)
        && channel.definition.sources.iter().any(|source| matches!(source, Source::Discord { bot_id: configured, thread_history: true, .. } if configured == bot_id))
        && token(channel, bot_id).is_some()
}

fn stamp(
    channel: &ResolvedChannel,
    event: &EventRefPayload,
    activity: &Activity,
) -> Option<([u8; 32], String)> {
    if !eligible(channel, event, activity) {
        return None;
    }
    let SourceKind::Discord { bot_id } = &activity.source else {
        return None;
    };
    Some((
        crate::delivery::discord_edit::authority(channel, event.consumer.as_ref(), bot_id)?,
        crate::channels::definition::hash_secret(token(channel, bot_id)?.as_str()),
    ))
}
fn denied() -> Problem {
    Problem::new(ProblemCode::Forbidden, 403, "thread_history_unavailable")
}

impl ToolsState {
    /// Ordinary context reads retain their original latency and serialized shape.
    pub async fn context_with_history(
        &self,
        caller: AgentCaller,
        reference: &EventRef,
        include: bool,
    ) -> Result<ContextResponse, Problem> {
        if !include {
            return self.context(caller, reference).await;
        }
        let client = self.discord_thread_history.as_ref().ok_or_else(denied)?;
        let _permit = client
            .slots
            .try_acquire()
            .map_err(|_| Problem::new(ProblemCode::RateLimited, 429, "thread_history_busy"))?;
        tokio::time::timeout(
            Duration::from_secs(8),
            self.read_thread_history(caller, reference, client),
        )
        .await
        .map_err(|_| {
            Problem::new(
                ProblemCode::ProviderUnavailable,
                503,
                "thread_history_unavailable",
            )
        })?
    }

    async fn read_thread_history(
        &self,
        caller: AgentCaller,
        reference: &EventRef,
        client: &DiscordThreadHistory,
    ) -> Result<ContextResponse, Problem> {
        let (_, event, _) = self
            .authorize_event(caller, reference, "readEventContext")
            .await?;
        let channel = self
            .resolver
            .resolve_current(event.channel_id.as_str())
            .await
            .map_err(|error| error.problem())?;
        if event.record_id != channel.pointer.record_id {
            return Err(denied());
        }
        let matched = super::state::owner(
            caller,
            &channel,
            event.consumer.as_ref(),
            "readEventContext",
        )?;
        let consumer = context::consumer_view(&channel, &matched)?;
        let (activity, verified) =
            context::read_activity(self, &channel, &consumer, &event).await?;
        let authority = stamp(&channel, &event, &activity).ok_or_else(denied)?;
        let ReplyAddress::Discord {
            bot_id,
            channel_id,
            message_id,
            ..
        } = &event.reply
        else {
            return Err(denied());
        };
        let credential = token(&channel, bot_id).ok_or_else(denied)?;
        let history = client
            .api
            .thread_history(
                credential,
                bot_id,
                channel_id,
                message_id,
                &activity.conversation,
            )
            .await
            .map_err(problem)?;
        // A source, key or consumer revoked while the provider was being read
        // cannot turn a previously authorized request into a stale data response.
        let current = self
            .resolver
            .resolve_current(channel.id.as_str())
            .await
            .map_err(|error| error.problem())?;
        super::state::owner(
            caller,
            &current,
            event.consumer.as_ref(),
            "readEventContext",
        )?;
        if stamp(&current, &event, &activity).as_ref() != Some(&authority) {
            return Err(denied());
        }
        reference
            .open(&self.keys, time::OffsetDateTime::now_utc().unix_timestamp())
            .map_err(|_| Problem::new(ProblemCode::EventRefExpired, 410, "event_ref_expired"))?;
        let inbound = context::inbound_media(self, &consumer, &activity.source);
        let mut response = context::project(
            &channel, &consumer, &event, activity, verified, inbound, self,
        );
        response.thread_history = Some(history);
        Ok(response)
    }
}
fn problem(error: crate::discord::RestError) -> Problem {
    use crate::discord::RestError;
    match error {
        RestError::RateLimited(_) => {
            Problem::new(ProblemCode::RateLimited, 429, "thread_history_rate_limited")
        }
        RestError::Rejected | RestError::NotFound => denied(),
        _ => Problem::new(
            ProblemCode::ProviderUnavailable,
            503,
            "thread_history_unavailable",
        ),
    }
}

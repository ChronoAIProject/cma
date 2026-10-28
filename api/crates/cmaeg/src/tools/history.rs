//! One read, one original event, one deadline; no history-derived action authority.
use super::ToolsState;
use crate::{
    auth::AgentCaller,
    channels::resolver::ResolvedChannel,
    discord::{
        history_authority::{self, denied, unavailable, Authority},
        mutation::hash,
        rest::{history_projection, token},
        DiscordRest, RestError,
    },
};
use cmaeg_core::{
    channel::Source,
    conversation_history::{
        ConversationHistoryCapabilities, ConversationHistoryPage, HistoryContentCoverage,
        HistoryContinuation, HistoryDirection, HISTORY_METADATA_BYTES, HISTORY_OUTPUT_BYTES,
        HISTORY_PAGE_SIZE, HISTORY_TEXT_BYTES,
    },
    problem::{Problem, ProblemCode},
    seal::{
        EventRef, EventRefPayload, HistoryCursor, HistoryCursorPayload, OpenError, ReplyAddress,
    },
};
use std::{sync::Arc, time::Duration};
use tokio::{sync::Semaphore, time::Instant};

pub struct DiscordConversationHistory {
    api: DiscordRest,
    slots: Semaphore,
    budget: Duration,
}
#[derive(Clone, Copy)]
pub(super) struct HistoryReadOptions<'a> {
    pub cursor: Option<&'a HistoryCursor>,
    pub include_media: bool,
}
impl DiscordConversationHistory {
    pub fn new(api: DiscordRest) -> Self {
        Self {
            api,
            slots: Semaphore::new(8),
            budget: Duration::from_secs(8),
        }
    }
}

impl ToolsState {
    fn history_client(&self) -> Result<&DiscordConversationHistory, Problem> {
        self.discord_conversation_history.as_deref().ok_or_else(|| {
            Problem::new(ProblemCode::Forbidden, 403, "conversation_history_disabled")
        })
    }
    async fn history_authority(
        &self,
        caller: AgentCaller,
        event: &EventRefPayload,
    ) -> Result<(Arc<ResolvedChannel>, Authority), Problem> {
        self.history_client()?;
        let channel = self
            .resolver
            .resolve_current(event.channel_id.as_str())
            .await
            .map_err(|error| error.problem())?;
        super::state::owner(
            caller,
            &channel,
            event.consumer.as_ref(),
            "readEventHistory",
        )?;
        let authority = history_authority::load(
            self.resolver.record_client(),
            self.keys.clone(),
            &channel,
            event,
        )
        .await?;
        Ok((channel, authority))
    }
    pub(super) async fn history_capabilities(
        &self,
        caller: AgentCaller,
        event: &EventRefPayload,
        authorized: &ResolvedChannel,
    ) -> Option<ConversationHistoryCapabilities> {
        if matches!(event.reply, ReplyAddress::NativeLark(_)) {
            return self
                .native_history_capabilities(caller, event, authorized)
                .await;
        }
        // Ordinary context must never incur history I/O for unrelated platforms or sources.
        let ReplyAddress::Discord { bot_id, .. } = &event.reply else {
            return None;
        };
        if event.lineage.is_some() || event.expires_at <= now()
            || !authorized.definition.sources.iter().any(|source| matches!(source,
                Source::Discord { bot_id: configured, conversation_history: true, .. } if configured == bot_id))
        { return None; }
        let client = self.history_client().ok()?;
        let _permit = client.slots.try_acquire().ok()?;
        super::state::owner(
            caller,
            authorized,
            event.consumer.as_ref(),
            "readEventHistory",
        )
        .ok()?;
        // Discovery is an optional hint. Explicit reads retain their independent eight-second budget.
        tokio::time::timeout(
            Duration::from_millis(100),
            history_authority::load(
                self.resolver.record_client(),
                self.keys.clone(),
                authorized,
                event,
            ),
        )
        .await
        .ok()?
        .ok()?;
        if event.expires_at <= now() {
            return None;
        }
        Some(ConversationHistoryCapabilities {
            page_size: HISTORY_PAGE_SIZE,
            direction: HistoryDirection::Backward,
            attachment_facts_only: true,
            max_text_bytes: HISTORY_TEXT_BYTES as u32,
            max_metadata_bytes: HISTORY_METADATA_BYTES as u32,
            max_output_bytes: HISTORY_OUTPUT_BYTES as u32,
            expires_at: event.expires_at,
        })
    }
    pub async fn read_event_history(
        &self,
        caller: AgentCaller,
        reference: &EventRef,
        cursor: Option<&HistoryCursor>,
    ) -> Result<ConversationHistoryPage, Problem> {
        self.read_event_history_with_media(caller, reference, cursor, false)
            .await
    }
    /// Opted-in response shapes never flow to callers using the original method.
    pub async fn read_event_history_with_media(
        &self,
        caller: AgentCaller,
        reference: &EventRef,
        cursor: Option<&HistoryCursor>,
        include_media: bool,
    ) -> Result<ConversationHistoryPage, Problem> {
        let started = Instant::now();
        let event = reference
            .open(&self.keys, now())
            .map_err(reference_problem)?
            .payload;
        if matches!(event.reply, ReplyAddress::NativeLark(_)) {
            return self
                .read_native_lark_history(
                    caller,
                    reference,
                    HistoryReadOptions {
                        cursor,
                        include_media,
                    },
                    event,
                    started,
                )
                .await;
        }
        if include_media {
            return Err(Problem::new(
                ProblemCode::InvalidRequest,
                400,
                "include_media_requires_native_lark_history",
            ));
        }
        let client = self.history_client()?;
        let end = started + client.budget;
        let _permit = client.slots.try_acquire().map_err(|_| {
            Problem::new(ProblemCode::RateLimited, 429, "conversation_history_busy")
        })?;
        tokio::time::timeout_at(
            end,
            self.history_inner(caller, reference, cursor, client, end),
        )
        .await
        .map_err(|_| unavailable())?
    }
    async fn history_inner(
        &self,
        caller: AgentCaller,
        reference: &EventRef,
        cursor: Option<&HistoryCursor>,
        client: &DiscordConversationHistory,
        end: Instant,
    ) -> Result<ConversationHistoryPage, Problem> {
        let event = reference
            .open(&self.keys, now())
            .map_err(reference_problem)?
            .payload;
        let (channel, authority) = self.history_authority(caller, &event).await?;
        let event_sha256 = hash(&event);
        let before = if let Some(cursor) = cursor {
            let payload = cursor
                .open(&self.keys, now())
                .map_err(|_| invalid_cursor())?
                .payload;
            if payload.event_sha256 != event_sha256
                || payload.custody != authority.custody
                || payload.origin != authority.origin
                || payload.anchor_message_id != authority.message
                || payload.expires_at != event.expires_at
            {
                return Err(invalid_cursor());
            }
            payload.before_message_id
        } else {
            authority.message.clone()
        };
        let credential = token(&channel, &authority.bot).ok_or_else(denied)?;
        client
            .api
            .verify_bot(credential, &authority.bot)
            .await
            .map_err(read_problem)?;
        client
            .api
            .check_history_origin(credential, &authority)
            .await
            .map_err(read_problem)?;
        let rows = client
            .api
            .history_rows(credential, &authority.origin.channel_id, &before)
            .await
            .map_err(read_problem)?;
        let fetched_at = time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|_| unavailable())?;
        let projection = history_projection::project(&rows, &authority.origin.channel_id, &before)
            .map_err(read_problem)?;
        client
            .api
            .check_history_origin(credential, &authority)
            .await
            .map_err(read_problem)?;
        let (_, current) = self.history_authority(caller, &event).await?;
        if current.custody != authority.custody || current.origin != authority.origin {
            return Err(denied());
        }
        let issued_at = now();
        reference
            .open(&self.keys, issued_at)
            .map_err(reference_problem)?;
        let continuation = if let Some(next) = projection.next {
            HistoryContinuation::MoreVisibleRows {
                cursor: HistoryCursor::seal(
                    &HistoryCursorPayload {
                        v: 1,
                        event_sha256,
                        custody: authority.custody,
                        origin: authority.origin,
                        anchor_message_id: authority.message.clone(),
                        before_message_id: next,
                        page_size: HISTORY_PAGE_SIZE,
                        issued_at,
                        expires_at: event.expires_at,
                    },
                    &self.keys,
                )
                .map_err(|_| unavailable())?,
            }
        } else {
            HistoryContinuation::EndOfVisibleHistory
        };
        let page = ConversationHistoryPage {
            conversation: authority.activity.conversation,
            anchor_message_id: authority.message,
            before_message_id: before,
            fetched_at,
            expires_at: event.expires_at,
            messages: projection.messages,
            scanned_messages: projection.scanned,
            omitted_message_count: projection.omitted,
            text_truncated: projection.text_truncated,
            metadata_truncated: projection.metadata_truncated,
            content_coverage: HistoryContentCoverage::PlatformReturnedOnly,
            continuation,
        };
        let encoded = serde_json::to_vec(&page).map_err(|_| unavailable())?;
        if encoded.len() > HISTORY_OUTPUT_BYTES {
            return Err(unavailable());
        }
        // timeout_at cannot preempt synchronous projection/serialization; check their shared budget too.
        if Instant::now() >= end {
            return Err(unavailable());
        }
        reference
            .open(&self.keys, now())
            .map_err(reference_problem)?;
        Ok(page)
    }
}
fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}
fn invalid_cursor() -> Problem {
    Problem::new(
        ProblemCode::InvalidRequest,
        400,
        "history_cursor_invalid_or_expired",
    )
}
pub(super) fn reference_problem(error: OpenError) -> Problem {
    match error {
        OpenError::Expired => Problem::new(ProblemCode::EventRefExpired, 410, "event_ref_expired"),
        _ => Problem::new(ProblemCode::EventRefInvalid, 400, "event_ref_invalid"),
    }
}
fn read_problem(error: RestError) -> Problem {
    match error {
        RestError::RateLimited(_) => {
            Problem::new(ProblemCode::RateLimited, 429, "history_rate_limited")
        }
        RestError::Rejected | RestError::NotFound => denied(),
        _ => unavailable(),
    }
}

#[cfg(test)]
mod tests;

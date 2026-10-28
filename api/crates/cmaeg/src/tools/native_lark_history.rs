//! Caller-driven native chat/thread scans; never auto-drain or expand the original container.
use super::ToolsState;
use crate::{
    auth::AgentCaller,
    channels::resolver::ResolvedChannel,
    native_lark::{
        history_authority::{self, denied, unavailable, Authority},
        history_projection, Client, Error,
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
    seal::{EventRef, EventRefPayload, HistoryCursor, ReplyAddress},
};
use std::{sync::Arc, time::Duration};
use tokio::{sync::Semaphore, time::Instant};

pub struct NativeLarkConversationHistory {
    api: Client,
    slots: Semaphore,
    budget: Duration,
    mixed_media: bool,
}
impl NativeLarkConversationHistory {
    pub fn new(api: Client, mixed_media: bool) -> Self {
        Self {
            api,
            slots: Semaphore::new(8),
            budget: Duration::from_secs(8),
            mixed_media,
        }
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
fn read_problem(error: Error) -> Problem {
    match error {
        Error::RateLimited => Problem::new(
            ProblemCode::RateLimited,
            429,
            "native_lark_history_rate_limited",
        ),
        Error::Rejected | Error::Invalid => denied(),
        Error::Unconfirmed => unavailable(),
    }
}
impl ToolsState {
    async fn native_history_authority(
        &self,
        caller: AgentCaller,
        event: &EventRefPayload,
    ) -> Result<(Arc<ResolvedChannel>, Authority), Problem> {
        let channel = self
            .resolver
            .resolve_current(event.channel_id.as_str())
            .await
            .map_err(|e| e.problem())?;
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
    pub(super) async fn native_history_capabilities(
        &self,
        caller: AgentCaller,
        event: &EventRefPayload,
        authorized: &ResolvedChannel,
    ) -> Option<ConversationHistoryCapabilities> {
        let client = self.native_lark_conversation_history.as_ref()?;
        if event.lineage.is_some()
            || event.expires_at <= now()
            || !matches!(&event.reply, ReplyAddress::NativeLark(_))
            || !authorized
                .definition
                .sources
                .iter()
                .any(|s| matches!(s,Source::NativeLark(s) if s.conversation_history))
        {
            return None;
        }
        let _permit = client.slots.try_acquire().ok()?;
        super::state::owner(
            caller,
            authorized,
            event.consumer.as_ref(),
            "readEventHistory",
        )
        .ok()?;
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
    pub(super) async fn read_native_lark_history(
        &self,
        caller: AgentCaller,
        reference: &EventRef,
        cursor: Option<&HistoryCursor>,
        event: EventRefPayload,
        started: Instant,
    ) -> Result<ConversationHistoryPage, Problem> {
        let client = self
            .native_lark_conversation_history
            .as_ref()
            .ok_or_else(|| {
                Problem::new(ProblemCode::Forbidden, 403, "conversation_history_disabled")
            })?;
        let end = started + client.budget;
        let _permit = client.slots.try_acquire().map_err(|_| {
            Problem::new(ProblemCode::RateLimited, 429, "conversation_history_busy")
        })?;
        tokio::time::timeout_at(
            end,
            self.native_history_inner(caller, reference, cursor, event, client, end),
        )
        .await
        .map_err(|_| unavailable())?
    }
    async fn native_history_inner(
        &self,
        caller: AgentCaller,
        reference: &EventRef,
        cursor: Option<&HistoryCursor>,
        event: EventRefPayload,
        client: &NativeLarkConversationHistory,
        end: Instant,
    ) -> Result<ConversationHistoryPage, Problem> {
        let (channel, authority) = self.native_history_authority(caller, &event).await?;
        let mut position = if let Some(cursor) = cursor {
            let p = cursor
                .open_native_lark(&self.keys, now())
                .map_err(|_| invalid_cursor())?
                .payload;
            let a = &authority.cursor;
            if p.event_sha256 != a.event_sha256
                || p.custody != a.custody
                || p.origin != a.origin
                || p.anchor_created_ms != a.anchor_created_ms
                || p.expires_at != event.expires_at
            {
                return Err(invalid_cursor());
            }
            p
        } else {
            authority.cursor.clone()
        };
        let credential = crate::native_lark::credentials(&channel.definition, &event.reply)
            .ok_or_else(denied)?;
        let token = client
            .api
            .verified_token(&position.origin.identity, credential)
            .await
            .map_err(read_problem)?;
        client
            .api
            .history_anchor(&token, &authority)
            .await
            .map_err(read_problem)?;
        let rows = client
            .api
            .history_page(&token, &position)
            .await
            .map_err(read_problem)?;
        let fetched_at = time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|_| unavailable())?;
        let projection = history_projection::project(&rows, &position, client.mixed_media)
            .map_err(read_problem)?;
        client
            .api
            .history_anchor(&token, &authority)
            .await
            .map_err(read_problem)?;
        let (_, current) = self.native_history_authority(caller, &event).await?;
        if current.cursor.custody != authority.cursor.custody {
            return Err(denied());
        }
        let issued_at = now();
        reference
            .open(&self.keys, issued_at)
            .map_err(super::history::reference_problem)?;
        let before = position.before_message_id.clone();
        let continuation = if let Some(next) = projection.next {
            position.before_message_id = projection.before;
            position.last_scanned_ms = projection.last_ms;
            position.last_scanned_id = projection.last_id;
            position.page_token = next;
            position.issued_at = issued_at;
            HistoryContinuation::MoreVisibleRows {
                cursor: HistoryCursor::seal_native_lark(&position, &self.keys)
                    .map_err(|_| unavailable())?,
            }
        } else {
            HistoryContinuation::EndOfVisibleHistory
        };
        let page = ConversationHistoryPage {
            conversation: authority.activity.conversation,
            anchor_message_id: position.origin.message_id,
            before_message_id: before,
            fetched_at,
            expires_at: event.expires_at,
            messages: projection.messages,
            scanned_messages: projection.scanned,
            omitted_message_count: projection.omitted,
            text_truncated: projection.text_truncated,
            metadata_truncated: false,
            content_coverage: HistoryContentCoverage::PlatformReturnedOnly,
            continuation,
        };
        if serde_json::to_vec(&page).map_err(|_| unavailable())?.len() > HISTORY_OUTPUT_BYTES
            || Instant::now() >= end
        {
            return Err(unavailable());
        }
        reference
            .open(&self.keys, now())
            .map_err(super::history::reference_problem)?;
        Ok(page)
    }
}
#[cfg(test)]
mod tests;

//! Bounded rich Agent replay and authenticated first-party transport eligibility.
use crate::{
    Client, Error, Result,
    http::{self, Request},
};
use serde::de::DeserializeOwned;
mod cursor;
mod kinds;
mod stream;
mod types;
mod validation;
pub use cursor::{AgentCursor, AgentPageCursor};
pub use kinds::*;
pub use stream::AgentEventStream;
pub use types::*;
const MAX_BYTES: usize = 4 * 1024 * 1024;

/// One bounded fixed-head snapshot request.
#[derive(Debug, Clone, Default)]
pub struct SnapshotOptions {
    /// Maximum rows from 1 through 200; the server defaults to 100.
    pub limit: Option<u16>,
    /// Exact continuation from the preceding page, never the tail cursor.
    pub page_cursor: Option<AgentPageCursor>,
}
/// One read-only replay/tail connection, without implicit reconnect.
#[derive(Debug, Clone, Default)]
pub struct TailOptions {
    /// Request real server observation timestamps for live fragments; older servers may omit them.
    pub live_timestamps: bool,
    /// Last fully applied Agent cursor; omission starts at retained history's beginning.
    pub after_cursor: Option<AgentCursor>,
    /// Standard SSE header alternative; if both coordinates are supplied they must agree.
    pub last_event_id: Option<AgentCursor>,
    /// Follow future events when true; defaults to true on the server.
    pub follow: Option<bool>,
}
/// Agent event and eligibility resource operations.
pub struct AgentEventsV2<'a> {
    client: &'a Client,
}
impl Client {
    /// Access rich Agent history with opaque cursors independent of Responses.
    pub fn agent_events_v2(&self) -> AgentEventsV2<'_> {
        AgentEventsV2 { client: self }
    }
}
impl AgentEventsV2<'_> {
    /// Read the current tail boundary and active turn while history loads separately.
    /// Rebase new frames over a complete snapshot before acknowledging its history.
    pub async fn head(&self, agent_id: &str) -> Result<AgentEventHead> {
        cursor::identifier(agent_id)?;
        validation::head(
            self.read(&["api", "v2", "agents", agent_id, "events", "head"], &[])
                .await?,
            agent_id,
        )
    }
    /// Observe eligibility for new-tab transport selection under authentication.
    pub async fn read_transport_eligibility(&self) -> Result<UiTransport> {
        validation::eligibility(self.read(&["api", "v2", "ui", "transport"], &[]).await?)
    }
    /// Fetch one bounded fixed-head page. Apply all pages before tailing through_cursor.
    pub async fn snapshot(
        &self,
        agent_id: &str,
        options: &SnapshotOptions,
    ) -> Result<AgentEventPage> {
        cursor::identifier(agent_id)?;
        let limit = options.limit.unwrap_or(100);
        if !(1..=200).contains(&limit) {
            return Err(Error::validation(
                "Agent snapshot limit must be 1 through 200",
            ));
        }
        let mut query = Vec::new();
        if let Some(limit) = options.limit {
            query.push(("limit", limit.to_string()));
        }
        if let Some(cursor) = &options.page_cursor {
            query.push(("page_cursor", cursor.as_str().into()));
        }
        validation::page(
            self.read(
                &["api", "v2", "agents", agent_id, "events", "snapshot"],
                &query,
            )
            .await?,
            agent_id,
            limit,
            options.page_cursor.as_ref(),
        )
    }
    /// Open one read-only tail; dropping observation never submits or cancels work.
    pub async fn events(&self, agent_id: &str, options: &TailOptions) -> Result<AgentEventStream> {
        cursor::identifier(agent_id)?;
        if let (Some(query), Some(header)) = (&options.after_cursor, &options.last_event_id)
            && query != header
        {
            return Err(Error::validation(
                "after_cursor and Last-Event-ID must agree",
            ));
        }
        let segments = ["api", "v2", "agents", agent_id, "events"];
        let mut query = Vec::new();
        if let Some(cursor) = &options.after_cursor {
            query.push(("after_cursor", cursor.as_str().into()));
        }
        if let Some(follow) = options.follow {
            query.push(("follow", follow.to_string()));
        }
        let mut request = Request::get(&segments);
        request.root = true;
        request.query = &query;
        if options.live_timestamps {
            request.headers.insert(
                "prefer",
                reqwest::header::HeaderValue::from_static("cma-live-observed-at"),
            );
        }
        if let Some(cursor) = &options.last_event_id {
            request.headers.insert(
                "last-event-id",
                cursor
                    .as_str()
                    .parse()
                    .map_err(|_| Error::validation("invalid Agent resume header"))?,
            );
        }
        let stream =
            crate::stream::open_bounded(&self.client.inner, request, MAX_BYTES + 1024).await?;
        Ok(AgentEventStream::new(
            stream,
            agent_id,
            options
                .after_cursor
                .as_ref()
                .or(options.last_event_id.as_ref()),
        ))
    }
    async fn read<T: DeserializeOwned>(
        &self,
        segments: &[&str],
        query: &[(&str, String)],
    ) -> Result<T> {
        let mut request = Request::get(segments);
        request.root = true;
        request.query = query;
        let received = http::receive_bounded(&self.client.inner, request, MAX_BYTES).await?;
        if !received
            .headers
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| {
                value
                    .split(';')
                    .next()
                    .is_some_and(|kind| kind.trim().eq_ignore_ascii_case("application/json"))
            })
        {
            return Err(Error::protocol("Agent read requires application/json"));
        }
        serde_json::from_slice(&received.body)
            .map_err(|_| Error::protocol("invalid Agent event resource document"))
    }
}

//! Pull-based AG-UI runs, durable replay, and read-only hierarchy companions.

use crate::{
    Client, Error, Result,
    http::Request,
    resources::identity,
    stream::{self, EventStream},
};
use reqwest::{Method, StatusCode};
mod types;
pub use types::*;

/// Exclusive replay cursor and finite-history or follow mode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReplayOptions {
    /// Last fully acknowledged durable sequence.
    pub after: Option<u64>,
    /// Follow future events; false closes after captured durable history.
    pub follow: bool,
}

/// AG-UI stream validating Agent identity and durable cursor progression.
pub struct AgentStream {
    inner: EventStream,
    thread_id: String,
    cursor: u64,
    last_id: String,
    ended: bool,
}

impl AgentStream {
    /// Read one protocol event; no automatic reconnect or command retry occurs.
    pub async fn next(&mut self) -> Option<Result<AgentEvent>> {
        if self.ended {
            return None;
        }
        let next = self.inner.next().await?;
        let result = next.and_then(|frame| {
            let mut event: AgentEvent = serde_json::from_value(frame.data)
                .map_err(|_| Error::protocol("invalid AG-UI event document"))?;
            identity(&event.thread_id, &self.thread_id)?;
            if frame.event != "message" || event.run_id.is_empty() {
                return Err(Error::protocol("AG-UI event omitted run identity"));
            }
            if !frame.last_event_id.is_empty() && frame.last_event_id != self.last_id {
                let cursor = frame
                    .last_event_id
                    .parse::<u64>()
                    .map_err(|_| Error::protocol("invalid AG-UI durable cursor"))?;
                if event.live || cursor <= self.cursor {
                    return Err(Error::protocol("AG-UI durable cursor did not advance"));
                }
                event.cursor = Some(cursor);
                self.cursor = cursor;
                self.last_id = frame.last_event_id;
            }
            Ok(event)
        });
        if result.is_err() {
            self.ended = true;
        }
        Some(result)
    }

    /// Last complete durable record yielded by this reader; live frames never advance it.
    pub fn cursor(&self) -> u64 {
        self.cursor
    }
}

/// Public live conversation operations.
pub struct Agui<'a> {
    client: &'a Client,
}

impl Agui<'_> {
    /// Read declared supported and unsupported protocol capabilities.
    pub async fn read_capabilities(&self) -> Result<Capabilities> {
        let result: Capabilities = self.client.get(&["agui", "capabilities"], &[]).await?;
        if result.protocol != "ag-ui"
            || !result.transports.iter().any(|transport| transport == "sse")
        {
            return Err(Error::protocol("unexpected AG-UI capability declaration"));
        }
        Ok(result)
    }
    /// Read the immutable hierarchy companion for an existing Agent.
    pub async fn read_sandbox(&self, id: &str) -> Result<SandboxCompanion> {
        let result: SandboxCompanion = self
            .client
            .get(&["agui", "agents", id, "sandbox"], &[])
            .await?;
        identity(&result.thread_id, id)?;
        Ok(result)
    }
    /// Admit exactly one message or approval answer; dropping the stream does not cancel work.
    pub async fn run(&self, body: &RunRequest) -> Result<AgentStream> {
        self.submit(body, None).await
    }
    /// Recover an uncertain run by retaining this key and the same input. No automatic retry occurs.
    pub async fn run_with_key(&self, body: &RunRequest, key: &str) -> Result<AgentStream> {
        self.submit(body, Some(key)).await
    }
    async fn submit(&self, body: &RunRequest, key: Option<&str>) -> Result<AgentStream> {
        if let Some(props) = &body.forwarded_props {
            props.answer_context.validate()?;
            if !body.resume.is_empty() {
                return Err(Error::validation(
                    "answer context is only valid for user input",
                ));
            }
        }
        if body.thread_id.is_empty()
            || body.resume.len() > 1
            || body
                .run_id
                .as_ref()
                .is_some_and(|id| id.trim().is_empty() || id.len() > 128)
            || (body.resume.is_empty()
                && body
                    .messages
                    .last()
                    .is_none_or(|message| message.content.trim().is_empty()))
        {
            return Err(Error::validation(
                "AG-UI run requires an existing Agent and a user message or one approval answer",
            ));
        }
        let request =
            Request::operation(Method::POST, &["agui", "run"], Some(body), StatusCode::OK)?
                .with_command_key(key)?;
        Ok(AgentStream {
            inner: stream::open(&self.client.inner, request).await?,
            thread_id: body.thread_id.clone(),
            cursor: 0,
            last_id: String::new(),
            ended: false,
        })
    }
    /// Open one durable replay connection; resume explicitly using the last consumed cursor.
    pub async fn events(&self, id: &str, options: ReplayOptions) -> Result<AgentStream> {
        let mut query = vec![
            ("thread_id", id.into()),
            ("follow", options.follow.to_string()),
        ];
        if let Some(after) = options.after {
            query.push(("starting_after", after.to_string()));
        }
        let mut request = Request::get(&["agui", "events"]);
        request.query = &query;
        Ok(AgentStream {
            inner: stream::open(&self.client.inner, request).await?,
            thread_id: id.into(),
            cursor: options.after.unwrap_or(0),
            last_id: String::new(),
            ended: false,
        })
    }
}

impl Client {
    /// Access public AG-UI conversation and replay helpers.
    pub fn agui(&self) -> Agui<'_> {
        Agui { client: self }
    }
}

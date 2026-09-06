//! Bounded, pull-based SSE transport using the standard eventsource parser.

use crate::{
    Error, Result,
    client::ClientInner,
    http::{Request, outgoing, parse_retry_after, problem_error},
};
use eventsource_stream::{EventStreamError, Eventsource};
use futures_util::{Stream, StreamExt};
use reqwest::{StatusCode, header};
use std::pin::Pin;

const MAX_FRAME_BYTES: usize = 1024 * 1024;
const MAX_ERROR_BYTES: usize = 8 * 1024 * 1024;

/// One parsed SSE event, retaining the standard inherited last-event identifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerEvent {
    /// SSE event name; `message` when no explicit event name was sent.
    pub event: String,
    /// Standard inherited last-event ID, not necessarily an ID on this frame.
    pub last_event_id: String,
    /// JSON data carried by this event.
    pub data: serde_json::Value,
}

/// Pull-based event reader; dropping it cancels only observation, never durable work.
pub struct EventStream {
    inner: Pin<Box<dyn Stream<Item = Result<ServerEvent>> + Send>>,
    ended: bool,
}

impl EventStream {
    /// Read the next event without reconnecting or retrying an admitted command.
    pub async fn next(&mut self) -> Option<Result<ServerEvent>> {
        if self.ended {
            return None;
        }
        let next = self.inner.next().await;
        if next.as_ref().is_none_or(Result::is_err) {
            self.ended = true;
        }
        next
    }
}

pub(crate) async fn open(client: &ClientInner, mut request: Request<'_>) -> Result<EventStream> {
    request.headers.insert(
        header::ACCEPT,
        header::HeaderValue::from_static("text/event-stream"),
    );
    let mut response = outgoing(client, request)?
        .send()
        .await
        .map_err(|_| Error::transport("event stream could not obtain an HTTP response"))?;
    if response.status() != StatusCode::OK {
        let status = response.status();
        let request_id = response
            .headers()
            .get("x-request-id")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let retry_after = response
            .headers()
            .get(header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(parse_retry_after);
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| Error::transport("event stream problem body was interrupted"))?
        {
            if chunk.len() > MAX_ERROR_BYTES.saturating_sub(body.len()) {
                return Err(Error::protocol(
                    "event stream problem body exceeded its bound",
                ));
            }
            body.extend_from_slice(&chunk);
        }
        if status.is_success() {
            return Err(Error::protocol("event stream expected HTTP 200"));
        }
        return Err(problem_error(status, request_id, retry_after, &body));
    }
    if !response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(';')
                .next()
                .is_some_and(|kind| kind.trim().eq_ignore_ascii_case("text/event-stream"))
        })
    {
        return Err(Error::protocol("event stream requires text/event-stream"));
    }
    // Bound incomplete frames before the parser can accumulate arbitrary remote data.
    let mut guard = FrameBound::default();
    let bytes = response.bytes_stream().map(move |chunk| {
        let chunk = chunk.map_err(|_| Error::transport("event stream body was interrupted"))?;
        guard.inspect(&chunk)?;
        Ok::<_, Error>(chunk)
    });
    let parsed = bytes.eventsource().map(|event| {
        let event = event.map_err(|error| match error {
            EventStreamError::Transport(error) => error,
            _ => Error::protocol("event stream contained invalid framing"),
        })?;
        let data = serde_json::from_str(&event.data)
            .map_err(|_| Error::protocol("event stream data was not valid JSON"))?;
        Ok(ServerEvent {
            event: event.event,
            last_event_id: event.id,
            data,
        })
    });
    Ok(EventStream {
        inner: Box::pin(parsed),
        ended: false,
    })
}

#[derive(Default)]
struct FrameBound {
    bytes: usize,
    line_content: bool,
    previous_cr: bool,
}

impl FrameBound {
    fn inspect(&mut self, chunk: &[u8]) -> Result<()> {
        for &byte in chunk {
            self.bytes += 1;
            if self.bytes > MAX_FRAME_BYTES {
                return Err(Error::protocol("event stream frame exceeded its bound"));
            }
            if byte == b'\n' && self.previous_cr {
                self.previous_cr = false;
                continue;
            }
            self.previous_cr = byte == b'\r';
            if matches!(byte, b'\r' | b'\n') {
                if !self.line_content {
                    self.bytes = 0;
                }
                self.line_content = false;
            } else {
                self.line_content = true;
            }
        }
        Ok(())
    }
}

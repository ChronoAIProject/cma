//! Reconnecting server-sent event stream.

use std::fmt;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use eventsource_stream::{EventStreamError, Eventsource};
use futures_util::{Stream, StreamExt};

use crate::{Client, Error, Event, Result, SessionId};

const MAX_EVENT_BYTES: usize = 2 * 1024 * 1024;

/// A cancellation-safe stream of validated, typed session events.
///
/// Dropping this value drops the active response body, reconnect timer, and all
/// pending work. It does not mutate shared client state, so another request can
/// use the cloned client immediately.
pub struct EventStream {
    inner: Pin<Box<dyn Stream<Item = Result<Event>> + Send + 'static>>,
}

impl EventStream {
    pub(crate) async fn open(
        client: Client,
        session_id: SessionId,
        starting_after: Option<u64>,
    ) -> Result<Self> {
        let cursor = starting_after.unwrap_or(0);
        let response = client.connect_events(&session_id, cursor).await?;
        let stream = event_stream(client, session_id, cursor, response);
        Ok(Self {
            inner: Box::pin(stream),
        })
    }

    /// Wait for the next event or stream failure.
    ///
    /// This inherent method avoids requiring callers to import `StreamExt` for
    /// the common case. The type also implements [`futures_util::Stream`].
    pub async fn next(&mut self) -> Option<Result<Event>> {
        StreamExt::next(self).await
    }
}

impl fmt::Debug for EventStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EventStream")
            .field("connection", &"[streaming]")
            .finish()
    }
}

impl Stream for EventStream {
    type Item = Result<Event>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}

fn event_stream(
    client: Client,
    session_id: SessionId,
    starting_cursor: u64,
    first_response: reqwest::Response,
) -> impl Stream<Item = Result<Event>> + Send + 'static {
    async_stream::stream! {
        let mut cursor = starting_cursor;
        let mut response = Some(first_response);
        let mut consecutive_empty_connections = 0_usize;
        let mut server_retry = None::<Duration>;

        loop {
            let Some(current_response) = response.take() else {
                yield Err(Error::transport("event stream lost its active response"));
                return;
            };
            let mut source = current_response.bytes_stream().eventsource();
            let mut received_event = false;

            while let Some(frame) = source.next().await {
                let frame = match frame {
                    Ok(frame) => frame,
                    Err(EventStreamError::Transport(_)) => break,
                    Err(EventStreamError::Utf8(error)) => {
                        yield Err(Error::protocol(format!(
                            "event stream is not valid UTF-8: {error}"
                        )));
                        return;
                    }
                    Err(EventStreamError::Parser(error)) => {
                        yield Err(Error::protocol(format!(
                            "event stream is not valid SSE: {error}"
                        )));
                        return;
                    }
                };

                if frame.event != "message" {
                    yield Err(Error::protocol(format!(
                        "event stream used named event '{}' instead of the default message event",
                        frame.event
                    )));
                    return;
                }
                if frame.id.is_empty() {
                    yield Err(Error::protocol("event stream frame omitted its sequence id"));
                    return;
                }
                let frame_sequence = match frame.id.parse::<u64>() {
                    Ok(sequence) if sequence > 0 => sequence,
                    _ => {
                        yield Err(Error::protocol(format!(
                            "event stream frame id '{}' is not a positive sequence",
                            frame.id
                        )));
                        return;
                    }
                };
                if frame.data.len() > MAX_EVENT_BYTES {
                    yield Err(Error::protocol(format!(
                        "event frame exceeds the {MAX_EVENT_BYTES}-byte boundary"
                    )));
                    return;
                }
                let event = match serde_json::from_str::<Event>(&frame.data) {
                    Ok(event) => event,
                    Err(error) => {
                        yield Err(Error::protocol(format!(
                            "event frame does not match the CMA wire shape: {error}"
                        )));
                        return;
                    }
                };
                if event.seq != frame_sequence {
                    yield Err(Error::protocol(format!(
                        "event frame id {frame_sequence} does not match payload sequence {}",
                        event.seq
                    )));
                    return;
                }
                if event.session_id != session_id {
                    yield Err(Error::protocol(format!(
                        "event for {} arrived on stream for {session_id}",
                        event.session_id
                    )));
                    return;
                }
                if event.seq < cursor {
                    yield Err(Error::protocol(format!(
                        "event sequence {} moved behind resume cursor {cursor}",
                        event.seq
                    )));
                    return;
                }
                if event.seq == cursor {
                    continue;
                }

                cursor = event.seq;
                received_event = true;
                consecutive_empty_connections = 0;
                if let Some(retry) = frame.retry {
                    server_retry = Some(retry);
                }
                yield Ok(event);
            }

            if !received_event {
                consecutive_empty_connections = consecutive_empty_connections.saturating_add(1);
                if consecutive_empty_connections >= client.retry_policy_ref().max_attempts() {
                    yield Err(Error::transport(
                        "event stream closed repeatedly without delivering an event",
                    ));
                    return;
                }
            }

            let delay = server_retry.unwrap_or_else(|| {
                client
                    .retry_policy_ref()
                    .delay(consecutive_empty_connections.max(1))
            });
            tokio::time::sleep(delay).await;
            match client.connect_events(&session_id, cursor).await {
                Ok(next_response) => response = Some(next_response),
                Err(error) => {
                    yield Err(error);
                    return;
                }
            }
        }
    }
}

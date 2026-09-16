use super::{ResponseEvent, state::State};
use crate::{Error, Result, stream::EventStream};

/// Durable receipt established by the successful SSE handshake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponseReceipt {
    /// Opaque response identity, retained across retry and replay.
    pub response_id: String,
    /// Admitted command identity.
    pub command_id: String,
}
/// Pull-based observation; dropping this stream never resubmits or cancels work.
pub struct ResponseStream {
    inner: EventStream,
    receipt: ResponseReceipt,
    state: State,
    next: u64,
    allow_empty: bool,
    closed: bool,
}
impl ResponseStream {
    pub(super) fn new(
        inner: EventStream,
        conversation: Option<String>,
        response_id: Option<&str>,
        after: Option<u64>,
    ) -> Result<Self> {
        let header = |name| {
            inner
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| Error::protocol("Responses handshake has no durable receipt"))
        };
        let receipt = ResponseReceipt {
            response_id: header("x-cma-response-id")?,
            command_id: header("x-cma-command-id")?,
        };
        if response_id.is_some_and(|id| id != receipt.response_id) {
            return Err(Error::protocol(
                "Responses handshake identifies another response",
            ));
        }
        let state = State::new(receipt.response_id.clone(), conversation, after.is_some());
        Ok(Self {
            inner,
            receipt,
            state,
            next: after
                .map_or(Some(0), |n| n.checked_add(1))
                .ok_or_else(|| Error::validation("replay cursor overflow"))?,
            allow_empty: after.is_some(),
            closed: false,
        })
    }
    /// Retain this receipt before consuming any frames; it supports recovery after disconnect.
    pub fn receipt(&self) -> &ResponseReceipt {
        &self.receipt
    }
    /// Receive one validated event without automatic retries or reconnects.
    pub async fn next(&mut self) -> Option<Result<ResponseEvent>> {
        if self.closed {
            return None;
        }
        if self.state.connection_error {
            self.closed = true;
            return Some(Err(Error::transport(
                "Response observation ended; retrieve or replay with current authorization",
            )));
        }
        let Some(frame) = self.inner.next().await else {
            self.closed = true;
            return if self.state.ended || self.allow_empty {
                None
            } else {
                Some(Err(Error::transport(
                    "Responses stream closed before its outcome was known",
                )))
            };
        };
        let result = frame.and_then(|frame| {
            let event: ResponseEvent = serde_json::from_value(frame.data)
                .map_err(|_| Error::protocol("invalid Responses v2 event document"))?;
            let error = event.payload.kind() == "error";
            if frame.event != event.payload.kind()
                || event.sequence_number != self.next
                || if error {
                    frame.explicit_id.is_some()
                } else {
                    frame.explicit_id.as_deref() != Some(self.next.to_string().as_str())
                }
            {
                return Err(Error::protocol(
                    "Responses v2 event name, durable ID or sequence disagrees with its envelope",
                ));
            }
            self.state.accept(&event)?;
            self.next = self
                .next
                .checked_add(1)
                .ok_or_else(|| Error::protocol("Responses sequence overflow"))?;
            self.allow_empty = false;
            Ok(event)
        });
        if result.is_err() {
            self.closed = true;
        }
        Some(result)
    }
}

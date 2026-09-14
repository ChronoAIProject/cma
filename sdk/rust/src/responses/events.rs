//! Named Responses stream events and connection-local sequence checking.

use crate::{Error, Result, stream::EventStream};
use serde::Deserialize;
use std::collections::BTreeMap;

string_enum! { /// Named event emitted by CMA's Responses subset.
    pub enum ResponseEventKind {
        Created => "response.created", InProgress => "response.in_progress", Completed => "response.completed",
        Failed => "response.failed", Incomplete => "response.incomplete",
        OutputItemAdded => "response.output_item.added", OutputItemDone => "response.output_item.done",
        ContentPartAdded => "response.content_part.added", ContentPartDone => "response.content_part.done",
        OutputTextDelta => "response.output_text.delta", OutputTextDone => "response.output_text.done",
        ReasoningSummaryPartAdded => "response.reasoning_summary_part.added", ReasoningSummaryPartDone => "response.reasoning_summary_part.done",
        ReasoningSummaryTextDelta => "response.reasoning_summary_text.delta", ReasoningSummaryTextDone => "response.reasoning_summary_text.done",
        ToolActivity => "response.cma_chat.tool_activity"
    }
}

/// Named Responses envelope with standard and explicit CMA extension payloads retained.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ResponseEvent {
    /// Named standard or CMA extension event.
    #[serde(rename = "type")]
    pub kind: ResponseEventKind,
    /// Connection-local sequence, not a durable replay cursor.
    pub sequence_number: u64,
    /// Type-specific response, item, content, or text-delta fields.
    #[serde(flatten)]
    pub payload: BTreeMap<String, serde_json::Value>,
}

/// One non-resumable Responses observation connection.
pub struct ResponseStream {
    pub(crate) inner: EventStream,
    pub(crate) sequence: u64,
    pub(crate) ended: bool,
}

impl ResponseStream {
    /// Read a named event, validating exact connection-local order and terminal completion.
    pub async fn next(&mut self) -> Option<Result<ResponseEvent>> {
        if self.ended {
            return None;
        }
        let frame = match self.inner.next().await {
            Some(frame) => frame,
            None => {
                self.ended = true;
                return Some(Err(Error::transport(
                    "Responses stream ended before a terminal event; retrieve the response before retrying",
                )));
            }
        };
        let result = frame.and_then(|frame| {
            let event: ResponseEvent = serde_json::from_value(frame.data)
                .map_err(|_| Error::protocol("invalid Responses event document"))?;
            if frame.event != event.kind.as_str()
                || !frame.last_event_id.is_empty()
                || event.sequence_number != self.sequence
            {
                return Err(Error::protocol(
                    "Responses event name, cursor, or sequence did not match its envelope",
                ));
            }
            self.sequence = self
                .sequence
                .checked_add(1)
                .ok_or_else(|| Error::protocol("Responses sequence overflow"))?;
            if matches!(
                event.kind,
                ResponseEventKind::Completed
                    | ResponseEventKind::Failed
                    | ResponseEventKind::Incomplete
            ) {
                self.ended = true;
            }
            Ok(event)
        });
        if result.is_err() {
            self.ended = true;
        }
        Some(result)
    }
}

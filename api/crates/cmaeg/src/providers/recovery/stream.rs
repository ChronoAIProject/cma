use futures_util::StreamExt;
use serde_json::Value;
use tokio::sync::mpsc;

use super::{
    malformed, timeout_error, ProviderError, RecoveryEvent, RecoveryIdentity, ResponsesEvent,
};
use crate::providers::{parse_sse, SseFrame};
use crate::telemetry::turn_trace::ProviderReader;

const MAX_REPLAY_BYTES: usize = 16 * 1024 * 1024;

pub(super) async fn forward(
    response: reqwest::Response,
    mut identity: RecoveryIdentity,
    starting_after: Option<u64>,
    sender: mpsc::Sender<Result<RecoveryEvent, ProviderError>>,
    _permit: super::super::permits::Permit,
    deadline: tokio::time::Instant,
    mut trace: Option<Box<ProviderReader>>,
) {
    let mut bytes = 0usize;
    let wire = response.bytes_stream().map(move |chunk| {
        let chunk = chunk.map_err(super::transport_error)?;
        bytes = bytes.saturating_add(chunk.len());
        if bytes > MAX_REPLAY_BYTES {
            return Err(malformed("provider replay exceeds byte limit"));
        }
        Ok(chunk)
    });
    let mut frames = Box::pin(parse_sse(wire));
    let mut next = starting_after.map_or(0, |cursor| cursor + 1);
    loop {
        let frame = tokio::select! {
            _ = sender.closed() => return,
            _ = tokio::time::sleep_until(deadline) => { let _ = sender.try_send(Err(timeout_error())); return; },
            frame = frames.next() => frame,
        };
        let event = match frame {
            Some(Ok(frame)) => decode(&frame, &mut identity, next, trace.as_deref_mut()),
            Some(Err(error)) => Err(ProviderError::Sse(error.to_string())),
            None => Err(ProviderError::Sse(
                "durable replay ended before completion".into(),
            )),
        };
        let terminal = event.as_ref().map_or(true, |value| {
            matches!(
                value.event,
                ResponsesEvent::Completed { .. }
                    | ResponsesEvent::Failed { .. }
                    | ResponsesEvent::Incomplete { .. }
            )
        });
        tokio::select! {
            _ = sender.closed() => return,
            // A wire timeout and this deadline can become ready together. Do
            // not silently close an empty channel when the deadline wins.
            _ = tokio::time::sleep_until(deadline) => { let _ = sender.try_send(Err(timeout_error())); return; },
            result = sender.send(event) => if result.is_err() { return; },
        }
        if terminal {
            return;
        }
        let Some(advanced) = next.checked_add(1) else {
            return;
        };
        next = advanced;
    }
}

fn decode(
    frame: &SseFrame,
    identity: &mut RecoveryIdentity,
    next: u64,
    mut trace: Option<&mut ProviderReader>,
) -> Result<RecoveryEvent, ProviderError> {
    let value: Value =
        serde_json::from_str(&frame.data).map_err(|_| malformed("invalid replay JSON"))?;
    let kind = value["type"]
        .as_str()
        .ok_or_else(|| malformed("replay event has no type"))?;
    if frame.event.as_ref().is_some_and(|event| event != kind) {
        return Err(malformed("replay event type mismatch"));
    }
    // A transport/policy observation error is not the command's durable outcome.
    // Its sequence_number is advisory and never becomes a checkpoint cursor.
    if kind == "error" {
        if frame.id.is_some() {
            return Err(malformed("observation error has a durable event ID"));
        }
        return Err(ProviderError::Unavailable(
            "response observation unavailable; retrieve or reconnect".into(),
        ));
    }
    let sequence = value["sequence_number"]
        .as_u64()
        .ok_or_else(|| malformed("replay event has no durable sequence"))?;
    let canonical = sequence.to_string();
    if frame.id.as_deref() != Some(canonical.as_str()) || sequence != next {
        return Err(malformed("replay cursor gap or durable ID mismatch"));
    }
    if value
        .get("response_id")
        .is_some_and(|id| id.as_str() != Some(identity.response_id.as_str()))
    {
        return Err(malformed("replay event response identity mismatch"));
    }
    if let Some(response) = value.get("response") {
        let snapshot = identity.snapshot(response.clone())?;
        identity.native_agent_id = Some(snapshot.native_agent_id);
        let expected_status = match kind {
            "response.created" => Some("queued"),
            "response.in_progress" => Some("in_progress"),
            "response.completed" => Some("completed"),
            "response.failed" => Some("failed"),
            "response.incomplete" => Some("incomplete"),
            _ => None,
        };
        if expected_status.is_some_and(|expected| response["status"].as_str() != Some(expected)) {
            return Err(malformed("replay lifecycle status mismatch"));
        }
    } else if matches!(
        kind,
        "response.created"
            | "response.in_progress"
            | "response.completed"
            | "response.failed"
            | "response.incomplete"
    ) {
        return Err(malformed("replay lifecycle event lacks its response"));
    }
    let event = crate::providers::responses::parse_event_with_trace(frame, trace.as_deref_mut())
        .map_err(|_| malformed("invalid replay event shape"))?;
    Ok(RecoveryEvent {
        trace_contract: trace.is_some(),
        trace: trace.and_then(|reader| reader.association()),
        response_id: identity.response_id.clone(),
        sequence_number: sequence,
        event,
        raw: value,
    })
}

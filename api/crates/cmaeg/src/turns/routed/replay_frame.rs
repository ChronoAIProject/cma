//! Negotiated diagnostics do not participate in execution or delivery identity.
use super::{storage, ExecutionError};
use crate::providers::{parse_event, RecoveryEvent, ResponsesEvent, SseFrame};

pub(super) fn hash(frame: &RecoveryEvent, negotiated: bool) -> Result<String, ExecutionError> {
    if !negotiated
        || (frame.raw.get("nyxid_trace").is_none()
            && frame
                .raw
                .get("response")
                .and_then(|r| r.get("nyxid_trace"))
                .is_none())
    {
        return Ok(storage::hash(&storage::bytes(&frame.raw)?));
    }
    // Only the two protocol-reserved annotations are diagnostic. All output,
    // response identities, cursors, provider metadata and unknown fields keep
    // their original integrity checks. Never recursively strip model content.
    let mut execution = frame.raw.clone();
    if let Some(object) = execution.as_object_mut() {
        object.remove("nyxid_trace");
        if let Some(response) = object
            .get_mut("response")
            .and_then(serde_json::Value::as_object_mut)
        {
            response.remove("nyxid_trace");
        }
    }
    Ok(storage::hash(&storage::bytes(&execution)?))
}

pub(super) fn shape_matches(frame: &RecoveryEvent) -> bool {
    let parsed = parse_event(&SseFrame {
        event: None,
        id: Some(frame.sequence_number.to_string()),
        data: frame.raw.to_string(),
        retry: None,
    });
    match parsed {
        Ok(ResponsesEvent::Unknown { r#type })
            if frame.trace_contract && matches!(frame.event, ResponsesEvent::NyxidTrace) =>
        {
            r#type == "response.nyxid_trace"
        }
        Ok(event) => event == frame.event,
        Err(_) => false,
    }
}

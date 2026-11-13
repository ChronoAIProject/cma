//! Bound inbound ID validation before cloning or encoding rejected values.

use std::io::Cursor;

use serde_json::Value;

const MAX_CALL_BYTES: usize = 256;

// Kept for disabled attachments and the independent restored-locator path.
pub(super) fn valid_call(call: &Value) -> bool {
    matches!(call, Value::String(_) | Value::Number(_)) && call.to_string().len() <= MAX_CALL_BYTES
}

pub(super) fn valid_call_with_policy(call: &Value, bounded: bool) -> bool {
    if !bounded {
        return valid_call(call);
    }
    if !matches!(call, Value::String(_) | Value::Number(_)) {
        return false;
    }
    // Use the same serializer as the legacy check so quotes, escapes and
    // numeric formatting count exactly. Cursor cannot grow this stack buffer.
    let mut encoded = [0; MAX_CALL_BYTES];
    serde_json::to_writer(Cursor::new(&mut encoded[..]), call).is_ok()
}

pub(super) fn permission_call(frame: &Value, bounded: bool) -> Option<Value> {
    if bounded {
        frame
            .get("id")
            .filter(|call| valid_call_with_policy(call, true))
            .cloned()
    } else {
        frame.get("id").cloned().filter(valid_call)
    }
}

#[cfg(test)]
#[path = "acp_correlation_tests.rs"]
mod tests;

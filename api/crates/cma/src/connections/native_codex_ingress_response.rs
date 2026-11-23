//! Borrowed inspection for the future qualified native response owner.

use serde_json::Value;

use super::ingress_protocol::NativeFrameError;

#[derive(Debug, PartialEq)]
pub(super) enum NativeResponseView<'a> {
    Result(&'a Value),
    Completed { login_id: &'a str, success: bool },
    Ignore,
}

#[cfg_attr(
    not(test),
    expect(dead_code, reason = "native response owner qualification pending")
)]
pub(super) fn inspect_response(
    frame: &Value,
    expected_id: u64,
) -> Result<NativeResponseView<'_>, NativeFrameError> {
    if frame.get("id").and_then(Value::as_u64) == Some(expected_id) {
        return frame
            .get("result")
            .map(NativeResponseView::Result)
            .ok_or(NativeFrameError::Unavailable);
    }
    if frame.get("method").and_then(Value::as_str) != Some("account/login/completed") {
        return Ok(NativeResponseView::Ignore);
    }
    let Some(params) = frame.get("params") else {
        return Ok(NativeResponseView::Ignore);
    };
    if let (Some(login_id), Some(success)) = (
        params.get("loginId").and_then(Value::as_str),
        params.get("success").and_then(Value::as_bool),
    ) {
        return Ok(NativeResponseView::Completed { login_id, success });
    }
    Ok(NativeResponseView::Ignore)
}

#[cfg(test)]
#[path = "native_codex_ingress_response_tests.rs"]
mod tests;

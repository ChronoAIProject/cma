//! Apply existing native identity rules before copying rejected retained input.

use serde_json::Value;

use crate::AdapterError;

pub(super) fn validate(session: &str) -> crate::Result<()> {
    if session.is_empty() || session.len() > 1_024 || session.chars().any(char::is_control) {
        return Err(AdapterError::Rejected(
            "bridge returned an invalid conversation identifier".into(),
        ));
    }
    Ok(())
}

pub(super) fn read(result: &Value, bounded: bool) -> crate::Result<String> {
    let session = result
        .get("sessionId")
        .and_then(Value::as_str)
        .ok_or_else(|| AdapterError::Rejected("bridge named no conversation".into()))?;
    if bounded {
        validate(session)?;
    }
    Ok(session.to_owned())
}

#[cfg(test)]
#[path = "acp_session_identity_tests.rs"]
mod tests;

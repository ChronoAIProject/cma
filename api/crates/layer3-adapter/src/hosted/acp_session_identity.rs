//! Apply existing native identity rules before copying rejected retained input.

use crate::ingress_budget::diagnostic::text;
use serde_json::Value;

use crate::AdapterError;

pub(super) const MAX_BYTES: usize = 1_024;

pub(super) fn validate(session: &str) -> crate::Result<()> {
    if session.is_empty() || session.len() > MAX_BYTES || session.chars().any(char::is_control) {
        return Err(AdapterError::Rejected(text(
            "bridge returned an invalid conversation identifier",
        )));
    }
    Ok(())
}

pub(super) fn read(result: &Value, bounded: bool) -> crate::Result<String> {
    let session = result
        .get("sessionId")
        .and_then(Value::as_str)
        .ok_or_else(|| AdapterError::Rejected(text("bridge named no conversation")))?;
    if bounded {
        validate(session)?;
    }
    Ok(session.to_owned())
}

#[cfg(test)]
#[path = "acp_session_identity_tests.rs"]
mod tests;

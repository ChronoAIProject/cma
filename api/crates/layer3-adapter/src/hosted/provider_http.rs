use futures_util::StreamExt as _;

use crate::wire::{ResponseFrame, WireFailure};
use crate::AdapterError;

pub(super) const CONTROL_RESPONSE_BYTES: usize = 384 * 1024 * 1024;

pub(super) async fn decode_control(
    response: reqwest::Response,
    operation: crate::wire::WireOperation,
) -> crate::Result<ResponseFrame> {
    let status = response.status();
    if response
        .content_length()
        .is_some_and(|length| length > CONTROL_RESPONSE_BYTES as u64)
    {
        return Err(AdapterError::Rejected(
            "runtime control response exceeds the size limit".into(),
        ));
    }
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| {
            crate::workspace_prepare_observation::transport(operation);
            AdapterError::Unavailable(format!("runtime control response failed: {error}"))
        })?;
        if bytes.len().saturating_add(chunk.len()) > CONTROL_RESPONSE_BYTES {
            return Err(AdapterError::Rejected(
                "runtime control response exceeds the size limit".into(),
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    decode_control_bytes(status, &bytes)
}

pub(super) fn decode_control_bytes(
    status: http::StatusCode,
    bytes: &[u8],
) -> crate::Result<ResponseFrame> {
    if !status.is_success() {
        let detail = String::from_utf8_lossy(bytes);
        let message = format!("runtime control returned {status}: {detail}");
        return if status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            Err(AdapterError::Unavailable(message))
        } else {
            Err(AdapterError::Rejected(message))
        };
    }
    serde_json::from_slice(bytes)
        .map_err(|_| AdapterError::Rejected("runtime control response is malformed".into()))
}

pub(super) fn runtime_failure(failure: WireFailure) -> AdapterError {
    let detail = format!("{}: {}", failure.code, failure.detail);
    if super::reply::ledger_hold(&failure) {
        AdapterError::RuntimeLedgerHold(detail)
    } else if failure.retryable {
        AdapterError::Unavailable(detail)
    } else if matches!(
        failure.code.as_str(),
        "mutation_authority_conflict"
            | "runtime_mutation_replay_mismatch"
            | "attachment_upload_conflict"
    ) {
        AdapterError::OperationIdentityConflict(detail)
    } else if failure.code == "at_capacity" {
        AdapterError::AtCapacity(detail)
    } else {
        AdapterError::Rejected(detail)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::WireOperation;

    #[test]
    fn only_exact_nonretryable_identity_codes_enable_reconnect_compatibility() {
        for (code, retryable, conflict) in [
            ("mutation_authority_conflict", false, true),
            ("runtime_mutation_replay_mismatch", false, true),
            ("mutation_authority_conflict", true, false),
            ("mutation_authority_stale", false, false),
            ("mutation_authority_retired", false, false),
            ("runtime_mutation_replay_mismatch_suffix", false, false),
            ("agent_process_not_live", false, false),
        ] {
            let error = runtime_failure(WireFailure {
                operation: WireOperation::OpenAgent,
                code: code.into(),
                detail: "runtime_mutation_replay_mismatch".into(),
                retryable,
            });
            assert_eq!(
                matches!(error, AdapterError::OperationIdentityConflict(_)),
                conflict
            );
        }
    }
}

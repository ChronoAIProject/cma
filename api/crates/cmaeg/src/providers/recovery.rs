//! Read-only reattachment to an already accepted CMA Responses v2 command.
//! No recovery operation submits, retries or recreates a turn.
use std::{collections::BTreeMap, time::Duration};

use cmaeg_core::{channel::Secret, ids::ProviderSlug};
use serde_json::Value;
use tokio::sync::mpsc;

use super::{
    client::{acquire, transport_error, validate_metadata},
    ProviderClient, ProviderError, ResponsesEvent,
};

mod stream;

const MAX_SNAPSHOT_BYTES: usize = 4 * 1024 * 1024;
const MAX_ERROR_BYTES: usize = 64 * 1024;
const MAX_RECOVERY_TIME: Duration = Duration::from_secs(60);

/// Identity frozen by the caller's accepted, authenticated execution journal.
/// `native_agent_id` is CMA's actual Agent ID, never the gateway conversation alias.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryIdentity {
    pub response_id: String,
    pub native_agent_id: Option<String>,
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RecoverySnapshot {
    pub native_agent_id: String,
    pub response: Value,
}

/// Only a durable frame has this shape. Persist its cursor only after the
/// journal's fenced delivery/checkpoint succeeds, never merely on receipt.
#[derive(Debug, Clone, PartialEq)]
pub struct RecoveryEvent {
    pub response_id: String,
    pub sequence_number: u64,
    pub event: ResponsesEvent,
    pub raw: Value,
}

#[derive(Debug)]
pub struct RecoveryStream {
    pub events: mpsc::Receiver<Result<RecoveryEvent, ProviderError>>,
}

impl ProviderClient {
    pub fn supports_response_replay(&self, provider: &ProviderSlug) -> Result<bool, ProviderError> {
        Ok(self.handle(provider)?.entry.paths.recovery.is_some())
    }

    /// Validate the accepted response's immutable identity under fresh current
    /// provider authority. The caller can persist/pin the returned native Agent ID.
    pub async fn response_snapshot(
        &self,
        provider: &ProviderSlug,
        identity: &RecoveryIdentity,
        agent_key: &Secret,
    ) -> Result<RecoverySnapshot, ProviderError> {
        identity.validate()?;
        let handle = self.handle(provider)?;
        let url = handle
            .entry
            .url(
                "recovery_response",
                &[("response_id", &identity.response_id)],
            )
            .map_err(|error| ProviderError::InvalidRequest(error.to_string()))?;
        let _permit = acquire(&handle.inflight).await?;
        let timeout = self.request_timeout.min(MAX_RECOVERY_TIME);
        tokio::time::timeout(timeout, async {
            let response = handle
                .client
                .get(url)
                .bearer_auth(agent_key.as_str())
                .header(reqwest::header::ACCEPT, "application/json")
                .timeout(timeout)
                .send()
                .await
                .map_err(transport_error)?;
            let response = checked(response).await?;
            let bytes = bounded_body(response, MAX_SNAPSHOT_BYTES).await?;
            let value = serde_json::from_slice(&bytes)
                .map_err(|_| malformed("invalid response snapshot JSON"))?;
            identity.snapshot(value)
        })
        .await
        .map_err(|_| timeout_error())?
    }

    /// Replay only an existing response. Every request reauthenticates with the
    /// supplied fresh key; no stored credential or management token is substituted.
    pub async fn replay_response(
        &self,
        provider: &ProviderSlug,
        identity: &RecoveryIdentity,
        starting_after: Option<u64>,
        agent_key: &Secret,
    ) -> Result<RecoveryStream, ProviderError> {
        identity.validate()?;
        if starting_after == Some(u64::MAX) {
            return Err(ProviderError::InvalidRequest(
                "replay cursor cannot advance".into(),
            ));
        }
        let handle = self.handle(provider)?;
        let mut url = handle
            .entry
            .url("recovery_events", &[("response_id", &identity.response_id)])
            .map_err(|error| ProviderError::InvalidRequest(error.to_string()))?;
        if let Some(cursor) = starting_after {
            url.query_pairs_mut()
                .append_pair("starting_after", &cursor.to_string());
        }
        let permit = acquire(&handle.inflight).await?;
        let timeout = self.request_timeout.min(MAX_RECOVERY_TIME);
        let deadline = tokio::time::Instant::now() + timeout;
        let response = tokio::time::timeout_at(deadline, async {
            let response = handle
                .client
                .get(url)
                .bearer_auth(agent_key.as_str())
                .header(reqwest::header::ACCEPT, "text/event-stream")
                .timeout(timeout)
                .send()
                .await
                .map_err(transport_error)?;
            checked(response).await
        })
        .await
        .map_err(|_| timeout_error())??;
        let headers = response.headers();
        if headers.get_all("x-cma-response-id").iter().count() != 1
            || headers
                .get("x-cma-response-id")
                .and_then(|value| value.to_str().ok())
                != Some(identity.response_id.as_str())
        {
            return Err(malformed("replay response identity header mismatch"));
        }
        if headers
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .map(str::trim)
            != Some("text/event-stream")
        {
            return Err(malformed("replay response is not an event stream"));
        }
        let (sender, events) = mpsc::channel(16);
        tokio::spawn(stream::forward(
            response,
            identity.clone(),
            starting_after,
            sender,
            permit,
            deadline,
        ));
        Ok(RecoveryStream { events })
    }
}

impl RecoveryIdentity {
    fn validate(&self) -> Result<(), ProviderError> {
        if !opaque_id(&self.response_id)
            || self
                .native_agent_id
                .as_ref()
                .is_some_and(|value| !opaque_id(value))
        {
            return Err(ProviderError::InvalidRequest(
                "invalid recovery identity".into(),
            ));
        }
        validate_metadata(&self.metadata)
    }

    fn snapshot(&self, response: Value) -> Result<RecoverySnapshot, ProviderError> {
        if response["id"].as_str() != Some(self.response_id.as_str())
            || response["object"] != "response"
            || !matches!(
                response["status"].as_str(),
                Some("queued" | "in_progress" | "completed" | "failed" | "incomplete")
            )
            || !response["output"].is_array()
        {
            return Err(malformed("response snapshot identity or shape mismatch"));
        }
        let native_agent_id = response
            .pointer("/conversation/id")
            .and_then(Value::as_str)
            .filter(|value| opaque_id(value))
            .ok_or_else(|| malformed("response has no native Agent identity"))?;
        if self
            .native_agent_id
            .as_ref()
            .is_some_and(|expected| expected != native_agent_id)
        {
            return Err(malformed("response native Agent identity mismatch"));
        }
        let metadata: BTreeMap<String, String> =
            serde_json::from_value(response["metadata"].clone())
                .map_err(|_| malformed("invalid response metadata"))?;
        if metadata != self.metadata {
            return Err(malformed(
                "response metadata does not match the accepted command",
            ));
        }
        Ok(RecoverySnapshot {
            native_agent_id: native_agent_id.to_owned(),
            response,
        })
    }
}

fn opaque_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && !matches!(value, "." | "..")
        && !value.bytes().any(|byte| byte.is_ascii_control())
}
fn malformed(detail: &str) -> ProviderError {
    ProviderError::ResponseMalformed(detail.into())
}
fn timeout_error() -> ProviderError {
    ProviderError::Unavailable("provider recovery deadline reached".into())
}

async fn bounded_body(
    mut response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, ProviderError> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(malformed("provider recovery body exceeds byte limit"));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(transport_error)? {
        if bytes.len().saturating_add(chunk.len()) > limit {
            return Err(malformed("provider recovery body exceeds byte limit"));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

async fn checked(response: reqwest::Response) -> Result<reqwest::Response, ProviderError> {
    let status = response.status();
    if status == reqwest::StatusCode::OK {
        return Ok(response);
    }
    let retry_after = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_secs);
    let body = bounded_body(response, MAX_ERROR_BYTES).await?;
    let code = serde_json::from_slice::<Value>(&body)
        .ok()
        .and_then(|value| {
            value
                .get("code")
                .or_else(|| value.pointer("/error/code"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .filter(|code| {
            code.len() <= 128
                && code
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        })
        .unwrap_or_else(|| "provider_error".into());
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(ProviderError::Busy { retry_after });
    }
    Err(ProviderError::Rejected {
        status: status.as_u16(),
        code,
    })
}

//! Direct, bounded submission of an already frozen request. No detached start,
//! input reconstruction or automatic retry is allowed at this boundary.
use super::{client::acquire, parse_sse, ProviderClient, ProviderError, RecoveryIdentity};
use cmaeg_core::{
    channel::Secret,
    ids::{IdempotencyKey, ProviderSlug},
};
use futures_util::StreamExt;
use serde_json::Value;
use std::{collections::BTreeMap, time::Duration};

pub(crate) enum FrozenSubmission {
    Accepted(String),
    NotAccepted,
    NotAcceptedWithCode(String),
    Rejected(String),
    Unknown,
}
impl ProviderClient {
    pub(crate) async fn submit_frozen(
        &self,
        provider: &ProviderSlug,
        bytes: Vec<u8>,
        idempotency: &IdempotencyKey,
        metadata: &BTreeMap<String, String>,
        credential: &Secret,
        telemetry: (bool, Option<crate::telemetry::timing::IngressTiming>),
    ) -> FrozenSubmission {
        let (nyxid_trace, timing) = telemetry;
        if self.record_worker_key(credential).await.is_err() {
            return FrozenSubmission::NotAccepted;
        }
        let Ok(handle) = self.handle(provider) else {
            return FrozenSubmission::Rejected("provider_not_configured".into());
        };
        if handle.entry.paths.recovery.is_none() {
            return FrozenSubmission::Rejected("provider_recovery_required".into());
        }
        if bytes.len() > 1024 * 1024 {
            return FrozenSubmission::Rejected("provider_body_too_large".into());
        }
        let Ok(url) = handle.entry.url("responses", &[]) else {
            return FrozenSubmission::Rejected("provider_path_invalid".into());
        };
        let Ok(_permit) = acquire(&handle.inflight).await else {
            return FrozenSubmission::NotAccepted;
        };
        // Any timeout after entering the transport is uncertain, even if no
        // response headers arrived. Cancellation drops this awaited future.
        let deadline = self.request_timeout.min(Duration::from_secs(20));
        tokio::time::timeout(deadline, async {
            let mut trace = crate::telemetry::turn_trace::ProviderReader::new(
                self.turn_trace_enabled,
                provider.as_str() == "nyxbot" && nyxid_trace,
                None,
            );
            if let Some(trace) = &mut trace {
                trace.begin_source();
            }
            let observed = match handle
                .responses
                .send_with_trace(
                    url,
                    credential.as_str(),
                    idempotency.as_str(),
                    bytes,
                    timing,
                    self.turn_trace_enabled && provider.as_str() == "nyxbot" && nyxid_trace,
                )
                .await
            {
                Ok(response) => response,
                Err(_) => return FrozenSubmission::Unknown,
            };
            if let Some(trace) = &mut trace {
                trace.response(
                    observed.response.headers(),
                    observed.response.status().as_u16(),
                );
            }
            if observed.response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
                return if conversation_not_ready(observed.response).await {
                    FrozenSubmission::NotAcceptedWithCode("conversation_not_ready".into())
                } else {
                    FrozenSubmission::Unknown
                };
            }
            if matches!(observed.response.status().as_u16(), 403 | 422 | 503) {
                return sender_context_refusal(observed.response).await;
            }
            if observed.response.status() == reqwest::StatusCode::BAD_REQUEST {
                return bad_request(observed.response).await;
            }
            let response = match super::recovery::checked(observed.response).await {
                Ok(response) => response,
                Err(ProviderError::Busy { .. }) => return FrozenSubmission::Unknown,
                Err(_) => return FrozenSubmission::Unknown,
            };
            let Some(response_id) = response
                .headers()
                .get("x-cma-response-id")
                .and_then(|header| header.to_str().ok())
                .map(str::to_owned)
            else {
                return FrozenSubmission::Unknown;
            };
            if response
                .headers()
                .get_all("x-cma-response-id")
                .iter()
                .count()
                != 1
                || response
                    .headers()
                    .get(reqwest::header::CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok())
                    .and_then(|value| value.split(';').next())
                    .map(str::trim)
                    != Some("text/event-stream")
            {
                return FrozenSubmission::Unknown;
            }
            let identity = RecoveryIdentity {
                response_id: response_id.clone(),
                native_agent_id: None,
                metadata: metadata.clone(),
            };
            let mut total = 0usize;
            let wire = response.bytes_stream().map(move |chunk| {
                let chunk = chunk.map_err(|_| "transport")?;
                total = total.saturating_add(chunk.len());
                if total > 1024 * 1024 {
                    return Err("acceptance prefix exceeds byte limit");
                }
                Ok(chunk)
            });
            let mut frames = Box::pin(parse_sse(wire));
            let Some(Ok(frame)) = frames.next().await else {
                return FrozenSubmission::Unknown;
            };
            let Ok(value) = serde_json::from_str::<Value>(&frame.data) else {
                return FrozenSubmission::Unknown;
            };
            if frame.id.as_deref() != Some("0")
                || frame
                    .event
                    .as_deref()
                    .is_some_and(|event| event != "response.created")
                || value["type"] != "response.created"
                || value["sequence_number"].as_u64() != Some(0)
                || value["response"]["status"] != "queued"
                || identity.snapshot(value["response"].clone()).is_err()
            {
                return FrozenSubmission::Unknown;
            }
            if let Some(trace) = &mut trace {
                trace.event(&value);
            }
            FrozenSubmission::Accepted(response_id)
        })
        .await
        .unwrap_or(FrozenSubmission::Unknown)
    }
}

/// A 400 proves non-admission only in a documented pre-admission dialect: CMA
/// v2 `invalid_json`, or the NyxID provider's `validation_error` (code 1008),
/// which it answers before accepting an event. Anything else stays unknown.
async fn bad_request(response: reqwest::Response) -> FrozenSubmission {
    let unadmitted = !response.headers().contains_key("x-cma-response-id");
    let json = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.split(';').next())
        .map(str::trim)
        == Some("application/json");
    let Ok(body) = super::recovery::bounded_body(response, 64 * 1024).await else {
        return FrozenSubmission::Unknown;
    };
    let Ok(value) = serde_json::from_slice::<Value>(&body) else {
        return FrozenSubmission::Unknown;
    };
    // The existing CMA v2 refusal, matched as before at either code location.
    let code = value.get("code").or_else(|| value.pointer("/error/code"));
    if code.and_then(Value::as_str) == Some("invalid_json") {
        return FrozenSubmission::Rejected("invalid_json".into());
    }
    if unadmitted
        && json
        && value["error"] == "validation_error"
        && value["error_code"] == 1008
        && value["message"].is_string()
    {
        return FrozenSubmission::Rejected("provider_validation_refused".into());
    }
    FrozenSubmission::Unknown
}

/// Only the documented CMA pre-admission dialect proves that a turn did not start.
async fn sender_context_refusal(response: reqwest::Response) -> FrozenSubmission {
    let status = response.status().as_u16();
    if response.headers().contains_key("x-cma-response-id")
        || response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|h| h.to_str().ok())
            .and_then(|h| h.split(';').next())
            .map(str::trim)
            != Some("application/json")
    {
        return FrozenSubmission::Unknown;
    }
    let Ok(body) = super::recovery::bounded_body(response, 64 * 1024).await else {
        return FrozenSubmission::Unknown;
    };
    let Ok(value) = serde_json::from_slice::<Value>(&body) else {
        return FrozenSubmission::Unknown;
    };
    if value.as_object().is_none_or(|object| object.len() != 1)
        || value["error"]["type"]
            != if status == 503 {
                "server_error"
            } else {
                "invalid_request_error"
            }
    {
        return FrozenSubmission::Unknown;
    }
    match (status, value["error"]["code"].as_str()) {
        (403, Some("verified_sender_context_invalid"))
        | (422, Some("verified_sender_context_unsupported")) => {
            FrozenSubmission::Rejected(value["error"]["code"].as_str().unwrap().into())
        }
        (503, Some("verified_sender_context_unavailable")) => {
            FrozenSubmission::NotAcceptedWithCode("verified_sender_context_unavailable".into())
        }
        _ => FrozenSubmission::Unknown,
    }
}

/// CMA v2 resolves a provisioning conversation before commands.admit. Only its
/// complete, precise refusal proves that this POST did not admit a turn. The
/// dispatcher retains the frozen body/key and schedules a bounded retry; this
/// transport boundary must never retry a POST itself.
async fn conversation_not_ready(response: reqwest::Response) -> bool {
    if response.headers().contains_key("x-cma-response-id")
        || response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .map(str::trim)
            != Some("application/json")
    {
        return false;
    }
    let Ok(body) = super::recovery::bounded_body(response, 64 * 1024).await else {
        return false;
    };
    let Ok(value) = serde_json::from_slice::<Value>(&body) else {
        return false;
    };
    value.as_object().is_some_and(|object| object.len() == 1)
        && value["error"]["type"] == "invalid_request_error"
        && value["error"]["code"] == "conversation_not_ready"
}

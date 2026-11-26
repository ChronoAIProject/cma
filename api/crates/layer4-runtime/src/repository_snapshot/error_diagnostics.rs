use futures_util::StreamExt;
use serde::Deserialize;
use std::time::Duration;

const MAX_BYTES: usize = 8 * 1024;
const READ_BUDGET: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ResponseHint {
    Unknown,
    ClaimedApiKeyScopeForbidden,
}

impl ResponseHint {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::ClaimedApiKeyScopeForbidden => "claimed_api_key_scope_forbidden",
        }
    }
}

#[derive(Deserialize)]
struct ErrorClaim {
    error: String,
    error_code: u32,
}

// The response can describe a refusal; it cannot prove its origin or change
// authorization, retry, or cleanup. Only this fixed hint leaves the parser.
pub(super) async fn read_hint(response: reqwest::Response) -> ResponseHint {
    if response.status() != reqwest::StatusCode::FORBIDDEN
        || !response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .is_some_and(|media_type| media_type.trim().eq_ignore_ascii_case("application/json"))
        || response
            .content_length()
            .is_some_and(|size| size > MAX_BYTES as u64)
    {
        return ResponseHint::Unknown;
    }
    // The response stream retains reqwest's original request deadline. This
    // extra budget may shorten that deadline, but cannot extend it.
    tokio::time::timeout(READ_BUDGET, async move {
        let mut body = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let Ok(chunk) = chunk else {
                return ResponseHint::Unknown;
            };
            if body.len().saturating_add(chunk.len()) > MAX_BYTES {
                return ResponseHint::Unknown;
            }
            body.extend_from_slice(&chunk);
        }
        match serde_json::from_slice::<ErrorClaim>(&body) {
            Ok(claim) if claim.error == "api_key_scope_forbidden" && claim.error_code == 9000 => {
                ResponseHint::ClaimedApiKeyScopeForbidden
            }
            _ => ResponseHint::Unknown,
        }
    })
    .await
    .unwrap_or(ResponseHint::Unknown)
}

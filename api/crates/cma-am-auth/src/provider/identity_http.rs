//! Complete, bounded provider evidence with phase-aware timeout classification.

use reqwest::{header, RequestBuilder, StatusCode};
use serde_json::Value;
use tokio::time::{timeout_at, Instant};

use super::{
    provider_http,
    verification_diagnostics::{record, Endpoint, Reason, Stage},
    verification_error::ProviderVerificationError as Error,
};

pub const MAX_BYTES: usize = 64 * 1024;
// Login/worker paths outside the retry sequence retain the HTTP client's budget.
pub const DIRECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

pub async fn json(
    request: RequestBuilder,
    deadline: Instant,
    endpoint: Endpoint,
) -> Result<Value, Error> {
    // Only a timeout before response headers is retryable. Once headers
    // arrive, the body must complete and validate within the same attempt budget.
    let mut call = super::identity_observations::Call::start(endpoint);
    let response = timeout_at(deadline, request.send())
        .await
        .map_err(|_| Error::Transient("timeout"))
        .and_then(|response| response.map_err(Error::transport))
        .inspect_err(|error| {
            let reason = match error {
                Error::Transient("timeout") => Reason::Timeout,
                Error::Transient("connection") => Reason::Connection,
                _ => Reason::Transport,
            };
            call.finish(None);
            record(endpoint, Stage::ResponseHeaders, reason, None, error);
        })?;
    let status = response.status();
    call.finish(Some(status.as_u16()));
    if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
        let error = Error::status(status);
        record(
            endpoint,
            Stage::Status,
            Reason::HttpStatus,
            Some(status.as_u16()),
            &error,
        );
        return Err(error);
    }
    // A throttle is not successful identity evidence. Legal advisory headers
    // must not hide its backoff or turn it into an invalid-evidence refusal.
    if status == StatusCode::TOO_MANY_REQUESTS {
        let error = Error::RateLimited(super::identity_throttle::RetryAfter::from_headers(
            response.headers(),
        ));
        record(
            endpoint,
            Stage::Status,
            Reason::HttpStatus,
            Some(status.as_u16()),
            &error,
        );
        return Err(error);
    }
    if response.headers().contains_key(header::CONTENT_RANGE)
        || response.headers().contains_key(header::LINK)
        || response
            .headers()
            .get(header::CONTENT_TYPE)
            .is_some_and(|value| {
                value
                    .to_str()
                    .ok()
                    .is_none_or(|value| value.trim().to_ascii_lowercase().starts_with("multipart/"))
            })
    {
        let error = Error::Unavailable("partial identity provider response".into());
        record(
            endpoint,
            Stage::ResponseHeaders,
            Reason::PartialResponse,
            Some(status.as_u16()),
            &error,
        );
        return Err(error);
    }
    if response.status() != StatusCode::OK {
        let error = Error::status(status);
        record(
            endpoint,
            Stage::Status,
            Reason::HttpStatus,
            Some(status.as_u16()),
            &error,
        );
        return Err(error);
    }
    let body: EvidenceBody =
        timeout_at(deadline, provider_http::read_evidence(response, MAX_BYTES))
            .await
            .map_err(|_| {
                let error = Error::Unavailable("identity provider body did not complete".into());
                record(
                    endpoint,
                    Stage::Body,
                    Reason::Timeout,
                    Some(status.as_u16()),
                    &error,
                );
                error
            })?
            .map_err(|failure| {
                let error = Error::Unavailable("invalid identity provider body".into());
                let reason = match failure {
                    provider_http::EvidenceReadError::Timeout => Reason::Timeout,
                    provider_http::EvidenceReadError::Read => Reason::BodyRead,
                    provider_http::EvidenceReadError::TooLarge => Reason::BodyTooLarge,
                    provider_http::EvidenceReadError::Partial => Reason::PartialResponse,
                    provider_http::EvidenceReadError::InvalidJson
                    | provider_http::EvidenceReadError::Refused(_) => Reason::InvalidBody,
                };
                record(endpoint, Stage::Body, reason, Some(status.as_u16()), &error);
                error
            })?;
    require_success(&body.0).inspect_err(|error| {
        record(
            endpoint,
            Stage::Claims,
            Reason::ErrorDisposition,
            Some(status.as_u16()),
            error,
        );
    })?;
    Ok(body.0)
}

struct EvidenceBody(Value);

impl<'de> serde::Deserialize<'de> for EvidenceBody {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct UniqueClaims;
        impl<'de> serde::de::Visitor<'de> for UniqueClaims {
            type Value = EvidenceBody;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("an identity object with unique fields")
            }

            fn visit_map<M: serde::de::MapAccess<'de>>(
                self,
                mut map: M,
            ) -> Result<Self::Value, M::Error> {
                let mut claims = serde_json::Map::new();
                while let Some((key, value)) = map.next_entry::<String, Value>()? {
                    if claims.insert(key, value).is_some() {
                        return Err(serde::de::Error::custom("duplicate identity field"));
                    }
                }
                Ok(EvidenceBody(Value::Object(claims)))
            }
        }
        deserializer.deserialize_map(UniqueClaims)
    }
}

fn require_success(body: &Value) -> Result<(), Error> {
    let object = body
        .as_object()
        .ok_or_else(|| Error::Unavailable("identity provider body is not an object".into()))?;
    if ["error", "error_description", "error_uri", "errors"]
        .iter()
        .any(|field| {
            object.get(*field).is_some_and(|value| {
                !value.is_null() && !value.as_str().is_some_and(|text| text.trim().is_empty())
            })
        })
    {
        return Err(match object.get("error").and_then(Value::as_str) {
            Some(
                "invalid_token" | "invalid_grant" | "revoked_token" | "token_revoked"
                | "expired_token" | "token_expired" | "access_denied",
            ) => Error::Rejected("identity provider explicitly refused credential".into()),
            _ => Error::Unavailable("identity provider returned an error disposition".into()),
        });
    }
    Ok(())
}

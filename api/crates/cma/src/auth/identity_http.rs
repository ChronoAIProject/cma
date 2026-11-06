//! Complete, bounded provider evidence with phase-aware timeout classification.

use reqwest::{header, RequestBuilder, StatusCode};
use serde_json::Value;
use tokio::time::{timeout_at, Instant};

use super::{provider_http, verification_error::ProviderVerificationError as Error};

pub(super) const MAX_BYTES: usize = 64 * 1024;
// Login/worker paths outside the retry sequence retain the HTTP client's budget.
pub(super) const DIRECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

pub(super) async fn json(request: RequestBuilder, deadline: Instant) -> Result<Value, Error> {
    // Only a timeout before response headers is fallback-eligible. Once headers
    // arrive, the body must complete and validate within the same attempt budget.
    let response = timeout_at(deadline, request.send())
        .await
        .map_err(|_| Error::Transient("timeout"))?
        .map_err(Error::transport)?;
    if matches!(
        response.status(),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
    ) {
        return Err(Error::status(response.status()));
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
        return Err(Error::Unavailable(
            "partial identity provider response".into(),
        ));
    }
    if response.status() != StatusCode::OK {
        return Err(Error::status(response.status()));
    }
    let body: EvidenceBody = timeout_at(
        deadline,
        provider_http::read_json_limited(response, MAX_BYTES),
    )
    .await
    .map_err(|_| Error::Unavailable("identity provider body did not complete".into()))?
    .map_err(|_| Error::Unavailable("invalid identity provider body".into()))?;
    require_success(&body.0)?;
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

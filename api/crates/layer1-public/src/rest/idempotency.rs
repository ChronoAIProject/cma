//! Idempotent execution for the two side-effecting REST operations that opt in.

use std::{sync::Arc, time::Duration};

use axum::{
    body::{to_bytes, Body, Bytes},
    extract::{FromRequest, FromRequestParts, Request},
    http::{header, request::Parts, HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use layer2_core::{Caller, OperationClaim, OperationReservation, OperationStore, StoreError};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{problem_document, problem_response, BodyProblem, RequestFailure};

const IDEMPOTENCY_KEY: &str = "idempotency-key";
const MAX_KEY_BYTES: usize = 255;
pub(super) const RETENTION: Duration = Duration::from_secs(24 * 60 * 60);

/// A JSON value accompanied by the digest of the bytes that carried it.
pub(super) struct RequestBody<T> {
    value: T,
    fingerprint: [u8; 32],
}

impl<T> RequestBody<T> {
    pub(super) fn into_parts(self) -> (T, [u8; 32]) {
        (self.value, self.fingerprint)
    }
}

impl<T, S> FromRequest<S> for RequestBody<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = BodyProblem;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        if !is_json_content_type(request.headers()) {
            return Err(BodyProblem::missing_json_content_type());
        }
        let bytes = Bytes::from_request(request, state).await.map_err(|error| {
            BodyProblem::from(axum::extract::rejection::JsonRejection::from(error))
        })?;
        let Json(value) = Json::<T>::from_bytes(&bytes).map_err(BodyProblem::from)?;
        Ok(Self {
            value,
            fingerprint: Sha256::digest(&bytes).into(),
        })
    }
}

fn is_json_content_type(headers: &HeaderMap) -> bool {
    let Some(content_type) = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<mime::Mime>().ok())
    else {
        return false;
    };
    content_type.type_() == mime::APPLICATION
        && (content_type.subtype() == mime::JSON || content_type.suffix() == Some(mime::JSON))
}

/// A validated optional operation key.
pub(super) struct IdempotencyKey(Option<String>);

impl<S> FromRequestParts<S> for IdempotencyKey
where
    S: Send + Sync,
{
    type Rejection = IdempotencyProblem;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let mut values = parts.headers.get_all(IDEMPOTENCY_KEY).iter();
        let Some(value) = values.next() else {
            return Ok(Self(None));
        };
        if values.next().is_some() {
            return Err(IdempotencyProblem::Invalid);
        }
        let bytes = value.as_bytes();
        if bytes.is_empty()
            || bytes.len() > MAX_KEY_BYTES
            || !bytes.iter().all(|byte| (0x21..=0x7e).contains(byte))
        {
            return Err(IdempotencyProblem::Invalid);
        }
        let key = std::str::from_utf8(bytes)
            .map_err(|_| IdempotencyProblem::Invalid)?
            .to_owned();
        Ok(Self(Some(key)))
    }
}

/// A refusal specific to operation-key handling.
pub enum IdempotencyProblem {
    Invalid,
    Pending,
    Reused,
}

impl IntoResponse for IdempotencyProblem {
    fn into_response(self) -> Response {
        let (status, slug, code, detail) = match self {
            Self::Invalid => (
                StatusCode::BAD_REQUEST,
                "invalid-idempotency-key",
                "invalid_idempotency_key",
                "Idempotency-Key must contain between 1 and 255 visible ASCII bytes.",
            ),
            Self::Pending => (
                StatusCode::CONFLICT,
                "idempotency-in-flight",
                "idempotency_in_flight",
                "An operation with this Idempotency-Key is still in progress. Retry shortly.",
            ),
            Self::Reused => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "idempotency-key-reused",
                "idempotency_key_reused",
                "This Idempotency-Key already names a different request.",
            ),
        };
        let body = problem_document(status, slug, detail, code);
        problem_response(status, body)
    }
}

/// The result of inspecting an optional operation key.
pub(super) enum Idempotency {
    Unkeyed,
    Acquired {
        store: Arc<dyn OperationStore>,
        claim: OperationClaim,
    },
    Replay(Response),
}

impl Idempotency {
    /// Reserve a key, identify a duplicate, or leave an unkeyed request alone.
    pub(super) async fn begin(
        key: IdempotencyKey,
        store: Arc<dyn OperationStore>,
        caller: &Caller,
        operation: impl Into<String>,
        fingerprint: [u8; 32],
    ) -> Result<Self, Response> {
        let Some(key) = key.0 else {
            return Ok(Self::Unkeyed);
        };
        let claim = OperationClaim::new(
            caller.subject.clone(),
            operation,
            key,
            fingerprint,
            std::time::SystemTime::now() + RETENTION,
        );
        let reservation = store
            .reserve(&claim)
            .await
            .map_err(|error| RequestFailure::from(error).into_response())?;
        Ok(match reservation {
            OperationReservation::Acquired => Self::Acquired { store, claim },
            OperationReservation::Pending => {
                return Err(IdempotencyProblem::Pending.into_response())
            }
            OperationReservation::FingerprintMismatch => {
                return Err(IdempotencyProblem::Reused.into_response())
            }
            OperationReservation::Complete(outcome) => Self::Replay(
                decode_response(&outcome)
                    .map_err(|error| RequestFailure::from(error).into_response())?,
            ),
        })
    }

    /// Record the result of an acquired operation and return it to the caller.
    pub(super) async fn finish(self, result: Result<Response, RequestFailure>) -> Response {
        let response = result.unwrap_or_else(IntoResponse::into_response);
        let Self::Acquired { store, claim } = self else {
            return response;
        };
        let (response, outcome) = match encode_response(response).await {
            Ok(recorded) => recorded,
            Err(error) => return RequestFailure::from(error).into_response(),
        };
        if let Err(error) = store.complete(&claim, outcome).await {
            return RequestFailure::from(error).into_response();
        }
        response
    }
}

#[derive(Serialize, Deserialize)]
struct RecordedResponse {
    status: u16,
    headers: Vec<(String, Vec<u8>)>,
    body: Vec<u8>,
}

async fn encode_response(response: Response) -> Result<(Response, Vec<u8>), StoreError> {
    let (parts, body) = response.into_parts();
    let body = to_bytes(body, usize::MAX)
        .await
        .map_err(|error| StoreError::Malformed(format!("operation outcome body: {error}")))?;
    let recorded = RecordedResponse {
        status: parts.status.as_u16(),
        headers: parts
            .headers
            .iter()
            .map(|(name, value)| (name.as_str().to_owned(), value.as_bytes().to_vec()))
            .collect(),
        body: body.to_vec(),
    };
    let outcome = serde_json::to_vec(&recorded)
        .map_err(|error| StoreError::Malformed(format!("operation outcome encoding: {error}")))?;
    Ok((Response::from_parts(parts, Body::from(body)), outcome))
}

fn decode_response(outcome: &[u8]) -> Result<Response, StoreError> {
    let recorded: RecordedResponse = serde_json::from_slice(outcome)
        .map_err(|error| StoreError::Malformed(format!("stored operation outcome: {error}")))?;
    let status = StatusCode::from_u16(recorded.status)
        .map_err(|error| StoreError::Malformed(format!("stored operation status: {error}")))?;
    let mut response = Response::builder().status(status);
    for (name, value) in recorded.headers {
        let name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|error| StoreError::Malformed(format!("stored operation header: {error}")))?;
        let value = HeaderValue::from_bytes(&value).map_err(|error| {
            StoreError::Malformed(format!("stored operation header value: {error}"))
        })?;
        response = response.header(name, value);
    }
    response
        .body(Body::from(recorded.body))
        .map_err(|error| StoreError::Malformed(format!("stored operation response: {error}")))
}

#[cfg(test)]
mod tests {
    use axum::http::Request;
    use layer2_core::{Authorization, SubjectId};

    use super::*;

    #[tokio::test]
    async fn keys_are_visible_ascii_and_bounded() {
        let longest = "x".repeat(MAX_KEY_BYTES);
        let oversized = "x".repeat(MAX_KEY_BYTES + 1);
        for (value, accepted) in [
            ("retry-1", true),
            ("", false),
            ("contains space", false),
            (longest.as_str(), true),
            (oversized.as_str(), false),
        ] {
            let request = Request::builder()
                .header(IDEMPOTENCY_KEY, value)
                .body(())
                .unwrap();
            let (mut parts, _) = request.into_parts();
            let result = IdempotencyKey::from_request_parts(&mut parts, &()).await;
            assert_eq!(result.is_ok(), accepted, "for a {} byte key", value.len());
        }

        let request = Request::builder()
            .header(IDEMPOTENCY_KEY, "first")
            .header(IDEMPOTENCY_KEY, "second")
            .body(())
            .unwrap();
        let (mut parts, _) = request.into_parts();
        assert!(matches!(
            IdempotencyKey::from_request_parts(&mut parts, &()).await,
            Err(IdempotencyProblem::Invalid)
        ));
    }

    #[tokio::test]
    async fn an_absent_key_never_reads_the_store() {
        struct Panics;
        #[async_trait::async_trait]
        impl OperationStore for Panics {
            async fn reserve(
                &self,
                _claim: &OperationClaim,
            ) -> Result<OperationReservation, StoreError> {
                panic!("an unkeyed request must not reserve")
            }

            async fn complete(
                &self,
                _claim: &OperationClaim,
                _outcome: Vec<u8>,
            ) -> Result<(), StoreError> {
                panic!("an unkeyed request must not complete")
            }
        }

        let caller = Caller::new(SubjectId("subject-a".into()), Authorization::none());
        let result = Idempotency::begin(
            IdempotencyKey(None),
            Arc::new(Panics),
            &caller,
            "create-resource",
            [0; 32],
        )
        .await;
        let Ok(begun) = result else {
            panic!("an unkeyed request is accepted");
        };
        assert!(matches!(begun, Idempotency::Unkeyed));
    }

    #[tokio::test]
    async fn response_encoding_preserves_status_headers_and_body() {
        let original = Response::builder()
            .status(StatusCode::CREATED)
            .header("content-type", "application/json")
            .body(Body::from(r#"{"id":"resource-a"}"#))
            .unwrap();
        let (_, encoded) = encode_response(original).await.unwrap();
        let replay = decode_response(&encoded).unwrap();
        assert_eq!(replay.status(), StatusCode::CREATED);
        assert_eq!(replay.headers()["content-type"], "application/json");
        assert_eq!(
            to_bytes(replay.into_body(), usize::MAX).await.unwrap(),
            r#"{"id":"resource-a"}"#
        );
    }
}

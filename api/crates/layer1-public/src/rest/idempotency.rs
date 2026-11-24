//! Idempotent execution for REST commands that opt in.

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
#[cfg(test)]
const MAX_KEY_BYTES: usize = 255;
pub(super) const RETENTION: Duration = Duration::from_secs(24 * 60 * 60);
const EXECUTION_LEASE: Duration = Duration::from_secs(2 * 60);

/// Encode resource boundaries without letting opaque IDs alter the operation namespace.
pub(super) fn operation_scope(operation: &str, resources: &[&str]) -> String {
    serde_json::to_string(&(operation, resources)).expect("operation scopes contain only strings")
}

/// A JSON decoding result accompanied by the digest of the bytes that carried it.
pub(super) struct RequestBody<T> {
    value: Result<T, BodyProblem>,
    fingerprint: [u8; 32],
    creation_digest: Option<String>,
}

impl<T> RequestBody<T> {
    pub(super) fn into_parts(self) -> (Result<T, BodyProblem>, [u8; 32]) {
        (self.value, self.fingerprint)
    }

    pub(super) fn value(&self) -> Option<&T> {
        self.value.as_ref().ok()
    }

    pub(super) fn creation_digest(&self) -> Option<&str> {
        self.creation_digest.as_deref()
    }
}

impl<T, S> FromRequest<S> for RequestBody<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = BodyProblem;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        let capture_creation = request
            .extensions()
            .get::<Arc<dyn super::creation_outcomes::Backend>>()
            .is_some()
            && matches!(request.uri().path(), "/api/v1/sandboxes" | "/api/v1/agents");
        let is_json = is_json_content_type(request.headers());
        // Unkeyed requests retain Axum's content-type-first rejection. A keyed
        // refusal still needs the raw bytes so its fingerprint can be replayed.
        if !is_json && !request.headers().contains_key(IDEMPOTENCY_KEY) {
            return Err(BodyProblem::missing_json_content_type());
        }
        let bytes = Bytes::from_request(request, state).await.map_err(|error| {
            BodyProblem::from(axum::extract::rejection::JsonRejection::from(error))
        })?;
        let value = if is_json {
            Json::<T>::from_bytes(&bytes)
                .map(|Json(value)| value)
                .map_err(BodyProblem::from)
        } else {
            Err(BodyProblem::missing_json_content_type())
        };
        Ok(Self {
            creation_digest: capture_creation
                .then(|| super::creation_outcomes::wire_digest(&bytes)),
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

impl IdempotencyKey {
    pub(super) fn value(&self) -> Option<&str> {
        self.0.as_deref()
    }
    /// Durable domain ledgers own their receipts and retain keys permanently.
    pub(super) fn into_required_value(self) -> Result<String, IdempotencyProblem> {
        self.0.ok_or(IdempotencyProblem::Missing)
    }

    pub(super) fn required(self) -> Result<Self, IdempotencyProblem> {
        if self.0.is_some() {
            Ok(self)
        } else {
            Err(IdempotencyProblem::Missing)
        }
    }

    /// Stable evidence for a confirmation record without retaining the HTTP
    /// operation key itself.
    pub(super) fn confirmation_hash(&self, domain: &str) -> Option<String> {
        self.0.as_ref().map(|key| {
            let mut digest = Sha256::new();
            digest.update(domain.as_bytes());
            digest.update([0]);
            digest.update(key.as_bytes());
            let digest = digest.finalize();
            digest.iter().map(|byte| format!("{byte:02x}")).collect()
        })
    }
}

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
        if !layer2_core::operation::valid_idempotency_key(bytes) {
            return Err(IdempotencyProblem::Invalid);
        }
        let key = std::str::from_utf8(bytes)
            .map_err(|_| IdempotencyProblem::Invalid)?
            .to_owned();
        if super::creation_outcomes::is_covered_key(&key)
            && (parts.method != axum::http::Method::POST
                || !matches!(parts.uri.path(), "/api/v1/sandboxes" | "/api/v1/agents")
                || parts.uri.query().is_some()
                || parts
                    .extensions
                    .get::<Arc<dyn super::creation_outcomes::Backend>>()
                    .is_none())
        {
            return Err(IdempotencyProblem::Invalid);
        }
        Ok(Self(Some(key)))
    }
}

/// A refusal specific to operation-key handling.
pub enum IdempotencyProblem {
    Missing,
    Invalid,
    Pending,
    RecoveryRequired,
    Reused,
}

impl IntoResponse for IdempotencyProblem {
    fn into_response(self) -> Response {
        let (status, slug, code, detail) = match self {
            Self::Missing => (
                StatusCode::BAD_REQUEST,
                "missing-idempotency-key",
                "missing_idempotency_key",
                "This operation requires an Idempotency-Key header.",
            ),
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
            Self::RecoveryRequired => (
                StatusCode::CONFLICT,
                "idempotency-recovery-required",
                "idempotency_recovery_required",
                "The previous execution ended without a replay receipt. This Idempotency-Key remains fenced unless the original execution records its exact result; manual review of durable state is required.",
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
    /// A newer domain ledger may prove acceptance without repeating execution.
    /// Generic legacy commands continue returning their original refusal.
    Fenced(IdempotencyProblem),
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
        match Self::begin_recoverable(key, store, caller, operation, fingerprint).await? {
            Self::Fenced(problem) => Err(problem.into_response()),
            other => Ok(other),
        }
    }

    /// Inspect the existing outer receipt before consulting an additive ledger.
    /// Pending/recovery never grants permission to run the command again.
    pub(super) async fn begin_recoverable(
        key: IdempotencyKey,
        store: Arc<dyn OperationStore>,
        caller: &Caller,
        operation: impl Into<String>,
        fingerprint: [u8; 32],
    ) -> Result<Self, Response> {
        let Some(key) = key.0 else {
            return Ok(Self::Unkeyed);
        };
        let now = std::time::SystemTime::now();
        let claim = OperationClaim::new(
            caller.subject.clone(),
            operation,
            key,
            fingerprint,
            now + EXECUTION_LEASE,
            RETENTION,
        );
        let reservation = store
            .reserve(&claim)
            .await
            .map_err(|error| RequestFailure::from(error).into_response())?;
        Ok(match reservation {
            OperationReservation::Acquired => Self::Acquired { store, claim },
            OperationReservation::Pending => Self::Fenced(IdempotencyProblem::Pending),
            OperationReservation::RecoveryRequired => {
                Self::Fenced(IdempotencyProblem::RecoveryRequired)
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
    pub(super) async fn finish<E>(self, result: Result<Response, E>) -> Response
    where
        E: IntoResponse,
    {
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

/// Begin one required, body-fingerprinted command and derive confirmation
/// evidence from its operation key without retaining the key itself.
pub(super) async fn required_command<T: DeserializeOwned>(
    key: IdempotencyKey,
    store: Arc<dyn OperationStore>,
    caller: &Caller,
    operation: String,
    body: Result<RequestBody<T>, BodyProblem>,
) -> Result<(Idempotency, Result<T, BodyProblem>, String), Response> {
    let key = key.required().map_err(IntoResponse::into_response)?;
    let confirmation_hash = key
        .confirmation_hash(&operation)
        .expect("a required operation key has a digest");
    let (body, fingerprint) = match body {
        Ok(body) => body.into_parts(),
        Err(error) => return Err(RequestFailure::from(error).into_response()),
    };
    let guard = match Idempotency::begin(key, store, caller, operation, fingerprint).await {
        Ok(Idempotency::Replay(response)) => return Err(response),
        Ok(guard) => guard,
        Err(response) => return Err(response),
    };
    Ok((guard, body, confirmation_hash))
}

/// Begin a required command whose route deliberately has no request body.
pub(super) async fn required_empty_command(
    key: IdempotencyKey,
    store: Arc<dyn OperationStore>,
    caller: &Caller,
    operation: String,
) -> Result<Idempotency, Response> {
    let key = key.required().map_err(IntoResponse::into_response)?;
    let fingerprint = Sha256::digest([]).into();
    match Idempotency::begin(key, store, caller, operation, fingerprint).await {
        Ok(Idempotency::Replay(response)) => Err(response),
        Ok(guard) => Ok(guard),
        Err(response) => Err(response),
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

pub(super) fn decode_response(outcome: &[u8]) -> Result<Response, StoreError> {
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
mod tests;

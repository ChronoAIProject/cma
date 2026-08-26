//! Durable coordination for AG-UI creation aliases and run admission.
//!
//! An SSE response is open-ended and cannot itself be stored as an operation
//! outcome. The finite facts that precede it can: which core-minted session a
//! requested thread became, and which durable boundary admitted one run. This
//! module stores only those facts and leaves streaming to `run.rs`.

use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};

use axum::{
    body::to_bytes,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use layer2_core::{
    Caller, CoreError, OperationClaim, OperationReservation, OperationStore, StoreError,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    auth::StoreUnavailable,
    rest::{problem_document, problem_response, Problem},
};

const OUTCOME_VERSION: u8 = 1;
const CLAIM_RETENTION: Duration = Duration::from_secs(24 * 60 * 60);
const PENDING_POLL_INTERVAL: Duration = Duration::from_millis(50);
const PENDING_WAIT: Duration = Duration::from_secs(2);
const FINGERPRINT_DOMAIN: &[u8] = b"cma-agui-operation-fingerprint-v1\0";

/// A finite problem document that can be replayed from an operation outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct StoredProblem {
    status: u16,
    body: serde_json::Value,
}

impl StoredProblem {
    /// Render and retain one typed core refusal.
    pub(super) async fn from_core(error: CoreError) -> Result<Self, RunFailure> {
        Self::from_response(Problem(error).into_response()).await
    }

    /// Provisioning exhausted the Layer 1 wait after the stream disclosed its id.
    pub(super) fn provisioning_timeout() -> Self {
        let status = StatusCode::SERVICE_UNAVAILABLE;
        Self {
            status: status.as_u16(),
            body: problem_document(
                status,
                "provisioning-timeout",
                "The session did not become ready for conversation before the provisioning deadline.",
                "provisioning_timeout",
            ),
        }
    }

    /// The owned task stopped without producing an admission result.
    pub(super) fn task_failed() -> Self {
        let status = StatusCode::SERVICE_UNAVAILABLE;
        Self {
            status: status.as_u16(),
            body: problem_document(
                status,
                "run-admission-failed",
                "The run admission task stopped before it could report an outcome.",
                "run_admission_failed",
            ),
        }
    }

    /// Session creation stopped before publishing its core-minted identifier.
    pub(super) fn creation_task_failed() -> Self {
        let status = StatusCode::SERVICE_UNAVAILABLE;
        Self {
            status: status.as_u16(),
            body: problem_document(
                status,
                "thread-creation-failed",
                "The thread creation task stopped before it could report an outcome.",
                "thread_creation_failed",
            ),
        }
    }

    /// The finite result could not be published for durable retry coordination.
    pub(super) fn completion_failed() -> Self {
        let status = StatusCode::SERVICE_UNAVAILABLE;
        Self {
            status: status.as_u16(),
            body: problem_document(
                status,
                "run-admission-record-failed",
                "The run admission result could not be recorded for durable retry coordination.",
                "run_admission_record_failed",
            ),
        }
    }

    async fn from_response(response: Response) -> Result<Self, RunFailure> {
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .map_err(|error| malformed(format!("read stored problem body: {error}")))?;
        let body = serde_json::from_slice(&bytes)
            .map_err(|error| malformed(format!("decode stored problem body: {error}")))?;
        Ok(Self {
            status: status.as_u16(),
            body,
        })
    }

    fn into_response(self) -> Result<Response, RunFailure> {
        let status = StatusCode::from_u16(self.status)
            .map_err(|error| malformed(format!("stored problem status: {error}")))?;
        Ok(problem_response(status, self.body))
    }

    /// The error fields required by an AG-UI `RUN_ERROR` frame.
    pub(super) fn stream_fields(&self) -> (String, String, Option<String>) {
        let message = self
            .body
            .get("detail")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("The run could not be admitted.")
            .to_string();
        let code = self
            .body
            .get("code")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("run_admission_failed")
            .to_string();
        let capability = self
            .body
            .get("capability")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        (message, code, capability)
    }
}

/// A refusal persisted for exact replay, including where it was first exposed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct StoredFailure {
    pub(super) problem: StoredProblem,
    /// `true` after a synthetic start disclosed the effective session id.
    pub(super) streamed: bool,
}

/// A claim this process acquired and may complete exactly once.
pub(super) struct AcquiredClaim {
    store: Arc<dyn OperationStore>,
    claim: OperationClaim,
}

impl AcquiredClaim {
    /// Publish the finite result before reporting successful admission.
    pub(super) async fn complete<T: Serialize>(&self, outcome: &T) -> Result<(), RunFailure> {
        let encoded = encode_outcome(outcome)?;
        self.store
            .complete(&self.claim, encoded)
            .await
            .map_err(RunFailure::from)
    }
}

/// The first caller owns the side effect; later callers replay its finite result.
pub(super) enum Reservation<T> {
    Acquired(AcquiredClaim),
    Complete(T),
}

/// Failures that are specific to AG-UI's durable operation coordination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CoordinationProblem {
    Pending,
    Reused,
}

impl IntoResponse for CoordinationProblem {
    fn into_response(self) -> Response {
        let (status, slug, detail, code) = match self {
            Self::Pending => (
                StatusCode::CONFLICT,
                "agui-operation-in-flight",
                "This AG-UI operation is still in progress. Retry shortly.",
                "agui_operation_in_flight",
            ),
            Self::Reused => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "agui-operation-reused",
                "This AG-UI operation key already names a different request.",
                "agui_operation_reused",
            ),
        };
        problem_response(status, problem_document(status, slug, detail, code))
    }
}

/// Anything that can refuse the run before its SSE response is returned.
pub(super) enum RunFailure {
    Core(Problem),
    Stored(StoredProblem),
    Coordination(CoordinationProblem),
    Store(StoreUnavailable),
}

impl std::fmt::Debug for RunFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Core(_) => formatter.write_str("Core(..)"),
            Self::Stored(problem) => formatter.debug_tuple("Stored").field(problem).finish(),
            Self::Coordination(problem) => formatter
                .debug_tuple("Coordination")
                .field(problem)
                .finish(),
            Self::Store(problem) => formatter.debug_tuple("Store").field(problem).finish(),
        }
    }
}

impl From<CoreError> for RunFailure {
    fn from(error: CoreError) -> Self {
        Self::Core(error.into())
    }
}

impl From<StoreError> for RunFailure {
    fn from(_: StoreError) -> Self {
        Self::Store(StoreUnavailable)
    }
}

impl IntoResponse for RunFailure {
    fn into_response(self) -> Response {
        match self {
            Self::Core(problem) => problem.into_response(),
            Self::Stored(problem) => problem
                .into_response()
                .unwrap_or_else(|error| error.into_response()),
            Self::Coordination(problem) => problem.into_response(),
            Self::Store(problem) => problem.into_response(),
        }
    }
}

/// Atomically acquire an operation or replay the outcome its owner published.
pub(super) async fn reserve<T: DeserializeOwned>(
    store: Arc<dyn OperationStore>,
    caller: &Caller,
    operation: impl Into<String>,
    key: impl Into<String>,
    fingerprint: [u8; 32],
) -> Result<Reservation<T>, RunFailure> {
    let claim = OperationClaim::new(
        caller.subject.clone(),
        operation,
        key,
        fingerprint,
        SystemTime::now() + CLAIM_RETENTION,
    );
    let deadline = tokio::time::Instant::now() + PENDING_WAIT;

    loop {
        match store.reserve(&claim).await? {
            OperationReservation::Acquired => {
                return Ok(Reservation::Acquired(AcquiredClaim { store, claim }))
            }
            OperationReservation::Complete(outcome) => {
                return Ok(Reservation::Complete(decode_outcome(&outcome)?))
            }
            OperationReservation::FingerprintMismatch => {
                return Err(RunFailure::Coordination(CoordinationProblem::Reused))
            }
            OperationReservation::Pending if tokio::time::Instant::now() >= deadline => {
                return Err(RunFailure::Coordination(CoordinationProblem::Pending))
            }
            OperationReservation::Pending => {
                tokio::time::sleep(PENDING_POLL_INTERVAL).await;
            }
        }
    }
}

/// Hash a canonical semantic shape under this interface's versioned domain.
pub(super) fn fingerprint<T: Serialize>(value: &T) -> Result<[u8; 32], RunFailure> {
    let encoded = serde_json::to_vec(value)
        .map_err(|error| malformed(format!("encode operation fingerprint: {error}")))?;
    let mut digest = Sha256::new();
    digest.update(FINGERPRINT_DOMAIN);
    digest.update(encoded);
    Ok(digest.finalize().into())
}

#[derive(Serialize)]
struct OutcomeRef<'a, T> {
    version: u8,
    outcome: &'a T,
}

#[derive(Deserialize)]
struct Outcome<T> {
    version: u8,
    outcome: T,
}

fn encode_outcome<T: Serialize>(outcome: &T) -> Result<Vec<u8>, RunFailure> {
    serde_json::to_vec(&OutcomeRef {
        version: OUTCOME_VERSION,
        outcome,
    })
    .map_err(|error| malformed(format!("encode operation outcome: {error}")))
}

fn decode_outcome<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, RunFailure> {
    let stored: Outcome<T> = serde_json::from_slice(bytes)
        .map_err(|error| malformed(format!("decode operation outcome: {error}")))?;
    if stored.version != OUTCOME_VERSION {
        return Err(malformed(format!(
            "operation outcome version {} is not supported",
            stored.version
        )));
    }
    Ok(stored.outcome)
}

fn malformed(detail: String) -> RunFailure {
    tracing::error!(%detail, "AG-UI operation metadata is malformed");
    RunFailure::Store(StoreUnavailable)
}

#[cfg(test)]
#[path = "admission_tests.rs"]
mod tests;

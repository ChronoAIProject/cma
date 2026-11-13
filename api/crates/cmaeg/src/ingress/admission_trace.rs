//! Bounded observations; correlation never supplies admission or retry authority.
use super::{launch::LaunchRequest, routing_store::AdmissionError};
use crate::records::RecordError;
use cmaeg_core::problem::Problem;
use sha2::{Digest, Sha256};
use std::{future::Future, sync::OnceLock, time::Instant};
use tracing::{Instrument, Span};

tokio::task_local! { static ACTIVE: (); }

#[derive(Clone, Copy)]
pub(super) enum Stage {
    RootLookup,
    ChunkWrite,
    ChunkRead,
    RootWrite,
    RootReadback,
    Enqueue,
    IndexRetire,
}
impl Stage {
    fn name(self) -> &'static str {
        match self {
            Self::RootLookup => "root_lookup",
            Self::ChunkWrite => "chunk_write",
            Self::ChunkRead => "chunk_read",
            Self::RootWrite => "root_write",
            Self::RootReadback => "root_readback",
            Self::Enqueue => "enqueue",
            Self::IndexRetire => "index_retire",
        }
    }
}

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn random_hash() -> String {
    hash(uuid::Uuid::new_v4().as_bytes())
}
fn process_hash() -> &'static str {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(random_hash)
}

/// One durable-admission invocation, including restored roots. An HTTP request
/// may contain an ordinary attempt followed by a compatibility attempt; its
/// existing cmaeg.request parent is the separate transport correlation domain.
pub(super) async fn attempt<T>(
    request: &LaunchRequest,
    future: impl Future<Output = Result<T, Problem>>,
) -> Result<T, Problem> {
    let identity = super::routing_store::EventIdentity {
        owner: request.channel.definition.owner.subject.clone(),
        channel_id: request.channel.id.clone(),
        record_id: request.channel.pointer.record_id.clone(),
        event_id: request.activity.event_id.clone(),
    };
    // Match the existing queue-warning digest: canonical admission UUID bytes.
    let admission_sha256 = hash(identity.admission_id().as_bytes());
    let span = tracing::info_span!(
        "eg_admission_attempt",
        attempt_sha256 = random_hash(),
        process_sha256 = process_hash(),
        admission_sha256,
    );
    ACTIVE
        .scope((), async {
            let mut observation = Observation::new("eg_admission_attempt");
            let result = future.await;
            observation.finish(if result.is_ok() { "ok" } else { "refused" });
            if let Err(problem) = &result {
                tracing::info!(
                    event = "eg_admission_response",
                    public_status = problem.status,
                    "Admission returned a refusal"
                );
            }
            result
        })
        .instrument(span)
        .await
}

pub(super) async fn admission<T>(
    stage: Stage,
    future: impl Future<Output = Result<T, AdmissionError>>,
) -> Result<T, AdmissionError> {
    operation(stage, future, admission_class).await
}

pub(super) async fn record<T>(
    stage: Stage,
    future: impl Future<Output = Result<T, RecordError>>,
) -> Result<T, RecordError> {
    operation(stage, future, record_class).await
}

async fn operation<T, E>(
    stage: Stage,
    future: impl Future<Output = Result<T, E>>,
    classify: fn(&E) -> &'static str,
) -> Result<T, E> {
    // Recovery reads share the store but are not request-attempt observations.
    if ACTIVE.try_with(|_| ()).is_err() {
        return future.await;
    }
    let span = tracing::info_span!(
        "eg_admission_operation",
        stage = stage.name(),
        operation_sha256 = random_hash(),
    );
    async {
        let mut observation = Observation::new("eg_admission_stage");
        let result = future.await;
        observation.finish(result.as_ref().err().map_or("ok", classify));
        result
    }
    .instrument(span)
    .await
}

pub(super) fn admission_class(error: &AdmissionError) -> &'static str {
    match error {
        AdmissionError::Invalid => "invalid",
        AdmissionError::TooLarge => "too_large",
        AdmissionError::Conflict => "conflict",
        AdmissionError::GuardConflict => "guard_conflict",
        AdmissionError::Expired => "expired",
        AdmissionError::ExpiredOrUnavailable => "expired_or_unavailable",
        AdmissionError::Unavailable => "unavailable",
        AdmissionError::Uncertain => "uncertain",
        AdmissionError::Corrupt => "corrupt",
        AdmissionError::MissingChunk => "missing_chunk",
    }
}
fn record_class(error: &RecordError) -> &'static str {
    match error {
        RecordError::NotFound => "not_found",
        RecordError::Conflict { .. } => "conflict",
        RecordError::GuardConflict { .. } => "guard_conflict",
        RecordError::Unavailable(_) => "unavailable",
        RecordError::Rejected(_) => "rejected",
        RecordError::Malformed(_) => "malformed",
    }
}

struct Observation {
    event: &'static str,
    span: Span,
    started: Instant,
    finished: bool,
}
impl Observation {
    fn new(event: &'static str) -> Self {
        let observation = Self {
            event,
            span: Span::current(),
            started: Instant::now(),
            finished: false,
        };
        observation.emit("start", "pending");
        observation
    }
    fn emit(&self, boundary: &'static str, class: &'static str) {
        let elapsed_ms = u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        if matches!(class, "pending" | "ok") {
            tracing::debug!(parent: &self.span,
                event = self.event, boundary, class, elapsed_ms, "Admission observation"
            );
        } else {
            tracing::warn!(parent: &self.span,
                event = self.event, boundary, class, elapsed_ms, "Admission observation"
            );
        }
    }
    fn finish(&mut self, class: &'static str) {
        self.emit("end", class);
        self.finished = true;
    }
}
impl Drop for Observation {
    fn drop(&mut self) {
        if !self.finished {
            // Dropping a future proves neither rollback nor absence of custody.
            self.emit("cancelled", "uncertain");
        }
    }
}

#[cfg(test)]
#[path = "admission_trace_tests.rs"]
mod tests;

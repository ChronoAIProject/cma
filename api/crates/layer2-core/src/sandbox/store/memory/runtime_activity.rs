//! Coherent in-memory projection used by the same admission contract in tests.
use super::super::runtime_activity::*;
use super::{InMemorySandboxStore, SandboxId, SandboxStoreError, State};

mod records;
#[cfg(test)]
mod tests;

#[async_trait::async_trait]
impl SandboxRuntimeActivityObservationStore for InMemorySandboxStore {
    async fn observe_runtime_activity(
        &self,
        id: &SandboxId,
        context: &RuntimeActivitySourceContext,
    ) -> Result<RuntimeActivityObservation, SandboxStoreError> {
        // Memory has no production all-writer/retention qualification. Fixture
        // proofs are available only through the explicitly named test seam.
        let _ = (id, context);
        Ok(RuntimeActivityObservation::Unknown(
            RuntimeActivityUnknownReason::UnqualifiedSource,
        ))
    }
}

pub(super) fn validate_in_memory(
    state: &State,
    id: &SandboxId,
    proof: &RuntimeActivityProof,
    at_ms: i64,
) -> Result<(), SandboxStoreError> {
    proof.validate_at(at_ms)?;
    if proof.source_context().coverage != RuntimeActivityCoverage::MemoryFixtureV1 {
        return Err(invalid());
    }
    let current = snapshot(state, id, proof.source_context(), at_ms)?;
    proof.compare_current(&current, at_ms)
}

fn snapshot(
    state: &State,
    id: &SandboxId,
    source: &RuntimeActivitySourceContext,
    at_ms: i64,
) -> Result<RuntimeActivitySnapshot, SandboxStoreError> {
    let desktop = crate::sandbox::runtime_desktop_activity_snapshot(id).map_err(|_| invalid())?;
    Ok(RuntimeActivitySnapshot {
        source: source.clone(),
        capture_started_at_ms: at_ms,
        capture_completed_at_ms: at_ms,
        watermark: RuntimeActivityWatermark {
            seconds: u32::try_from(at_ms / 1000).map_err(|_| invalid())?,
            increment: 0,
        },
        storage_incarnation: "memory-fixture".into(),
        desktop: RuntimeActivityDesktop {
            process_incarnation: desktop.process_incarnation,
            tracking_incarnation: desktop.tracking_incarnation,
            activity_epoch: desktop.activity_epoch,
            opening_or_open: desktop.opening_or_open,
        },
        vector: records::capture(state, id, at_ms)?,
    })
}

#[cfg(any(test, feature = "test-support"))]
impl InMemorySandboxStore {
    /// Synthetic time is confined to this fixture API. The complete actual
    /// memory snapshot still catches stale child/command/session mutations.
    pub fn runtime_activity_quiet_proof_fixture(
        &self,
        id: &SandboxId,
        at_ms: i64,
    ) -> Result<RuntimeActivityProof, SandboxStoreError> {
        let source = RuntimeActivitySourceContext {
            source_id: "memory-fixture".into(),
            adapter_version: "1".into(),
            policy_id: "memory-fixture-only".into(),
            producer_id: "test".into(),
            database_id: "memory".into(),
            api_image: "fixture".into(),
            source_revision: "fixture".into(),
            process_incarnation: crate::sandbox::runtime_desktop_process_incarnation().into(),
            serving_processes: 1,
            replica_mode: false,
            clock_uncertainty_ms: 0,
            coverage: RuntimeActivityCoverage::MemoryFixtureV1,
        };
        let state = self.lock();
        let initial = snapshot(
            &state,
            id,
            &source,
            at_ms
                .checked_sub(RUNTIME_ACTIVITY_QUIET_MS)
                .ok_or_else(invalid)?,
        )?;
        let final_observation = snapshot(&state, id, &source, at_ms)?;
        let proof = RuntimeActivityProof {
            schema_version: 1,
            initial,
            final_observation,
        };
        proof.validate_at(at_ms)?;
        Ok(proof)
    }
}

fn invalid() -> SandboxStoreError {
    SandboxStoreError::InvalidTransaction(
        "runtime activity memory snapshot is incomplete or changed",
    )
}

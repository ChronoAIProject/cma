//! Unknown legacy reuse reads its receipt without reapplying admission.
//! Corrupt proof refuses without publishing quarantine during reconciliation.
use super::*;
use crate::store::sandbox_mongo::wait_for_reconciliation;

pub(in crate::store) async fn reconcile_existing(
    store: &MongoSandboxStore,
    admission: &AgentAdmission,
    expected: AgentCreation,
    at: Option<Timestamp>,
) -> Result<AgentAdmissionResult, SandboxStoreError> {
    let at = at.ok_or_else(unresolved)?;
    let store = store.clone();
    let admission = admission.clone();
    wait_for_reconciliation("existing Agent admission commit", async move {
        let mut session = store
            .client
            .start_session()
            .snapshot(true)
            .snapshot_time(at)
            .await
            .map_err(|_| unresolved())?;
        let replay = Box::pin(load_replay_with_session(
            &store,
            &admission,
            &mut session,
            ReplayRead::UncertainSnapshot,
        ))
        .await
        .map_err(load_error)?
        .map(|receipt| receipt.result())
        .ok_or_else(unresolved)?;
        // Keep the original key, request fingerprint, selected Agent, timestamp,
        // first prompt and exact revision. Do not select another reusable Agent.
        if replay.creation() != &expected {
            return Err(SandboxStoreError::Conflict);
        }
        Ok(AgentAdmissionResult::Existing(expected))
    })
    .await
    .map_err(SandboxStoreError::Unavailable)?
}

fn unresolved() -> SandboxStoreError {
    SandboxStoreError::Unavailable(
        "existing Agent admission outcome remains unknown; retain the original admission".into(),
    )
}

//! Unknown reuse never restarts admission or allocates another publication key.
use super::*;
use crate::store::agent_admission_records::ReusedReceipt;
use mongodb::bson::Timestamp;

pub(super) async fn after_unknown(
    store: &MongoSandboxStore,
    admission: &AgentAdmission,
    receipt: Box<ReusedReceipt>,
    at: Option<Timestamp>,
) -> Result<AgentAdmissionResult, SandboxStoreError> {
    let at = at.ok_or_else(unresolved)?;
    let store = store.clone();
    let admission = admission.clone();
    wait_for_reconciliation("segmented Agent reuse", async move {
        let mut session = store
            .client
            .start_session()
            .snapshot(true)
            .snapshot_time(at)
            .await
            .map_err(|_| unresolved())?;
        if !receipt.recorded.matches(&store, &mut session).await? {
            return Err(unresolved());
        }
        // The immutable reuse binding, original creation and Trigger association
        // must agree in the same snapshot as the exact publication proof.
        let replay = super::super::agent_admission_records::replay_at_snapshot(
            &store,
            &admission,
            &mut session,
        )
        .await?
        .ok_or_else(unresolved)?;
        if replay.creation().agent.id != receipt.creation.agent.id
            || replay.creation().initial_log != receipt.creation.initial_log
            || replay.creation().worktree.id != receipt.creation.worktree.id
        {
            return Err(SandboxStoreError::Conflict);
        }
        Ok(AgentAdmissionResult::Existing(receipt.creation))
    })
    .await
    .map_err(SandboxStoreError::Unavailable)?
}

fn unresolved() -> SandboxStoreError {
    SandboxStoreError::Unavailable(
        "Agent reuse publication outcome remains unknown; retain the original admission".into(),
    )
}

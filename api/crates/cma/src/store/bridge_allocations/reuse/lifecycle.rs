//! The ordinary parent owns pause/revive; Channel owns only its child reservation.
use super::*;
use layer2_core::sandbox::{SandboxReviveCause, SandboxStatus};

impl MongoBridgeAllocationStore {
    /// A successful lifecycle admission invalidates the original root snapshot.
    /// Callers yield and acquire a fresh authority before any runtime operation.
    pub(crate) async fn revive_reuse_parent(
        &self,
        authority: &AllocationAuthority,
        lease: &BridgeControlPlaneLease,
        allocation: &AllocationRecord,
    ) -> Result<(), AllocationError> {
        for _ in 0..8 {
            transaction::require_current(authority, lease, allocation)?;
            let mut session = self
                .bridges
                .client
                .start_session()
                .await
                .map_err(|_| AllocationError::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| AllocationError::Unavailable)?;
            let result = apply(self, authority, lease, allocation, &mut session).await;
            if let Err(error) = result {
                let _ = session.abort_transaction().await;
                match error {
                    MutationError::Core(error) => return Err(error),
                    MutationError::Mongo(error) if is_transient(&error) => continue,
                    MutationError::Mongo(_) => return Err(AllocationError::Unavailable),
                }
            }
            if let Err(error) = transaction::require_current(authority, lease, allocation) {
                let _ = session.abort_transaction().await;
                return Err(error);
            }
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(()),
                CommitOutcome::Retry(_) => continue,
                CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => {
                    return Err(AllocationError::Unavailable)
                }
            }
        }
        Err(AllocationError::Unavailable)
    }
}
async fn apply(
    store: &MongoBridgeAllocationStore,
    authority: &AllocationAuthority,
    lease: &BridgeControlPlaneLease,
    allocation: &AllocationRecord,
    session: &mut ClientSession,
) -> Result<(), MutationError> {
    transaction::fence_allocation(store, authority, lease, allocation, session).await?;
    let AllocationAuthority::Reuse(proof) = authority else {
        return Err(AllocationError::IdentityMismatch.into());
    };
    let old = &proof.resources().lineage.sandbox;
    if !matches!(old.status, SandboxStatus::Paused | SandboxStatus::Reviving) {
        return Err(AllocationError::Unavailable.into());
    }
    let resources = &store
        .invokes
        .as_ref()
        .ok_or(AllocationError::AuthorityUnavailable)?
        .resources;
    let stamp = layer2_core::rfc3339_at(std::time::SystemTime::now());
    let mut next = old.clone();
    let cause = if proof.parent().workspace_created_at.is_some() {
        SandboxReviveCause::AgentCreation
    } else {
        SandboxReviveCause::WorkspaceCreation
    };
    let event = next
        .record_creation_admission(cause, stamp.clone())
        .map_err(|_| AllocationError::Unavailable)?;
    if &next != old {
        let database = store.bridges.client.database(&store.records.namespace().db);
        transaction::replace_fields(
            &database,
            "sandboxes",
            old.id.as_str(),
            old.revision,
            crate::store::sandboxes::sandbox_replacement_fields(&next)
                .map_err(|_| AllocationError::Corrupt)?,
            session,
        )
        .await?;
    }
    if let Some(event) = event {
        crate::store::sandbox_events::append_in_transaction(
            resources, session, &old.id, stamp, event,
        )
        .await
        .map_err(|error| match error {
            crate::store::sandbox_events::TransactionEventError::Driver(error) => {
                MutationError::Mongo(error)
            }
            crate::store::sandbox_events::TransactionEventError::Domain(_) => {
                MutationError::Core(AllocationError::Conflict)
            }
        })?;
    }
    Ok(())
}

use super::*;
use crate::sandbox::owned_workspace_cleanup::WorkspaceCleanupState;

pub(super) async fn finish(
    store: &MongoBridgeAllocationStore,
    database: &Database,
    state: &WorkspaceCleanupState,
    mut workspace: Workspace,
    session: &mut ClientSession,
) -> Result<u64, JobError> {
    let now = layer2_core::now_rfc3339();
    workspace
        .transition(WorkspaceStatus::Removing, now.clone(), None)
        .map_err(|_| CleanupError::Corrupt)?;
    workspace
        .transition(WorkspaceStatus::Removed, now.clone(), None)
        .map_err(|_| CleanupError::Corrupt)?;
    // No generic work step is scheduled and no child aggregate is rewritten.
    if workspace.work.step != WorkspaceWorkStep::None {
        return Err(CleanupError::Corrupt.into());
    }
    let row = read(database, "workspaces", workspace.id.as_str(), session).await?;
    let fields = crate::store::workspaces::workspace_replacement_fields(&workspace)
        .map_err(|_| CleanupError::Corrupt)?;
    let changed = majority_collection::<Document>(database, "workspaces")
        .update_one(row, doc! {"$set":fields})
        .session(&mut *session)
        .await?;
    if changed.matched_count != 1 {
        return Err(CleanupError::Conflict.into());
    }
    let resources = &store
        .invokes
        .as_ref()
        .ok_or(CleanupError::Unavailable)?
        .resources;
    crate::store::sandbox_events::append_in_transaction(
        resources,
        session,
        &workspace.sandbox_id,
        now,
        layer2_core::sandbox::SandboxEventKind::WorkspaceRemoved {
            workspace_id: workspace.id.clone(),
        },
    )
    .await
    .map_err(|error| match error {
        crate::store::sandbox_events::TransactionEventError::Driver(error) => {
            JobError::Mongo(error)
        }
        crate::store::sandbox_events::TransactionEventError::Domain(_) => {
            JobError::Core(CleanupError::Corrupt)
        }
    })?;
    let changed = majority_collection::<Document>(database, "sandboxes")
        .update_one(
            doc! {"_id":workspace.sandbox_id.as_str(),CLEANUP_EXECUTION_FIELD:state.id()},
            doc! {"$unset":{CLEANUP_EXECUTION_FIELD:""}},
        )
        .session(session)
        .await?;
    if changed.matched_count != 1 {
        return Err(CleanupError::Conflict.into());
    }
    Ok(workspace.revision)
}

impl MongoBridgeAllocationStore {
    #[cfg(test)]
    pub(in crate::store) async fn cleanup_released_workspace_capacity(
        &self,
        allocation: &AllocationRecord,
        session: &mut ClientSession,
    ) -> Result<bool, MutationError> {
        let database = self.bridges.client.database(&self.records.namespace().db);
        Self::released_reuse_workspace_capacity(&database, allocation, session).await
    }

    pub(in crate::store) async fn released_reuse_workspace_capacity(
        database: &Database,
        allocation: &AllocationRecord,
        session: &mut ClientSession,
    ) -> Result<bool, MutationError> {
        if !matches!(
            allocation.placement,
            layer2_core::bridge::allocation::AllocationPlacement::ReuseParent { .. }
        ) {
            return Ok(false);
        }
        let sandbox = crate::store::sandboxes::read_sandbox(
            &read(
                database,
                "sandboxes",
                allocation.placement.target().sandbox_id.as_str(),
                session,
            )
            .await?,
        )
        .map_err(|_| AllocationError::Corrupt)?;
        let proof = proof::retained(database, allocation, &sandbox, session)
            .await
            .map_err(core_error)?;
        Ok(proof.is_some_and(|(_, state, _)| state.progress == WorkspaceCleanupProgress::Complete))
    }
}

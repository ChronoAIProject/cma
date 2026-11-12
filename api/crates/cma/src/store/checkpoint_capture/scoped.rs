use super::*;
use crate::store::{
    agent_capacity_recovery::{lineage, source::fence_filter_scoped},
    agent_log_segments::{capture::capture_transaction, transaction_scope::TransactionScope},
};

/// No commit/abort here. The lifecycle root starts and owns this exact scope.
pub(in crate::store) async fn validate_and_fence_begin_scoped(
    store: &MongoSandboxStore,
    current: &Sandbox,
    next: &Sandbox,
    agents: &[Agent],
    scope: &mut TransactionScope<'_>,
) -> Result<(), LifecycleLoadError> {
    if current.lifecycle.checkpoint_capture.is_some() || next.lifecycle.checkpoint_capture.is_none()
    {
        return Ok(());
    }
    let workspaces = lineage::workspaces(store, &current.id, scope)
        .await
        .map_err(log_error)?;
    validate_checkpoint_capture_work(current, agents, &workspaces)
        .map_err(LifecycleLoadError::Domain)?;
    let mut activities = Vec::new();
    for agent in agents {
        let source = capture_transaction(store, agent, scope)
            .await
            .map_err(log_error)?;
        if agent.status.is_live() {
            activities.push(
                SandboxAgentActivity::from_live_agent(agent, source.latest_entry()).map_err(
                    |error| {
                        LifecycleLoadError::Domain(SandboxStoreError::Corrupt(error.to_string()))
                    },
                )?,
            );
        }
    }
    validate_checkpoint_capture_quiet(current, &activities, &next.updated_at)
        .map_err(LifecycleLoadError::Domain)?;
    for agent in agents {
        let filter = fence_filter_scoped(store, agent, scope)
            .await
            .map_err(log_error)?;
        let result = scope
            .update_one(
                &store.agents,
                filter,
                doc! {"$inc": {"checkpoint_capture_fence": 1_i64}},
            )
            .await
            .map_err(LifecycleLoadError::Driver)?;
        if result.matched_count != 1 {
            return Err(LifecycleLoadError::Domain(SandboxStoreError::Conflict));
        }
    }
    for workspace in workspaces {
        let revision = signed(workspace.revision, "checkpoint capture row revision")
            .map_err(|error| LifecycleLoadError::Domain(SandboxStoreError::Corrupt(error)))?;
        let result = scope.update_one(&store.workspaces, doc! {
            "_id": workspace.id.as_str(), "document_schema_version": DOCUMENT_SCHEMA_VERSION,
            "sandbox_id": current.id.as_str(), "revision": revision,
        }, doc! {"$inc": {"checkpoint_capture_fence": 1_i64}}).await.map_err(LifecycleLoadError::Driver)?;
        if result.matched_count != 1 {
            return Err(LifecycleLoadError::Domain(SandboxStoreError::Conflict));
        }
    }
    Ok(())
}

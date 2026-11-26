use super::*;
use layer2_core::sandbox::store::{requires_agent_skill_result_guard, AgentSkillResultGuard};

pub(super) async fn fence(
    store: &MongoSandboxStore,
    current: &Agent,
    mutation: &AgentWorkMutation,
    guard: Option<&AgentSkillResultGuard>,
    scope: &mut TransactionScope<'_>,
) -> Result<(), ApplyError> {
    if requires_agent_skill_result_guard(current, mutation) != guard.is_some() {
        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
    }
    let Some(guard) = guard else { return Ok(()) };
    let sandbox = scope
        .find_one(&store.sandboxes, doc! {"_id":current.sandbox_id.as_str()})
        .await
        .map_err(ApplyError::Driver)?
        .as_ref()
        .map(super::super::sandboxes::read_sandbox)
        .transpose()
        .map_err(ApplyError::Domain)?
        .ok_or(ApplyError::Domain(SandboxStoreError::NotFound))?;
    let workspace = scope
        .find_one(
            &store.workspaces,
            doc! {"_id":current.workspace_id.as_str()},
        )
        .await
        .map_err(ApplyError::Driver)?
        .as_ref()
        .map(super::super::workspaces::read_workspace)
        .transpose()
        .map_err(ApplyError::Domain)?
        .ok_or(ApplyError::Domain(SandboxStoreError::NotFound))?;
    let tree = scope
        .find_one(&store.worktrees, doc! {"_id":current.worktree_id.as_str()})
        .await
        .map_err(ApplyError::Driver)?
        .as_ref()
        .map(read_worktree)
        .transpose()
        .map_err(ApplyError::Domain)?
        .ok_or(ApplyError::Domain(SandboxStoreError::NotFound))?;
    guard
        .validate(current, &sandbox, &workspace, &tree)
        .map_err(ApplyError::Domain)?;
    // These real writes invalidate a snapshot that predates a parent change.
    // They commit or roll back with the Agent and either log-storage format.
    for (collection, id, revision) in [
        (&store.sandboxes, sandbox.id.as_str(), sandbox.revision),
        (&store.workspaces, workspace.id.as_str(), workspace.revision),
        (&store.worktrees, tree.id.as_str(), tree.revision),
    ] {
        let written = scope.update_one(collection,
            crate::store::work_schema::for_collection(collection, doc! {
                "_id": id, "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                "revision": signed(revision, "skill parent revision").map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?,
                "owned_workspace_cleanup_admission_v1": {"$exists": false},
                "channel_reuse_cleanup_execution": {"$exists": false},
            }), doc! {"$inc": {"runtime_writer_claim_fence": 1_i64}})
            .await.map_err(ApplyError::Driver)?;
        if written.matched_count != 1 {
            return Err(ApplyError::Domain(SandboxStoreError::Conflict));
        }
    }
    Ok(())
}

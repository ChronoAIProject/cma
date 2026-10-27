//! Serialize paired resource replay with its consumed preparation and live parents.
use super::*;

pub(super) async fn fence(
    store: &MongoSandboxStore,
    agent: &Agent,
    tree: &Worktree,
    mutation: &AgentWorkMutation,
    session: &mut mongodb::ClientSession,
) -> Result<(), ApplyError> {
    if !layer2_core::sandbox::store::is_prepared_agent_replay_pair(agent, mutation) {
        return Ok(());
    }
    let sandbox = store
        .sandboxes
        .find_one(doc! {"_id": agent.sandbox_id.as_str()})
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
        .as_ref()
        .map(crate::store::sandboxes::read_sandbox)
        .transpose()
        .map_err(ApplyError::Domain)?
        .ok_or(ApplyError::Domain(SandboxStoreError::NotFound))?;
    let workspace = store
        .workspaces
        .find_one(doc! {"_id": agent.workspace_id.as_str()})
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
        .as_ref()
        .map(crate::store::workspaces::read_workspace)
        .transpose()
        .map_err(ApplyError::Domain)?
        .ok_or(ApplyError::Domain(SandboxStoreError::NotFound))?;
    // At intent publication validate the proposed debt digests. At completion
    // validate the persisted debts, which full mutation validation binds to
    // both proposed receipts.
    let proof_agent = if mutation.agent.work.step == AgentWorkStep::AddWorktree {
        &mutation.agent
    } else {
        agent
    };
    let revision = crate::store::native_prepared::replay_proof_revision(
        store,
        proof_agent,
        &sandbox,
        &workspace,
        tree,
        session,
    )
    .await
    .map_err(ApplyError::Domain)?
    .ok_or(ApplyError::Domain(SandboxStoreError::Conflict))?;
    for (collection, id, revision) in [
        (&store.sandboxes, sandbox.id.as_str(), sandbox.revision),
        (&store.workspaces, workspace.id.as_str(), workspace.revision),
        (&store.worktrees, tree.id.as_str(), tree.revision),
        (&store.native_prepared, sandbox.id.as_str(), revision),
    ] {
        let written = collection
            .update_one(
                doc! {
                    "_id": id,
                    "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                    "revision": signed(revision, "prepared parent revision")
                        .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?,
                    "owned_workspace_cleanup_admission_v1": {"$exists": false},
                    "channel_reuse_cleanup_execution": {"$exists": false},
                },
                doc! {"$inc": {"agent_prepared_replay_fence": 1_i64}},
            )
            .session(&mut *session)
            .await
            .map_err(ApplyError::Driver)?;
        if written.matched_count != 1 {
            return Err(ApplyError::Domain(SandboxStoreError::Conflict));
        }
    }
    Ok(())
}

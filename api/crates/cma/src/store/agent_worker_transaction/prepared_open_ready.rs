//! A fresh observed hierarchy must still be live at the atomic Ready publication.
use super::*;
use layer2_core::sandbox::store::PreparedAgentOpenContext;

pub(super) async fn proof_revision(
    store: &MongoSandboxStore,
    current: &Agent,
    observed: &PreparedAgentOpenContext,
    session: &mut mongodb::ClientSession,
) -> Result<u64, ApplyError> {
    super::super::native_prepared::open_ready_proof_revision(
        store,
        current,
        &observed.sandbox,
        &observed.workspace,
        &observed.worktree,
        session,
    )
    .await
    .map_err(ApplyError::Domain)?
    .ok_or(ApplyError::PreparedIneligible)
}

pub(super) async fn fence(
    store: &MongoSandboxStore,
    agent: &Agent,
    tree: &Worktree,
    observed: &PreparedAgentOpenContext,
    revision: u64,
    session: &mut mongodb::ClientSession,
) -> Result<(), ApplyError> {
    let sandbox = store
        .sandboxes
        .find_one(doc! {"_id": agent.sandbox_id.as_str()})
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
        .as_ref()
        .map(super::super::sandboxes::read_sandbox)
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
        .map(super::super::workspaces::read_workspace)
        .transpose()
        .map_err(ApplyError::Domain)?
        .ok_or(ApplyError::Domain(SandboxStoreError::NotFound))?;
    if sandbox != observed.sandbox || workspace != observed.workspace || tree != &observed.worktree
    {
        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
    }
    // The proof read and these exact parent observations share this transaction.
    // A concurrent preparation/parent write must still conflict with its fence.
    for (collection, id, revision) in [
        (&store.sandboxes, sandbox.id.as_str(), sandbox.revision),
        (&store.workspaces, workspace.id.as_str(), workspace.revision),
        (&store.worktrees, tree.id.as_str(), tree.revision),
        (&store.native_prepared, sandbox.id.as_str(), revision),
    ] {
        let written = collection.update_one(doc! {
            "_id": id, "document_schema_version": DOCUMENT_SCHEMA_VERSION,
            "revision": signed(revision, "prepared Open parent revision").map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?,
            "owned_workspace_cleanup_admission_v1": {"$exists": false},
            "channel_reuse_cleanup_execution": {"$exists": false},
        }, doc! {"$inc": {"agent_prepared_open_ready_fence": 1_i64}})
            .session(&mut *session).await.map_err(ApplyError::Driver)?;
        if written.matched_count != 1 {
            return Err(ApplyError::Domain(SandboxStoreError::Conflict));
        }
    }
    Ok(())
}

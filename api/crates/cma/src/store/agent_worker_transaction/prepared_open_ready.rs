//! A fresh observed hierarchy must still be live at the atomic Ready publication.
use super::*;
use layer2_core::sandbox::store::{validate_prepared_agent_open_ready, PreparedAgentOpenContext};

pub(super) async fn eligible(
    store: &MongoSandboxStore,
    mutation: &AgentWorkMutation,
    observed: &PreparedAgentOpenContext,
) -> Result<bool, SandboxStoreError> {
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
    let current = store
        .agents
        .find_one(doc! {"_id": mutation.agent.id.as_str()})
        .session(&mut session)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
        .as_ref()
        .map(read_agent)
        .transpose()?
        .ok_or(SandboxStoreError::NotFound)?;
    let eligible = super::super::native_prepared::open_ready_proof_revision(
        store,
        &current,
        &observed.sandbox,
        &observed.workspace,
        &observed.worktree,
        &mut session,
    )
    .await?
    .is_some();
    if eligible {
        validate_prepared_agent_open_ready(
            &current,
            &observed.worktree,
            current.conversation.last_seq,
            mutation,
        )?;
    }
    Ok(eligible)
}

pub(super) async fn fence(
    store: &MongoSandboxStore,
    agent: &Agent,
    tree: &Worktree,
    observed: &PreparedAgentOpenContext,
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
    let revision = super::super::native_prepared::open_ready_proof_revision(
        store, agent, &sandbox, &workspace, tree, session,
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

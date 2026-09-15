//! A tombstone may never resolve an old path to a different resource incarnation.
use crate::sandbox::channel_reuse_cleanup::ChannelReuseRemovalState;
use crate::store::{
    sandbox_mongo::MongoSandboxStore, workspaces::read_workspace, worktrees::read_worktree,
};
use layer2_core::sandbox::store::SandboxStoreError;
use mongodb::{
    bson::{doc, Document},
    ClientSession,
};

pub(super) async fn require(
    store: &MongoSandboxStore,
    state: &ChannelReuseRemovalState,
    session: &mut ClientSession,
) -> Result<(), SandboxStoreError> {
    let seal = &state.seal;
    let owner = match &seal.owner {
        layer2_core::resource_access::ResourceOwner::Person { subject }
        | layer2_core::resource_access::ResourceOwner::Organization { subject } => subject,
    };
    let mut rows = store
        .workspaces
        .find(path_candidates(
            seal.target.sandbox_id.as_str(),
            seal.target.workspace_id.as_str(),
            &seal.workspace_path,
        ))
        .limit(257)
        .session(&mut *session)
        .await
        .map_err(unavailable)?;
    let mut found = false;
    while rows.advance(&mut *session).await.map_err(unavailable)? {
        let row: Document = rows.deserialize_current().map_err(unavailable)?;
        let workspace = read_workspace(&row)?;
        if found
            || workspace.id != seal.target.workspace_id
            || &workspace.owner != owner
            || workspace.created_at != seal.workspace_created_at
            || workspace.path != seal.workspace_path
            || workspace.sandbox_id != seal.target.sandbox_id
        {
            return Err(SandboxStoreError::Conflict);
        }
        found = true;
        let changed = store
            .workspaces
            .update_one(row, doc! {"$inc":{"cleanup_restore_fence":1_i64}})
            .session(&mut *session)
            .await
            .map_err(unavailable)?;
        if changed.matched_count != 1 {
            return Err(SandboxStoreError::Conflict);
        }
    }
    if !found {
        return Err(SandboxStoreError::Conflict);
    }
    let mut rows = store
        .worktrees
        .find(path_candidates(
            seal.target.sandbox_id.as_str(),
            seal.target.worktree_id.as_str(),
            &seal.worktree_path,
        ))
        .limit(257)
        .session(&mut *session)
        .await
        .map_err(unavailable)?;
    let mut found = false;
    while rows.advance(&mut *session).await.map_err(unavailable)? {
        let row: Document = rows.deserialize_current().map_err(unavailable)?;
        let tree = read_worktree(&row)?;
        if found
            || tree.id != seal.target.worktree_id
            || tree.workspace_id != seal.target.workspace_id
            || tree.agent_id.as_ref() != Some(&seal.target.agent_id)
            || tree.path != seal.worktree_path
        {
            return Err(SandboxStoreError::Conflict);
        }
        found = true;
        let changed = store
            .worktrees
            .update_one(row, doc! {"$inc":{"cleanup_restore_fence":1_i64}})
            .session(&mut *session)
            .await
            .map_err(unavailable)?;
        if changed.matched_count != 1 {
            return Err(SandboxStoreError::Conflict);
        }
    }
    if !found {
        return Err(SandboxStoreError::Conflict);
    }
    Ok(())
}
// Include indexed and embedded lineage: a corrupt index must not hide a
// different incarnation whose durable record still names this runtime path.
fn path_candidates(parent: &str, id: &str, path: &str) -> Document {
    doc! {"$or":[{"_id":id},{"$and":[
        {"$or":[{"sandbox_id":parent},{"record.sandbox_id":parent}]},
        {"$or":[{"path":path},{"record.path":path}]}
    ]}]}
}
fn unavailable(error: mongodb::error::Error) -> SandboxStoreError {
    SandboxStoreError::Unavailable(error.to_string())
}

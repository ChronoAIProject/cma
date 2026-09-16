use crate::sandbox::channel_reuse_cleanup::{
    ChannelReuseRemovalPhase, CLEANUP_EXECUTION_FIELD, CLEANUP_TOMBSTONES,
};
use crate::store::sandbox_mongo::{
    commit_transaction, start_transaction, CommitOutcome, MongoSandboxStore,
};
use crate::store::{bridge_allocations::read_cleanup_tombstone, sandboxes::read_sandbox};
use layer2_core::sandbox::{
    store::{RetiredAgentRuntimePaths, RetiredRuntimeCleanupPlan, SandboxStoreError},
    Sandbox,
};
use mongodb::bson::{doc, Document};
mod ownership;

pub(super) async fn paths(
    store: &MongoSandboxStore,
    sandbox: &Sandbox,
) -> Result<RetiredRuntimeCleanupPlan, SandboxStoreError> {
    let mut session = store.client.start_session().await.map_err(unavailable)?;
    start_transaction(&mut session).await.map_err(unavailable)?;
    let result = read_paths(store, sandbox, &mut session).await;
    let result = match result {
        Ok(value) => value,
        Err(error) => {
            let _ = session.abort_transaction().await;
            return Err(error);
        }
    };
    match commit_transaction(&mut session).await {
        CommitOutcome::Committed => Ok(result),
        _ => Err(SandboxStoreError::Conflict),
    }
}

async fn read_paths(
    store: &MongoSandboxStore,
    sandbox: &Sandbox,
    session: &mut mongodb::ClientSession,
) -> Result<RetiredRuntimeCleanupPlan, SandboxStoreError> {
    let parent = store
        .sandboxes
        .find_one(doc! {"_id":sandbox.id.as_str()})
        .session(&mut *session)
        .await
        .map_err(unavailable)?
        .ok_or(SandboxStoreError::NotFound)?;
    if read_sandbox(&parent)? != *sandbox || parent.contains_key(CLEANUP_EXECUTION_FIELD) {
        return Err(SandboxStoreError::Conflict);
    }
    let database = store.client.database(&store.sandboxes.namespace().db);
    let policy_row = store
        .resource_policies
        .find_one(doc! {"_id":sandbox.id.as_str()})
        .session(&mut *session)
        .await
        .map_err(unavailable)?;
    let owner = policy_row
        .as_ref()
        .map(crate::store::resource_access::read_inventory_policy)
        .transpose()
        .map_err(|_| SandboxStoreError::Conflict)?
        .map_or_else(
            || layer2_core::resource_access::ResourceOwner::Person {
                subject: sandbox.owner.clone(),
            },
            |policy| policy.owner,
        );
    let tombstones = database.collection::<Document>(CLEANUP_TOMBSTONES);
    let mut cursor = tombstones
        .find(doc! {"$or":[{"sandbox_id":sandbox.id.as_str()},
        {"record.seal.target.sandbox_id":sandbox.id.as_str()}]})
        .sort(doc! {"_id":1})
        .limit(257)
        .session(&mut *session)
        .await
        .map_err(unavailable)?;
    let mut result = Vec::new();
    while cursor.advance(&mut *session).await.map_err(unavailable)? {
        if result.len() == 256 {
            return Err(SandboxStoreError::Conflict);
        }
        let row = cursor.deserialize_current().map_err(unavailable)?;
        let tombstone = read_cleanup_tombstone(&row).map_err(|_| SandboxStoreError::Conflict)?;
        if tombstone.phase != ChannelReuseRemovalPhase::Complete
            || tombstone.seal.target.sandbox_id != sandbox.id
            || tombstone.seal.sandbox_created_at != sandbox.created_at
            || tombstone.seal.owner != owner
        {
            return Err(SandboxStoreError::Conflict);
        }
        let audit = database
            .collection::<Document>("channel_reuse_cleanup_removal_audits")
            .find_one(doc! {"_id":format!("{}:revision:{}",tombstone.id,tombstone.revision)})
            .session(&mut *session)
            .await
            .map_err(unavailable)?
            .ok_or(SandboxStoreError::Conflict)?;
        if read_cleanup_tombstone(&audit).map_err(|_| SandboxStoreError::Conflict)? != tombstone {
            return Err(SandboxStoreError::Conflict);
        }
        crate::store::bridge_allocations::require_completed_cleanup_child(
            &database, &tombstone, session,
        )
        .await
        .map_err(|_| SandboxStoreError::Conflict)?;
        ownership::require(store, &tombstone, session).await?;
        result.push(RetiredAgentRuntimePaths {
            agent_id: tombstone.seal.target.agent_id.clone(),
            workspace_id: tombstone.seal.target.workspace_id.clone(),
            workspace_name: tombstone.workspace_name().into(),
            worktree_name: tombstone.worktree_name().into(),
            authority_fence: tombstone.authority_fence,
        });
    }
    // Retained exact-child markers also require their permanent tombstones.
    // A missing/misindexed tombstone cannot silently reopen an old archive.
    let mut marked = store
        .agents
        .find(doc! {"$or":[{"sandbox_id":sandbox.id.as_str()},
        {"record.sandbox_id":sandbox.id.as_str()}],CLEANUP_EXECUTION_FIELD:{"$exists":true}})
        .limit(257)
        .session(&mut *session)
        .await
        .map_err(unavailable)?;
    let mut count = 0;
    while marked.advance(&mut *session).await.map_err(unavailable)? {
        count += 1;
        if count > 256 {
            return Err(SandboxStoreError::Conflict);
        }
        let row: Document = marked.deserialize_current().map_err(unavailable)?;
        let agent = crate::store::agents::read_agent(&row)?;
        if agent.sandbox_id != sandbox.id
            || agent.owner != sandbox.owner
            || !result.iter().any(|path| path.agent_id == agent.id)
            || agent.status != layer2_core::sandbox::AgentStatus::Deleted
        {
            return Err(SandboxStoreError::Conflict);
        }
    }
    let workspaces = crate::store::bridge_allocations::verified_workspace_restore_paths(
        &database, sandbox, &owner, session,
    )
    .await
    .map_err(|_| SandboxStoreError::Conflict)?;
    // Workspace and child proofs write parent fences; match its fresh exact row.
    let parent = store
        .sandboxes
        .find_one(doc! {"_id":sandbox.id.as_str()})
        .session(&mut *session)
        .await
        .map_err(unavailable)?
        .ok_or(SandboxStoreError::NotFound)?;
    if read_sandbox(&parent)? != *sandbox || parent.contains_key(CLEANUP_EXECUTION_FIELD) {
        return Err(SandboxStoreError::Conflict);
    }
    let changed = store
        .sandboxes
        .update_one(parent, doc! {"$inc":{"cleanup_restore_fence":1_i64}})
        .session(session)
        .await
        .map_err(unavailable)?;
    if changed.matched_count != 1 {
        return Err(SandboxStoreError::Conflict);
    }
    Ok(RetiredRuntimeCleanupPlan {
        agents: result,
        workspaces,
    })
}
fn unavailable(error: mongodb::error::Error) -> SandboxStoreError {
    SandboxStoreError::Unavailable(error.to_string())
}

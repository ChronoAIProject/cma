//! Old cleanup journals never grant an ordinary restore authority to resurrect removed paths.
use crate::store::sandbox_mongo::{
    commit_transaction, start_transaction, CommitOutcome, MongoSandboxStore,
};
use crate::store::sandboxes::read_sandbox;
use layer2_core::sandbox::{
    store::{RetiredRuntimeCleanupPlan, SandboxStoreError},
    Sandbox,
};
use mongodb::bson::{doc, Document};

pub(super) async fn paths(
    store: &MongoSandboxStore,
    sandbox: &Sandbox,
) -> Result<RetiredRuntimeCleanupPlan, SandboxStoreError> {
    let mut session = store.client.start_session().await.map_err(unavailable)?;
    start_transaction(&mut session).await.map_err(unavailable)?;
    let parent = store
        .sandboxes
        .find_one(doc! {"_id":sandbox.id.as_str()})
        .session(&mut session)
        .await
        .map_err(unavailable)?
        .ok_or(SandboxStoreError::NotFound)?;
    if read_sandbox(&parent)? != *sandbox || parent.contains_key("channel_reuse_cleanup_execution")
    {
        return Err(SandboxStoreError::Conflict);
    }
    let database = store.client.database(&store.sandboxes.namespace().db);
    for name in [
        "channel_reuse_cleanup_tombstones",
        "owned_workspace_cleanup_tombstones_v1",
    ] {
        if database.collection::<Document>(name).find_one(doc! {"$or":[
            {"sandbox_id":sandbox.id.as_str()}, {"record.seal.target.sandbox_id":sandbox.id.as_str()},
            {"record.admission.sandbox_id":sandbox.id.as_str()}
        ]}).session(&mut session).await.map_err(unavailable)?.is_some() {
            return Err(SandboxStoreError::Unavailable("retained legacy removal evidence requires explicit migration before restore".into()));
        }
    }
    for rows in [&store.agents, &store.workspaces] {
        if rows.find_one(doc! {"$and":[
            {"$or":[{"sandbox_id":sandbox.id.as_str()},{"record.sandbox_id":sandbox.id.as_str()}]},
            {"$or":[{"channel_reuse_cleanup_execution":{"$exists":true}}, {"owned_workspace_cleanup_admission_v1":{"$exists":true}}]}
        ]}).session(&mut session).await.map_err(unavailable)?.is_some() {
            return Err(SandboxStoreError::Conflict);
        }
    }
    let changed = store
        .sandboxes
        .update_one(parent, doc! {"$inc":{"cleanup_restore_fence":1_i64}})
        .session(&mut session)
        .await
        .map_err(unavailable)?;
    if changed.matched_count != 1 {
        return Err(SandboxStoreError::Conflict);
    }
    match commit_transaction(&mut session).await {
        CommitOutcome::Committed => Ok(RetiredRuntimeCleanupPlan {
            agents: vec![],
            workspaces: vec![],
        }),
        _ => Err(SandboxStoreError::Conflict),
    }
}
fn unavailable(error: mongodb::error::Error) -> SandboxStoreError {
    SandboxStoreError::Unavailable(error.to_string())
}

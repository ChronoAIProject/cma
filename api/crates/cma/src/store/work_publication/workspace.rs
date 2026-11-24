//! A worker's execution-policy save fences its parent in the same transaction.

use crate::store::transaction_terminal::GuardedTerminal as _;
use layer2_core::sandbox::{store::SandboxStoreError, Workspace};
use mongodb::bson::doc;

use crate::store::sandbox_mongo::{
    commit_transaction, signed, start_transaction, CommitOutcome, MongoSandboxStore,
    DOCUMENT_SCHEMA_VERSION,
};

pub(in crate::store) async fn save(
    store: &MongoSandboxStore,
    expected: u64,
    workspace: &Workspace,
) -> Result<(), SandboxStoreError> {
    let mut session = store.client.start_session().await.map_err(unavailable)?;
    start_transaction(&mut session).await.map_err(unavailable)?;
    let outcome = async {
        super::fence(store, &mut session).await?;
        let result = store.workspaces.update_one(crate::store::work_schema::query(super::workspace(doc! {
            "_id":workspace.id.as_str(),"document_schema_version":DOCUMENT_SCHEMA_VERSION,
            "revision":signed(expected,"Workspace publication revision").map_err(SandboxStoreError::Corrupt)?,
            "channel_reuse_cleanup_execution":{"$exists":false},
            "owned_workspace_cleanup_admission_v1":{"$exists":false},
        }, workspace.id.as_str())?), crate::store::work_lease_update::replacement(
            crate::store::workspaces::workspace_replacement_fields(workspace)?))
            .session(&mut session).await.map_err(unavailable)?;
        if result.matched_count != 1 { return Err(SandboxStoreError::Conflict); }
        Ok(())
    }.await;
    if let Err(error) = outcome {
        if let Err(abort) = session.abort_guarded().await {
            tracing::warn!(error = %abort, "Workspace publication abort was not acknowledged");
        }
        return Err(error);
    }
    match commit_transaction(&mut session).await {
        CommitOutcome::Committed => Ok(()),
        // The next attempt reconciles the exact content. An uncertain commit
        // is never converted to Conflict, which could authorize another effect.
        CommitOutcome::Retry(error)
        | CommitOutcome::Unknown(error)
        | CommitOutcome::Failed(error) => Err(unavailable(error)),
    }
}

fn unavailable(error: mongodb::error::Error) -> SandboxStoreError {
    SandboxStoreError::Unavailable(error.to_string())
}

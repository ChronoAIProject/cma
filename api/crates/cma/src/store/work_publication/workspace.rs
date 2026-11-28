//! A worker's execution-policy save fences its parent in the same transaction.

use crate::store::transaction_terminal::GuardedTerminal as _;
use layer2_core::sandbox::{
    store::SandboxStoreError, Workspace, WorkspaceStatus, WorkspaceWorkStep,
};
use mongodb::{bson::doc, options::ReadConcern};

use super::{retry_workspace_write, workspace_deadline, FenceError};

use crate::store::sandbox_mongo::{
    commit_transaction, signed, start_transaction, CommitOutcome, MongoSandboxStore,
    DOCUMENT_SCHEMA_VERSION,
};

pub(in crate::store) async fn save(
    store: &MongoSandboxStore,
    current: &Workspace,
    workspace: &Workspace,
) -> Result<(), SandboxStoreError> {
    let observe = store.workspace_authority_retry
        && current.work.authority_protocol.is_some()
        && current.status == WorkspaceStatus::Preparing
        && matches!(
            current.work.step,
            WorkspaceWorkStep::Prepare | WorkspaceWorkStep::Setup
        );
    let pin = observe
        && current.work.step == workspace.work.step
        && current.work.execution.is_none()
        && workspace.work.execution.is_some();
    let deadline = if pin {
        workspace_deadline(current)?
    } else {
        None
    };
    retry_workspace_write(deadline, "pre_effect_pin", || {
        save_once(store, current.revision, workspace, observe)
    })
    .await
}

async fn save_once(
    store: &MongoSandboxStore,
    expected: u64,
    workspace: &Workspace,
    observe_unknown: bool,
) -> Result<(), FenceError> {
    let mut session = store.client.start_session().await?;
    start_transaction(&mut session).await?;
    let outcome = async {
        super::fence_transaction(store, &mut session).await?;
        let result = store.workspaces.update_one(crate::store::work_schema::query(super::workspace(doc! {
            "_id":workspace.id.as_str(),"document_schema_version":DOCUMENT_SCHEMA_VERSION,
            "revision":signed(expected,"Workspace publication revision").map_err(SandboxStoreError::Corrupt)?,
            "channel_reuse_cleanup_execution":{"$exists":false},
            "owned_workspace_cleanup_admission_v1":{"$exists":false},
        }, workspace.id.as_str())?), crate::store::work_lease_update::replacement(
            crate::store::workspaces::workspace_replacement_fields(workspace)?))
            .session(&mut session).await?;
        if result.matched_count != 1 { return Err(SandboxStoreError::Conflict.into()); }
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
        CommitOutcome::Retry(error) => Err(FenceError::Driver(error)),
        CommitOutcome::Unknown(error) => {
            if observe_unknown {
                let durable_target_observed = observe_publication(store, workspace).await;
                tracing::warn!(
                    workspace_id = %workspace.id,
                    target_step = ?workspace.work.step,
                    target_revision = workspace.revision,
                    durable_target_observed,
                    "Workspace publication commit remained unknown"
                );
            }
            // Readback is evidence for recovery. The current caller must not
            // dispatch another effect after an unresolved commit.
            Err(SandboxStoreError::Unavailable(error.to_string()).into())
        }
        CommitOutcome::Failed(error) => {
            Err(SandboxStoreError::Unavailable(error.to_string()).into())
        }
    }
}

async fn observe_publication(store: &MongoSandboxStore, workspace: &Workspace) -> bool {
    let stored = store
        .workspaces
        .find_one(doc! {"_id":workspace.id.as_str()})
        .read_concern(ReadConcern::majority())
        .await
        .ok()
        .flatten()
        .and_then(|row| crate::store::workspaces::read_workspace(&row).ok());
    crate::store::work_lease_update::workspace_matches(stored.as_ref(), workspace)
}

//! Explicit original-Workspace recovery under the same fences as child admission.
use super::{
    sandbox_mongo::{signed, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION},
    sandboxes::read_sandbox,
    workspace_admission::{lock_sandbox, ApplyError},
    workspaces::{read_workspace, workspace_replacement_fields},
};
use crate::store::transaction_terminal::GuardedTerminal as _;
use layer2_core::sandbox::{store::SandboxStoreError, Workspace};
use layer2_core::{Caller, WorkspaceId};
use mongodb::bson::doc;

pub(super) async fn apply(
    store: &MongoSandboxStore,
    id: &WorkspaceId,
    caller: &Caller,
    operation_id: &str,
    now: &str,
    session: &mut mongodb::ClientSession,
) -> Result<Workspace, ApplyError> {
    let document = store
        .workspaces
        .find_one(doc! {"_id":id.as_str()})
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
        .ok_or(ApplyError::Domain(SandboxStoreError::NotFound))?;
    let mut workspace = read_workspace(&document).map_err(ApplyError::Domain)?;
    if caller.subject != workspace.owner
        && !caller
            .authorization
            .allows(layer2_core::Capability::ControlAnySandbox)
    {
        return Err(ApplyError::Domain(SandboxStoreError::NotFound));
    }
    lock_sandbox(store, workspace.sandbox_id.as_str(), session).await?;
    let parent = store
        .sandboxes
        .find_one(doc! {"_id":workspace.sandbox_id.as_str()})
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
        .as_ref()
        .map(read_sandbox)
        .transpose()
        .map_err(ApplyError::Domain)?
        .ok_or(ApplyError::Domain(SandboxStoreError::NotFound))?;
    let filter = doc! {"_id":id.as_str(),"document_schema_version":DOCUMENT_SCHEMA_VERSION,
    "revision":signed(workspace.revision,"workspace recovery revision")
        .map_err(|e|ApplyError::Domain(SandboxStoreError::Corrupt(e)))?,
    "channel_reuse_cleanup_execution":{"$exists":false},
    "owned_workspace_cleanup_admission_v1":{"$exists":false}};
    let locked = store
        .workspaces
        .update_one(
            crate::store::work_schema::query(filter.clone()),
            doc! {"$inc":{"removal_lock_fence":1_i64}},
        )
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?;
    if locked.matched_count != 1 {
        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
    }
    let has_agents = store
        .agents
        .find_one(doc! {"workspace_id":id.as_str()})
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
        .is_some();
    let has_worktrees = store
        .worktrees
        .find_one(doc! {"workspace_id":id.as_str()})
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
        .is_some();
    workspace
        .admit_prepare_recovery(
            &parent,
            caller,
            operation_id,
            has_agents,
            has_worktrees,
            now.into(),
        )
        .map_err(ApplyError::Domain)?;
    let changed = store
        .workspaces
        .update_one(
            crate::store::work_schema::query(filter),
            crate::store::work_lease_update::replacement(
                workspace_replacement_fields(&workspace).map_err(ApplyError::Domain)?,
            ),
        )
        .session(session)
        .await
        .map_err(ApplyError::Driver)?;
    if changed.matched_count != 1 {
        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
    }
    Ok(workspace)
}

pub(super) async fn begin(
    store: &MongoSandboxStore,
    id: &WorkspaceId,
    caller: &Caller,
    operation_id: &str,
    now: String,
) -> Result<Workspace, SandboxStoreError> {
    use super::sandbox_mongo::{
        commit_transaction, is_transient, start_transaction, CommitOutcome,
    };
    use super::workspace_admission::{
        reconcile_workspace,
        retry::{exhausted, retry_delay},
        TRANSACTION_ATTEMPTS,
    };
    let mut last_transient = None;
    for attempt in 0..TRANSACTION_ATTEMPTS {
        retry_delay(attempt).await;
        let mut session = store
            .client
            .start_session()
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
        if let Err(error) = start_transaction(&mut session).await {
            if is_transient(&error) {
                last_transient = Some(error.to_string());
                continue;
            }
            return Err(SandboxStoreError::Unavailable(error.to_string()));
        }
        let workspace = match super::workspace_prepare_recovery::apply(
            store,
            id,
            caller,
            operation_id,
            &now,
            &mut session,
        )
        .await
        {
            Ok(workspace) => workspace,
            Err(ApplyError::Domain(error)) => {
                let _ = session.abort_guarded().await;
                return Err(error);
            }
            Err(ApplyError::Retry(error)) | Err(ApplyError::Driver(error))
                if is_transient(&error) =>
            {
                let _ = session.abort_guarded().await;
                last_transient = Some(error.to_string());
                continue;
            }
            Err(ApplyError::Retry(error)) => {
                let _ = session.abort_guarded().await;
                last_transient = Some(error.to_string());
                continue;
            }
            Err(ApplyError::Driver(error)) => {
                let _ = session.abort_guarded().await;
                return Err(SandboxStoreError::Unavailable(error.to_string()));
            }
        };
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => return Ok(workspace),
            CommitOutcome::Retry(error) => last_transient = Some(error.to_string()),
            CommitOutcome::Unknown(error) => {
                return if reconcile_workspace(store, &workspace).await? {
                    Ok(workspace)
                } else {
                    Err(SandboxStoreError::Unavailable(format!(
                        "workspace prepare recovery admission commit remained unknown: {error}"
                    )))
                };
            }
            CommitOutcome::Failed(error) => {
                return Err(SandboxStoreError::Unavailable(error.to_string()));
            }
        }
    }
    Err(exhausted(
        "workspace prepare recovery admission",
        last_transient,
    ))
}

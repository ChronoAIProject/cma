//! The original Workspace save predicates, under the parent/child transaction.

use layer2_core::sandbox::{store::SandboxStoreError, ProvisioningExecution, Workspace};
use mongodb::{bson::doc, ClientSession};

use crate::store::sandbox_events::TransactionEventError as Error;
use crate::store::sandbox_mongo::{signed, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION};
use crate::store::workspaces::{read_workspace, workspace_replacement_fields};

pub(super) async fn apply(
    store: &MongoSandboxStore,
    expected_revision: u64,
    workspace: &Workspace,
    session: &mut ClientSession,
) -> Result<(), Error> {
    let current = store
        .workspaces
        .find_one(doc! {"_id": workspace.id.as_str()})
        .session(&mut *session)
        .await
        .map_err(Error::Driver)?
        .ok_or(Error::Domain(SandboxStoreError::NotFound))?;
    let current = read_workspace(&current).map_err(Error::Domain)?;
    if current.revision != expected_revision
        || current.id != workspace.id
        || current.owner != workspace.owner
        || current.sandbox_id != workspace.sandbox_id
        || current.created_at != workspace.created_at
        || Some(workspace.revision) != expected_revision.checked_add(1)
    {
        return Err(Error::Domain(SandboxStoreError::Conflict));
    }
    ProvisioningExecution::validate_successor(current.work.execution, workspace.work.execution)
        .map_err(|_| Error::Domain(SandboxStoreError::Conflict))?;
    super::fence_parent(store, &current.sandbox_id, &current.owner, session).await?;
    let result = store
        .workspaces
        .update_one(
            doc! {
                "_id": workspace.id.as_str(),
                "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                "channel_reuse_cleanup_execution": { "$exists": false },
                "owned_workspace_cleanup_admission_v1": { "$exists": false },
                "revision": signed(expected_revision, "expected workspace revision")
                    .map_err(|_| Error::Domain(SandboxStoreError::Conflict))?,
            },
            doc! {"$set": workspace_replacement_fields(workspace).map_err(Error::Domain)?},
        )
        .session(&mut *session)
        .await
        .map_err(Error::Driver)?;
    if result.matched_count != 1 {
        return Err(Error::Domain(SandboxStoreError::Conflict));
    }
    Ok(())
}

use super::{read_sandbox, ApplyError, MongoSandboxStore, SandboxStoreError};
use layer2_core::sandbox::{SandboxStatus, Workspace, WorkspaceStatus};
use mongodb::{bson::doc, ClientSession};

pub(super) async fn check_parent(
    store: &MongoSandboxStore,
    session: &mut ClientSession,
    current: &Workspace,
    next: &Workspace,
) -> Result<(), ApplyError> {
    if next.status != WorkspaceStatus::Ready {
        return Ok(());
    }
    if current.status != WorkspaceStatus::Preparing
        || current.owner != next.owner
        || current.sandbox_id != next.sandbox_id
    {
        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
    }
    let document = store
        .sandboxes
        .find_one(doc! { "_id": current.sandbox_id.as_str() })
        .session(session)
        .await
        .map_err(ApplyError::Driver)?
        .ok_or(ApplyError::Domain(SandboxStoreError::NotFound))?;
    let parent = read_sandbox(&document).map_err(ApplyError::Domain)?;
    if parent.owner != current.owner
        || !matches!(
            parent.status,
            SandboxStatus::Installing | SandboxStatus::Active
        )
    {
        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
    }
    // The Ready event updates this same Sandbox's event head in this transaction.
    // If retirement commits first, that write conflicts and retries reread the parent.
    Ok(())
}

//! Fence the independent aggregate/event transaction, not just generic saves.
//! Existing validation, authorization receipts and commit reconciliation remain
//! owned by the transaction. These checks grant no external execution authority.

use layer2_core::sandbox::ledger_maintenance::LedgerWriterClass;
use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::{Agent, Sandbox, Workspace, Worktree};
use layer2_core::{SandboxId, SubjectId};
use mongodb::{bson::doc, ClientSession};

use super::{ApplyError, MongoSandboxStore};

pub(super) async fn creation(
    store: &MongoSandboxStore,
    worktree: &Worktree,
    agent: &Agent,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    let row = store
        .workspaces
        .find_one(doc! {"_id":worktree.workspace_id.as_str()})
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
        .ok_or_else(|| ApplyError::Domain(SandboxStoreError::NotFound))?;
    let current = crate::store::workspaces::read_workspace(&row).map_err(ApplyError::Domain)?;
    if current.id != worktree.workspace_id
        || current.sandbox_id != worktree.sandbox_id
        || current.owner != worktree.owner
        || agent.workspace_id != current.id
        || agent.sandbox_id != current.sandbox_id
        || agent.owner != current.owner
    {
        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
    }
    parent(
        store,
        &current.sandbox_id,
        &current.owner,
        LedgerWriterClass::Creation,
        session,
    )
    .await
}

pub(super) async fn sandbox(
    store: &MongoSandboxStore,
    current: &Sandbox,
    next: &Sandbox,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    if current.id != next.id
        || current.owner != next.owner
        || current.created_at != next.created_at
        || current.origin != next.origin
    {
        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
    }
    parent(
        store,
        &current.id,
        &current.owner,
        LedgerWriterClass::Administration,
        session,
    )
    .await
}

pub(super) async fn workspace(
    store: &MongoSandboxStore,
    current: &Workspace,
    next: &Workspace,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    if current.id != next.id
        || current.sandbox_id != next.sandbox_id
        || current.owner != next.owner
        || current.created_at != next.created_at
    {
        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
    }
    parent(
        store,
        &current.sandbox_id,
        &current.owner,
        LedgerWriterClass::Administration,
        session,
    )
    .await
}

async fn parent(
    store: &MongoSandboxStore,
    id: &SandboxId,
    owner: &SubjectId,
    class: LedgerWriterClass,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    let row = store
        .sandboxes
        .find_one(doc! {"_id":id.as_str()})
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
        .ok_or_else(|| ApplyError::Domain(SandboxStoreError::NotFound))?;
    let current = crate::store::sandboxes::read_sandbox(&row).map_err(ApplyError::Domain)?;
    if current.id != *id || current.owner != *owner {
        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
    }
    crate::store::ledger_maintenance_writer_ports::parent_write(store, id, class, session).await?;
    Ok(())
}

#[cfg(test)]
#[path = "ledger_maintenance_aggregate_tests.rs"]
mod tests;

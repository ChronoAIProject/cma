//! Creation admission adapters. The integration owner wires these into the
//! existing transaction; an observed or replayed result grants no new effects.

use layer2_core::sandbox::ledger_maintenance::LedgerWriterClass;
use layer2_core::sandbox::ledger_maintenance_admission::LedgerMaintenanceAdmissionRefusal;
use layer2_core::sandbox::store::ledger_maintenance::LedgerMaintenanceStoreError;
use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::{SandboxId, SubjectId, WorkspaceId};
use mongodb::{bson::doc, ClientSession};

use super::ledger_maintenance::ApplyError;
use super::sandbox_mongo::MongoSandboxStore;

pub(super) enum CreationError {
    Domain(SandboxStoreError),
    Driver(mongodb::error::Error),
}

impl From<mongodb::error::Error> for CreationError {
    fn from(error: mongodb::error::Error) -> Self {
        Self::Driver(error)
    }
}

/// The binding fence makes even historical replay a mutation. Unbound replay
/// remains pure, including the accepted common-launch receipt path; a new
/// initiation must separately fence before its attribution write.
pub(super) async fn before_binding_replay(
    store: &MongoSandboxStore,
    admission: &layer2_core::sandbox::store::AgentAdmission,
    session: &mut ClientSession,
) -> Result<(), CreationError> {
    if admission.binding.is_none() {
        return Ok(());
    }
    fence_workspace(store, &admission.workspace_id, &admission.owner, session).await
}

/// Use after the caller's current ownership/lifecycle checks and before any
/// adoption, reuse or creation write, inside that same transaction.
pub(super) async fn fence(
    store: &MongoSandboxStore,
    sandbox: &SandboxId,
    session: &mut ClientSession,
) -> Result<(), CreationError> {
    super::ledger_maintenance_writer::fence_writer(
        store,
        sandbox,
        LedgerWriterClass::Creation,
        None,
        session,
    )
    .await
    .map_err(|error| match error {
        ApplyError::Driver(error) => CreationError::Driver(error),
        ApplyError::Domain(error) => CreationError::Domain(match error {
            LedgerMaintenanceStoreError::WriterRefused(_) => SandboxStoreError::Conflict,
            LedgerMaintenanceStoreError::Refused(LedgerMaintenanceAdmissionRefusal::NotFound) => {
                SandboxStoreError::SandboxNotReady
            }
            LedgerMaintenanceStoreError::Corrupt => {
                SandboxStoreError::Corrupt("ledger maintenance authority is inconsistent".into())
            }
            _ => {
                SandboxStoreError::Unavailable("ledger maintenance authority is unavailable".into())
            }
        }),
    })
}

/// Bound/attributed Agent replay may write a binding or initiation fence before
/// reaching ordinary creation. Resolve its current owned home in the same
/// transaction so hold refusal also rolls back those early authority writes.
pub(super) async fn fence_workspace(
    store: &MongoSandboxStore,
    workspace_id: &WorkspaceId,
    owner: &SubjectId,
    session: &mut ClientSession,
) -> Result<(), CreationError> {
    let row = store
        .workspaces
        .find_one(doc! {"_id": workspace_id.as_str()})
        .session(&mut *session)
        .await?
        .ok_or(CreationError::Domain(SandboxStoreError::WorkspaceNotReady))?;
    let workspace = super::workspaces::read_workspace(&row).map_err(CreationError::Domain)?;
    if workspace.id != *workspace_id || workspace.owner != *owner {
        return Err(CreationError::Domain(SandboxStoreError::WorkspaceNotReady));
    }
    let row = store
        .sandboxes
        .find_one(doc! {"_id": workspace.sandbox_id.as_str()})
        .session(&mut *session)
        .await?
        .ok_or(CreationError::Domain(SandboxStoreError::SandboxNotReady))?;
    let sandbox = super::sandboxes::read_sandbox(&row).map_err(CreationError::Domain)?;
    if sandbox.id != workspace.sandbox_id || sandbox.owner != *owner {
        return Err(CreationError::Domain(SandboxStoreError::WorkspaceNotReady));
    }
    fence(store, &sandbox.id, session).await
}

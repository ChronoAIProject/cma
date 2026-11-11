//! Error adapters for existing transactional writer ports. Shared call sites
//! remain integration-owned; no new request path or activation flag is added.

use layer2_core::sandbox::agent_io::CommandLedgerError;
use layer2_core::sandbox::ledger_maintenance::LedgerWriterClass;
use layer2_core::sandbox::ledger_maintenance_admission::LedgerMaintenanceAdmissionRefusal;
use layer2_core::sandbox::store::ledger_maintenance::LedgerMaintenanceStoreError;
use layer2_core::sandbox::store::AgentControlStoreError;
use layer2_core::SandboxId;
use mongodb::ClientSession;

use super::agent_commands::ApplyError as CommandError;
use super::agent_control_lineage::LineageError;
use super::ledger_maintenance::ApplyError;
use super::sandbox_mongo::MongoSandboxStore;

pub(super) async fn command(
    store: &MongoSandboxStore,
    sandbox: &SandboxId,
    class: LedgerWriterClass,
    session: &mut ClientSession,
) -> Result<(), CommandError> {
    super::ledger_maintenance_writer::fence_writer(store, sandbox, class, None, session)
        .await
        .map_err(|error| match error {
            ApplyError::Driver(error) => CommandError::Driver(error),
            ApplyError::Domain(error) => CommandError::Domain(match error {
                LedgerMaintenanceStoreError::WriterRefused(_) => CommandLedgerError::NotAdmissible,
                LedgerMaintenanceStoreError::Refused(
                    LedgerMaintenanceAdmissionRefusal::NotFound,
                ) => CommandLedgerError::NotFound,
                LedgerMaintenanceStoreError::Corrupt => CommandLedgerError::Corrupt,
                _ => CommandLedgerError::Unavailable,
            }),
        })
}

pub(super) async fn control(
    store: &MongoSandboxStore,
    sandbox: &SandboxId,
    session: &mut ClientSession,
) -> Result<(), LineageError> {
    super::ledger_maintenance_writer::fence_writer(
        store,
        sandbox,
        LedgerWriterClass::OwnerControl,
        None,
        session,
    )
    .await
    .map_err(|error| match error {
        ApplyError::Driver(error) => LineageError::Driver(error),
        ApplyError::Domain(error) => LineageError::Domain(match error {
            LedgerMaintenanceStoreError::WriterRefused(_) => AgentControlStoreError::Conflict,
            LedgerMaintenanceStoreError::Refused(LedgerMaintenanceAdmissionRefusal::NotFound) => {
                AgentControlStoreError::NotFound
            }
            LedgerMaintenanceStoreError::Corrupt => AgentControlStoreError::Corrupt(
                "ledger maintenance authority is inconsistent".into(),
            ),
            _ => AgentControlStoreError::Unavailable(
                "ledger maintenance authority is unavailable".into(),
            ),
        }),
    })
}

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

/// Resolve custody in the writer transaction. An orphan command cannot prove
/// that its original home is unheld, even when its lease remains valid.
pub(super) async fn command_agent(
    store: &MongoSandboxStore,
    agent_id: &layer2_core::AgentId,
    session: &mut ClientSession,
) -> Result<(), CommandError> {
    let agent = command_agent_record(store, agent_id, session).await?;
    command(
        store,
        &agent.sandbox_id,
        LedgerWriterClass::AgentWorker,
        session,
    )
    .await
}

/// Bound command replay has already written its binding authorization fence.
/// Preserve that serialization and roll it back if the actual Agent's home is
/// held. Ordinary unbound replay remains an immutable result observation.
pub(super) async fn bound_command_replay(
    store: &MongoSandboxStore,
    request: &layer2_core::sandbox::agent_io::CommandReservation,
    session: &mut ClientSession,
) -> Result<(), CommandError> {
    let Some(proof) = &request.binding else {
        return Ok(());
    };
    let agent = command_agent_record(store, &request.envelope.agent_id, session).await?;
    if agent.sandbox_id != proof.conversation.sandbox_id
        || agent.workspace_id != proof.conversation.workspace_id
        || agent.worktree_id != proof.conversation.worktree_id
        || agent.owner != proof.binding.owner
    {
        return Err(CommandLedgerError::Corrupt.into());
    }
    let class = if matches!(
        request.envelope.command,
        layer2_core::sandbox::agent_io::AgentCommand::Message { .. }
    ) {
        LedgerWriterClass::OwnerInput
    } else {
        LedgerWriterClass::OwnerControl
    };
    command(store, &agent.sandbox_id, class, session).await
}

async fn command_agent_record(
    store: &MongoSandboxStore,
    agent_id: &layer2_core::AgentId,
    session: &mut ClientSession,
) -> Result<layer2_core::sandbox::Agent, CommandError> {
    let document = store
        .agents
        .find_one(mongodb::bson::doc! {"_id":agent_id.as_str()})
        .session(&mut *session)
        .await?
        .ok_or(CommandLedgerError::NotFound)?;
    let agent = super::agents::read_agent(&document).map_err(|_| CommandLedgerError::Corrupt)?;
    if agent.id != *agent_id {
        return Err(CommandLedgerError::Corrupt.into());
    }
    Ok(agent)
}

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

/// Generic parent-save admission. This legacy administrative port may publish
/// lifecycle or credential state; it cannot bypass either hold format. More
/// specialized writers still require their own guard in the owning transaction.
pub(super) async fn parent_save(
    store: &MongoSandboxStore,
    sandbox: &SandboxId,
    session: &mut ClientSession,
) -> Result<(), super::sandbox_events::TransactionEventError> {
    use super::sandbox_events::TransactionEventError as EventError;
    use layer2_core::sandbox::store::SandboxStoreError;
    super::ledger_maintenance_writer::fence_writer(
        store,
        sandbox,
        LedgerWriterClass::Administration,
        None,
        session,
    )
    .await
    .map_err(|error| match error {
        ApplyError::Driver(error) => EventError::Driver(error),
        ApplyError::Domain(error) => EventError::Domain(match error {
            LedgerMaintenanceStoreError::WriterRefused(_) => SandboxStoreError::Conflict,
            LedgerMaintenanceStoreError::Refused(LedgerMaintenanceAdmissionRefusal::NotFound) => {
                SandboxStoreError::NotFound
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

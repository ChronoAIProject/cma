use layer2_core::sandbox::agent_io::{
    validate_command_reservation_policy, AdmittedAgentCommand, CommandLedgerError,
    QueuedCancellationReservation,
};
use layer2_core::sandbox::{Agent, Sandbox};
use mongodb::{bson::doc, ClientSession};

use super::super::ApplyError;
use crate::store::sandbox_mongo::{signed, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION};
use crate::store::{agents::read_agent, sandboxes::read_sandbox};

pub(in crate::store::agent_commands) async fn current(
    store: &MongoSandboxStore,
    request: &QueuedCancellationReservation,
    session: &mut ClientSession,
) -> Result<(Agent, Sandbox), ApplyError> {
    store
        .bindings
        .conversation_authority(request.command.binding.as_ref(), session)
        .await?;
    let agent = store
        .agents
        .find_one(doc! {"_id": request.command.envelope.agent_id.as_str()})
        .session(&mut *session)
        .await?
        .as_ref()
        .map(read_agent)
        .transpose()
        .map_err(|_| CommandLedgerError::Corrupt)?
        .ok_or(CommandLedgerError::NotFound)?;
    let parent = store
        .sandboxes
        .find_one(doc! {"_id": agent.sandbox_id.as_str()})
        .session(&mut *session)
        .await?
        .as_ref()
        .map(read_sandbox)
        .transpose()
        .map_err(|_| CommandLedgerError::Corrupt)?
        .ok_or(CommandLedgerError::NotFound)?;
    request.validate_current(&agent, &parent)?;
    let policy = store
        .resource_policies
        .find_one(doc! {"_id": parent.id.as_str()})
        .session(&mut *session)
        .await?
        .as_ref()
        .map(crate::store::resource_access::read_inventory_policy)
        .transpose()
        .map_err(|_| CommandLedgerError::Corrupt)?;
    validate_command_reservation_policy(&request.command, &parent, policy.as_ref())?;
    Ok((agent, parent))
}

pub(in crate::store::agent_commands) async fn resources(
    store: &MongoSandboxStore,
    agent: &Agent,
    parent: &Sandbox,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    for (collection, id, revision, created_at) in [
        (
            &store.agents,
            agent.id.as_str(),
            agent.revision,
            &agent.created_at,
        ),
        (
            &store.sandboxes,
            parent.id.as_str(),
            parent.revision,
            &parent.created_at,
        ),
    ] {
        let revision =
            signed(revision, "resource revision").map_err(|_| CommandLedgerError::Corrupt)?;
        let changed = collection.update_one(doc! {"_id": id, "document_schema_version": DOCUMENT_SCHEMA_VERSION, "revision": revision, "record.created_at": created_at}, doc! {"$inc": {"agent_command_admission_fence": 1_i64}}).session(&mut *session).await?;
        if changed.matched_count != 1 {
            return Err(CommandLedgerError::Conflict.into());
        }
    }
    Ok(())
}

pub(in crate::store::agent_commands) async fn source(
    store: &MongoSandboxStore,
    response: &AdmittedAgentCommand,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    let revision = signed(response.revision, "source command revision")
        .map_err(|_| CommandLedgerError::Corrupt)?;
    let changed = store.agent_commands.update_one(doc! {"_id": response.receipt.envelope.command_id.as_str(), "document_schema_version": DOCUMENT_SCHEMA_VERSION, "revision": revision}, doc! {"$inc": {"queued_cancellation_fence": 1_i64}}).session(session).await?;
    if changed.matched_count != 1 {
        return Err(CommandLedgerError::Conflict.into());
    }
    Ok(())
}

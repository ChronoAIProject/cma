use layer2_core::sandbox::agent_io::{
    validate_command_reservation_policy, AdmittedAgentCommand, CommandLedgerError,
    QueuedCancellationReservation,
};
use layer2_core::sandbox::{Agent, Sandbox};
use mongodb::{bson::doc, ClientSession};

use super::super::ApplyError;
use crate::store::sandbox_mongo::{signed, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION};
use crate::store::work_claim_authority::{without_ledger_maintenance, LEDGER_MAINTENANCE_FIELD};
use crate::store::{agents::read_agent, sandboxes::read_sandbox};

pub(in crate::store::agent_commands) async fn current(
    store: &MongoSandboxStore,
    request: &QueuedCancellationReservation,
    exact_replay: bool,
    session: &mut ClientSession,
) -> Result<(Agent, Sandbox, bool), ApplyError> {
    current_with_observation(store, request, exact_replay, session)
        .await?
        .ok_or_else(|| CommandLedgerError::Conflict.into())
}

pub(in crate::store::agent_commands) async fn current_with_observation(
    store: &MongoSandboxStore,
    request: &QueuedCancellationReservation,
    exact_replay: bool,
    session: &mut ClientSession,
) -> Result<Option<(Agent, Sandbox, bool)>, ApplyError> {
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
    let parent_row = store
        .sandboxes
        .find_one(doc! {"_id": agent.sandbox_id.as_str()})
        .session(&mut *session)
        .await?
        .ok_or(CommandLedgerError::NotFound)?;
    let parent = read_sandbox(&parent_row).map_err(|_| CommandLedgerError::Corrupt)?;
    let held = parent_row.contains_key(LEDGER_MAINTENANCE_FIELD);
    let proof = request.command.binding.as_ref();
    if held {
        if !exact_replay {
            return Err(CommandLedgerError::Conflict.into());
        }
        store
            .bindings
            .conversation_authority_read_only(proof, session)
            .await?;
    } else {
        store
            .bindings
            .conversation_authority(proof, session)
            .await?;
    }
    request.validate_current_identity(&agent, &parent)?;
    if !request.current_observation_matches(&agent, &parent) {
        return Ok(None);
    }
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
    Ok(Some((agent, parent, held)))
}

pub(in crate::store::agent_commands) async fn resources(
    store: &MongoSandboxStore,
    agent: &Agent,
    parent: &Sandbox,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    // Both supported Agent envelopes were decoded in this same transaction.
    // The Sandbox retains its own format; adoption must not disable cancellation.
    let agent_revision =
        signed(agent.revision, "Agent revision").map_err(|_| CommandLedgerError::Corrupt)?;
    let parent_revision =
        signed(parent.revision, "Sandbox revision").map_err(|_| CommandLedgerError::Corrupt)?;
    for (collection, filter) in [
        (
            &store.agents,
            doc! {
                "_id": agent.id.as_str(),
                "document_schema_version": {"$in": [1_i64, 2_i64]},
                "revision": agent_revision,
                "record.created_at": &agent.created_at,
            },
        ),
        (
            &store.sandboxes,
            without_ledger_maintenance(doc! {
                "_id": parent.id.as_str(),
                "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                "revision": parent_revision,
                "record.created_at": &parent.created_at,
            }),
        ),
    ] {
        let changed = collection
            .update_one(
                filter,
                doc! {"$inc": {"agent_command_admission_fence": 1_i64}},
            )
            .session(&mut *session)
            .await?;
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

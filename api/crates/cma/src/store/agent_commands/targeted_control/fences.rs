use layer2_core::sandbox::agent_io::{
    validate_command_reservation_policy, AdmittedAgentCommand, CommandLedgerError,
    TargetedControlReservation,
};
use layer2_core::sandbox::{Agent, Sandbox};
use mongodb::{bson::doc, ClientSession};

use super::super::ApplyError;
use super::trace_rejection;
use crate::store::agent_capacity_recovery::source::fence_filter;
use crate::store::agent_log_integrity::TransactionLogError;
use crate::store::agents::read_agent;
use crate::store::sandbox_mongo::{
    record_bson, signed, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION,
};
use crate::store::sandboxes::read_sandbox;

pub(super) async fn current(
    store: &MongoSandboxStore,
    request: &TargetedControlReservation,
    session: &mut ClientSession,
) -> Result<(Agent, Sandbox), ApplyError> {
    store
        .bindings
        .conversation_authority(request.command.binding.as_ref(), session)
        .await?;
    let agent = store
        .agents
        .find_one(doc! { "_id": request.command.envelope.agent_id.as_str() })
        .session(&mut *session)
        .await?
        .as_ref()
        .map(read_agent)
        .transpose()
        .map_err(|_| CommandLedgerError::Corrupt)?
        .ok_or(CommandLedgerError::NotFound)?;
    let parent = store
        .sandboxes
        .find_one(doc! { "_id": agent.sandbox_id.as_str() })
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
        .find_one(doc! { "_id": parent.id.as_str() })
        .session(&mut *session)
        .await?
        .as_ref()
        .map(crate::store::resource_access::read_inventory_policy)
        .transpose()
        .map_err(|_| CommandLedgerError::Corrupt)?;
    validate_command_reservation_policy(&request.command, &parent, policy.as_ref())?;
    Ok((agent, parent))
}

pub(super) async fn resources(
    store: &MongoSandboxStore,
    request: &TargetedControlReservation,
    agent: &Agent,
    parent: &Sandbox,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    let mut agent_filter = fence_filter(store, agent, session).await.map_err(|error| {
        trace_rejection(
            request,
            "resources_agent",
            match error {
                TransactionLogError::Domain(_) => ApplyError::Domain(CommandLedgerError::Corrupt),
                TransactionLogError::Driver(error) => ApplyError::Driver(error),
            },
        )
    })?;
    agent_filter.insert("record.created_at", &agent.created_at);
    let result = store
        .agents
        .update_one(
            agent_filter,
            doc! { "$inc": { "agent_command_admission_fence": 1_i64 } },
        )
        .session(&mut *session)
        .await
        .map_err(|error| trace_rejection(request, "resources_agent", error.into()))?;
    if result.matched_count != 1 {
        return Err(trace_rejection(
            request,
            "resources_agent",
            CommandLedgerError::Conflict.into(),
        ));
    }
    let parent_revision =
        signed(parent.revision, "resource revision").map_err(|_| CommandLedgerError::Corrupt)?;
    let result = store
        .sandboxes
        .update_one(
            doc! {
                "_id": parent.id.as_str(),
                "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                "revision": parent_revision,
                "record.created_at": &parent.created_at,
            },
            doc! { "$inc": { "agent_command_admission_fence": 1_i64 } },
        )
        .session(&mut *session)
        .await
        .map_err(|error| trace_rejection(request, "resources_parent", error.into()))?;
    if result.matched_count != 1 {
        return Err(trace_rejection(
            request,
            "resources_parent",
            CommandLedgerError::Conflict.into(),
        ));
    }
    Ok(())
}

pub(super) async fn response(
    store: &MongoSandboxStore,
    response: &AdmittedAgentCommand,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    let revision =
        signed(response.revision, "response revision").map_err(|_| CommandLedgerError::Corrupt)?;
    let dispatch = record_bson(&response.work.dispatch).map_err(|_| CommandLedgerError::Corrupt)?;
    let result = store
        .agent_commands
        .update_one(
            doc! {
                "_id": response.receipt.envelope.command_id.as_str(),
                "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                "revision": revision,
                "agent_id": response.receipt.envelope.agent_id.as_str(),
                "response_id": response.receipt.response_id.as_ref().map(|id| id.as_str()),
                "record.work.dispatch": dispatch,
            },
            doc! { "$inc": { "explicit_control_admission_fence": 1_i64 } },
        )
        .session(&mut *session)
        .await?;
    if result.matched_count != 1 {
        return Err(CommandLedgerError::Conflict.into());
    }
    Ok(())
}

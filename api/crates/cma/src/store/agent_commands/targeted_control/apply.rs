use crate::store::agent_log_telemetry::ScanCaller;
use layer2_core::sandbox::agent_io::{
    AgentCommand, CommandLedgerError, CommandReservationResult, TargetedControlBinding,
    TargetedControlInspection, TargetedControlReservation,
};
use layer2_core::sandbox::PendingAgentApprovals;
use mongodb::{
    bson::{doc, Document},
    ClientSession,
};

use super::super::{records, reserve, ApplyError};
use super::{fences, trace_rejection, Mode};
use crate::store::agent_log::scan_complete_log_with_session;
use crate::store::agent_log_integrity::TransactionLogError;
use crate::store::sandbox_mongo::{record_bson, signed, MongoSandboxStore};

pub(super) async fn apply(
    store: &MongoSandboxStore,
    request: &TargetedControlReservation,
    mode: Mode,
    session: &mut ClientSession,
) -> Result<TargetedControlInspection, ApplyError> {
    let existing = store
        .agent_commands
        .find_one(doc! {
            "scope": request.command.scope(), "key_sha256": &request.command.key_sha256,
        })
        .session(&mut *session)
        .await
        .map_err(|error| trace_rejection(request, "existing_lookup", error.into()))?
        .as_ref()
        .map(|document| request.replay(records::read(document)?, binding(document)?))
        .transpose()?;
    let (agent, parent) = fences::current(store, request, session)
        .await
        .map_err(|error| trace_rejection(request, "current", error))?;
    if let Some(existing) = existing {
        let response = store
            .agent_commands
            .find_one(doc! { "_id": existing.binding.response_command_id.as_str() })
            .session(&mut *session)
            .await
            .map_err(|error| trace_rejection(request, "existing_response", error.into()))?
            .as_ref()
            .map(records::read)
            .transpose()?
            .ok_or(CommandLedgerError::Corrupt)?;
        existing
            .binding
            .validate_response(&existing.record, &response)?;
        fences::resources(store, request, &agent, &parent, session).await?;
        return Ok(TargetedControlInspection::Existing(Box::new(existing)));
    }
    let stale = request.command.expected_agent_revision < agent.revision;
    if mode == Mode::Lookup || (mode == Mode::Inspect && !stale) {
        fences::resources(store, request, &agent, &parent, session).await?;
        return Ok(TargetedControlInspection::Missing);
    }
    let response = store
        .agent_commands
        .find_one(doc! { "response_id": request.target.response_id.as_str() })
        .session(&mut *session)
        .await
        .map_err(|error| trace_rejection(request, "response_lookup", error.into()))?
        .as_ref()
        .map(records::read)
        .transpose()?
        .ok_or(CommandLedgerError::NotFound)?;
    if mode == Mode::Inspect {
        request.validate_source(&agent, &response)?;
        // This successful outcome is returned only after commit. Writing the
        // current resources conflicts with any in-flight older admission;
        // the monotonically newer Agent revision then rules out later admission.
        fences::resources(store, request, &agent, &parent, session).await?;
        return Ok(TargetedControlInspection::NotAdmitted);
    }
    request
        .validate_target(&agent, &response)
        .map_err(|error| trace_rejection(request, "target", error.into()))?;
    if let AgentCommand::AnswerApproval { approval_id, .. } = &request.command.envelope.command {
        let projected = crate::store::agent_log_segments::ordinary::approval(
            store,
            &agent,
            approval_id,
            session,
        )
        .await
        .map_err(|error| match error {
            TransactionLogError::Domain(_) => ApplyError::Domain(CommandLedgerError::Corrupt),
            TransactionLogError::Driver(error) => ApplyError::Driver(error),
        })?;
        let entries = if let Some(entry) = projected {
            entry.into_iter().collect()
        } else {
            let mut pending = PendingAgentApprovals::default();
            scan_complete_log_with_session(
                ScanCaller::CommandAdmission,
                store,
                &agent.id,
                agent.conversation.last_seq,
                session,
                |entry| {
                    pending.observe(&entry);
                    Ok(())
                },
            )
            .await
            .map_err(|error| match error {
                TransactionLogError::Domain(_) => ApplyError::Domain(CommandLedgerError::Corrupt),
                TransactionLogError::Driver(error) => ApplyError::Driver(error),
            })?;
            pending.into_entries()
        };
        request.validate_approval(&entries)?;
    }
    fences::response(store, &response, session)
        .await
        .map_err(|error| trace_rejection(request, "response_fence", error))?;
    // The generic helper supplies current policy/resource writes, queue order,
    // bounded capacity, admission audit and ordinary v1 insertion in this session.
    let CommandReservationResult::Created(record) =
        reserve::apply(store, &request.command, session)
            .await
            .map_err(|error| trace_rejection(request, "command_reservation", error))?
    else {
        return Err(CommandLedgerError::IdempotencyConflict.into());
    };
    let result = request.bind(*record, &response)?;
    let revision = signed(result.record.revision, "command revision")
        .map_err(|_| CommandLedgerError::Corrupt)?;
    let sidecar = record_bson(&result.binding).map_err(|_| CommandLedgerError::Corrupt)?;
    let changed = store.agent_commands.update_one(
        doc! { "_id": result.binding.command_id.as_str(), "revision": revision, "explicit_control": { "$exists": false } },
        doc! { "$set": { "explicit_control": sidecar } },
    ).session(&mut *session).await
        .map_err(|error| trace_rejection(request, "sidecar", error.into()))?;
    if changed.matched_count != 1 {
        return Err(trace_rejection(
            request,
            "sidecar",
            CommandLedgerError::Conflict.into(),
        ));
    }
    Ok(TargetedControlInspection::Existing(Box::new(result)))
}

fn binding(document: &Document) -> Result<Option<TargetedControlBinding>, CommandLedgerError> {
    document
        .get("explicit_control")
        .map(|value| {
            mongodb::bson::deserialize_from_bson(value.clone())
                .map_err(|_| CommandLedgerError::Corrupt)
        })
        .transpose()
}

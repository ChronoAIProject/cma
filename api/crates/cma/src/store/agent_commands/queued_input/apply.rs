use layer2_core::sandbox::agent_io::{
    AgentResponseStatus, CommandAuthorizationAudit, CommandAuthorizationEffect, CommandLedgerError,
    QueuedInputBinding, QueuedInputOutcome, QueuedInputReservation,
};
use mongodb::{
    bson::{doc, Document},
    ClientSession,
};

use super::super::queued_cancellation::fences;
use super::super::{audit, lane_records, records, worker_apply, ApplyError};
use crate::store::sandbox_mongo::{record_bson, MongoSandboxStore};

pub(super) async fn apply(
    store: &MongoSandboxStore,
    request: &QueuedInputReservation,
    session: &mut ClientSession,
) -> Result<QueuedInputOutcome, ApplyError> {
    let existing = store
        .agent_commands
        .find_one(
            doc! {"scope": request.cancellation.command.scope(), "key_sha256": &request.cancellation.command.key_sha256},
        )
        .session(&mut *session)
        .await?;
    let retained = existing
        .as_ref()
        .map(|document| {
            let record = records::read(document)?;
            request.cancellation.command.replay(record.clone())?;
            let binding = binding(document)?.ok_or(CommandLedgerError::IdempotencyConflict)?;
            if binding.request_fingerprint != request.fingerprint()? {
                return Err(CommandLedgerError::IdempotencyConflict);
            }
            Ok::<_, CommandLedgerError>((record, binding))
        })
        .transpose()?;
    let Some((agent, parent)) =
        fences::current_with_observation(store, &request.cancellation, session).await?
    else {
        return Ok(QueuedInputOutcome::StaleObservation);
    };
    if let Some((record, binding)) = retained {
        let response = store
            .agent_commands
            .find_one(doc! {"_id": binding.target.source_command_id.as_str()})
            .session(&mut *session)
            .await?
            .as_ref()
            .map(records::read)
            .transpose()?
            .ok_or(CommandLedgerError::Corrupt)?;
        let result = request.replay(record, Some(binding), response)?;
        fences::resources(store, &agent, &parent, session).await?;
        return Ok(QueuedInputOutcome::Applied(Box::new(result)));
    }
    let mut response = store
        .agent_commands
        .find_one(doc! {"_id": request.cancellation.target.source_command_id.as_str()})
        .session(&mut *session)
        .await?
        .as_ref()
        .map(records::read)
        .transpose()?
        .ok_or(CommandLedgerError::NotFound)?;
    request.cancellation.validate_source(&agent, &response)?;
    if request.cancellation.target.expected_command_revision < response.revision {
        // A committed source write excludes older in-flight cancellation or
        // dispatch. The source revision can never return to the old intent.
        fences::source(store, &response, session).await?;
        fences::resources(store, &agent, &parent, session).await?;
        return Ok(QueuedInputOutcome::NotAdmitted);
    }
    let document = store
        .agent_command_orders
        .find_one(doc! {"_id": agent.id.as_str()})
        .session(&mut *session)
        .await?
        .ok_or(CommandLedgerError::Corrupt)?;
    let mut lane = lane_records::read(&document, agent.id.as_str())?;
    if lane.last_order < response.receipt.agent_order {
        return Err(CommandLedgerError::Corrupt.into());
    }
    let revision = response.revision;
    request.revise(&mut response, &mut lane)?;
    // Allocate the terminal receipt in the same lane value used for release;
    // writing a stale lane afterward would lose this order or a newer lease.
    lane.last_order = lane
        .last_order
        .checked_add(1)
        .filter(|order| *order <= i64::MAX as u64)
        .ok_or(CommandLedgerError::Corrupt)?;
    let mut record =
        request
            .cancellation
            .command
            .record(lane.last_order, agent.conversation.last_seq, None)?;
    record.status = AgentResponseStatus::Completed;
    let result = request.bind(record, response)?;
    fences::resources(store, &agent, &parent, session).await?;
    worker_apply::write(store, &result.response, revision, &lane, session).await?;
    if let Some(evidence) = &request.cancellation.command.admission_audit {
        audit::append(
            store,
            &CommandAuthorizationAudit::new(
                &result.record,
                CommandAuthorizationEffect::Admission,
                vec![evidence.clone()],
            )?,
            session,
        )
        .await?;
    }
    let mut document = records::document(&result.record)?;
    document.insert(
        "queued_input_edit",
        record_bson(&result.binding).map_err(|_| CommandLedgerError::Corrupt)?,
    );
    store
        .agent_commands
        .insert_one(document)
        .session(session)
        .await?;
    Ok(QueuedInputOutcome::Applied(Box::new(result)))
}

fn binding(document: &Document) -> Result<Option<QueuedInputBinding>, CommandLedgerError> {
    document
        .get("queued_input_edit")
        .map(|value| {
            mongodb::bson::deserialize_from_bson(value.clone())
                .map_err(|_| CommandLedgerError::Corrupt)
        })
        .transpose()
}

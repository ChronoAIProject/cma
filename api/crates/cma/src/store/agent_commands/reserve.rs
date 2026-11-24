use crate::store::agent_log_telemetry::ScanCaller;
use crate::store::work_claim_authority::without_ledger_maintenance;
use layer2_core::sandbox::agent_io::{
    validate_command_reservation, CommandLedgerError, CommandReservation, CommandReservationResult,
};
use mongodb::bson::doc;
use mongodb::ClientSession;

use super::super::agent_log_integrity::{
    integrity_document, trusted_summary, SemanticCheckpoint, TransactionLogError,
};
use super::super::agents::read_agent;
use super::super::sandbox_mongo::{
    require_version, signed, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION,
};
use super::super::sandboxes::read_sandbox;
use super::{records, ApplyError};

pub(in crate::store) async fn apply(
    store: &MongoSandboxStore,
    request: &CommandReservation,
    session: &mut ClientSession,
) -> Result<CommandReservationResult, ApplyError> {
    apply_with_cause(
        store,
        request,
        layer2_core::admission::AdmissionCause::Fresh,
        session,
    )
    .await
}

pub(in crate::store) async fn apply_with_cause(
    store: &MongoSandboxStore,
    request: &CommandReservation,
    cause: layer2_core::admission::AdmissionCause,
    session: &mut ClientSession,
) -> Result<CommandReservationResult, ApplyError> {
    store
        .bindings
        .conversation_authority(request.binding.as_ref(), session)
        .await?;
    super::super::media_uploads::require_continuation(store, &request.envelope, session).await?;
    let cause = if request.envelope.continuation.is_some() {
        // The store has proved the original accepted source and parent receipt.
        layer2_core::admission::AdmissionCause::Continuation
    } else {
        cause
    };
    if let Some(existing) = store
        .agent_commands
        .find_one(doc! { "scope": request.scope(), "key_sha256": &request.key_sha256 })
        .session(&mut *session)
        .await?
    {
        return Ok(request.replay(records::read(&existing)?)?);
    }
    if let Some(key) = request.envelope.external_event_key() {
        if store
            .agent_commands
            .find_one(doc! { "external_event_key": key })
            .session(&mut *session)
            .await?
            .is_some()
        {
            return Err(CommandLedgerError::IdempotencyConflict.into());
        }
    }
    store.bindings.current_input(request, session).await?;
    let agent_row = store
        .agents
        .find_one(doc! { "_id": request.envelope.agent_id.as_str() })
        .session(&mut *session)
        .await?
        .ok_or(CommandLedgerError::NotFound)?;
    // Already accepted scope/key replay returned above. Controls for accepted
    // turns keep their existing validation; only new messages are closed.
    if matches!(
        request.envelope.command,
        layer2_core::sandbox::agent_io::AgentCommand::Message { .. }
    ) && agent_row.contains_key("channel_reuse_cleanup_admission")
    {
        return Err(CommandLedgerError::NotAdmissible.into());
    }
    let agent = read_agent(&agent_row).map_err(|_| CommandLedgerError::Corrupt)?;
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
    validate_command_reservation(request, &agent, &parent)?;
    if agent.bootstrap_operation(&parent).is_some() {
        let workspace = store
            .workspaces
            .find_one(doc! {"_id": agent.workspace_id.as_str(),
            "owned_workspace_cleanup_admission_v1": {"$exists": false},
            "channel_reuse_cleanup_execution": {"$exists": false}})
            .session(&mut *session)
            .await?
            .as_ref()
            .map(super::super::workspaces::read_workspace)
            .transpose()
            .map_err(|_| CommandLedgerError::Corrupt)?
            .ok_or(CommandLedgerError::NotAdmissible)?;
        if agent.bootstrap_lineage(&parent, &workspace).is_none() {
            return Err(CommandLedgerError::NotAdmissible.into());
        }
        let fenced = store
            .workspaces
            .update_one(
                doc! {"_id": workspace.id.as_str(),
                "revision": signed(workspace.revision, "bootstrap workspace revision")
                    .map_err(|_| CommandLedgerError::Corrupt)?},
                doc! {"$inc": {"agent_command_admission_fence": 1_i64}},
            )
            .session(&mut *session)
            .await?;
        if fenced.matched_count != 1 {
            return Err(CommandLedgerError::Conflict.into());
        }
    }
    let policy = store
        .resource_policies
        .find_one(doc! { "_id": parent.id.as_str() })
        .session(&mut *session)
        .await?
        .as_ref()
        .map(super::super::resource_access::read_inventory_policy)
        .transpose()
        .map_err(|_| CommandLedgerError::Corrupt)?;
    layer2_core::sandbox::agent_io::validate_command_reservation_policy(
        request,
        &parent,
        policy.as_ref(),
    )?;

    if matches!(
        request.envelope.command,
        layer2_core::sandbox::agent_io::AgentCommand::Message { .. }
    ) {
        let now = layer2_core::seconds_from_rfc3339(&request.admitted_at)
            .ok_or(CommandLedgerError::Invalid)?;
        super::super::budget::reserve_command(
            store,
            &agent,
            request.envelope.command_id.as_str(),
            now,
            cause,
            request.binding.is_some(),
            session,
        )
        .await
        .map_err(|error| match error {
            super::super::agent_conversation_store::ApplyError::Driver(error) => {
                ApplyError::Driver(error)
            }
            super::super::agent_conversation_store::ApplyError::Domain(
                layer2_core::sandbox::store::SandboxStoreError::Budget(reason),
            ) => ApplyError::Domain(CommandLedgerError::Budget(reason)),
            super::super::agent_conversation_store::ApplyError::Domain(
                layer2_core::sandbox::store::SandboxStoreError::AdmissionPaused,
            ) => ApplyError::Domain(CommandLedgerError::AdmissionPaused),
            super::super::agent_conversation_store::ApplyError::Domain(
                layer2_core::sandbox::store::SandboxStoreError::SessionCapacityUnavailable,
            ) => ApplyError::Domain(CommandLedgerError::SessionCapacityUnavailable),
            _ => ApplyError::Domain(CommandLedgerError::Unavailable),
        })?;
    }

    // Real writes serialize resource retirement and policy changes against the observation. These
    // additive top-level fences do not change the old aggregate wire records.
    super::super::agent_log_segments::ordinary::source(store, &agent, &agent_row, session)
        .await
        .map_err(map_integrity_error)?;
    let parent_revision =
        signed(parent.revision, "Sandbox revision").map_err(|_| CommandLedgerError::Corrupt)?;
    let agent_fence =
        super::super::agent_capacity_recovery::source::envelope_filter(&agent, &agent_row)
            .map_err(|_| CommandLedgerError::Corrupt)?;
    let result = store
        .agents
        .update_one(
            agent_fence,
            doc! { "$inc": { "agent_command_admission_fence": 1_i64 } },
        )
        .session(&mut *session)
        .await?;
    if result.matched_count != 1 {
        return Err(CommandLedgerError::Conflict.into());
    }
    let result = store
        .sandboxes
        .update_one(
            crate::store::work_schema::query(without_ledger_maintenance(doc! {
                "_id": parent.id.as_str(),
                "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                "revision": parent_revision,
            })),
            doc! { "$inc": { "agent_command_admission_fence": 1_i64 } },
        )
        .session(&mut *session)
        .await?;
    if result.matched_count != 1 {
        return Err(CommandLedgerError::Conflict.into());
    }

    let waiting = store
        .agent_commands
        .count_documents(doc! { "agent_id": agent.id.as_str(), "$or": [
            {"status": "queued"},
            {"status": "waiting_action", "channel_admission.execution_hold": "worker_unavailable"},
            {"status": "waiting_action", "channel_control_admission.execution_hold": "worker_unavailable"}
        ] })
        .session(&mut *session)
        .await?;
    if waiting >= u64::from(request.queue_capacity) {
        return Err(CommandLedgerError::QueueFull.into());
    }
    if let Some(expected) =
        layer2_core::sandbox::agent_io::AnswerContext::from_command(&request.envelope.command)?
    {
        if store
            .agent_commands
            .find_one(doc! {
                "agent_id": agent.id.as_str(),
                "record.receipt.event_cursor": { "$gte": expected.answer_sequence as i64 },
                "record.receipt.envelope.command.type": "message",
            })
            .session(&mut *session)
            .await?
            .is_some()
        {
            return Err(CommandLedgerError::NotAdmissible.into());
        }
        let answer = match super::super::agent_log_segments::ordinary::answer(
            store, &agent, &agent_row, session,
        )
        .await
        .map_err(map_integrity_error)?
        {
            Some(answer) => answer,
            None => match trusted_summary(
                store,
                &agent,
                &agent_row,
                ScanCaller::CommandAdmission,
                session,
                false,
            )
            .await
            .map_err(map_integrity_error)?
            {
                Some(summary) => summary
                    .semantic()
                    .and_then(SemanticCheckpoint::answer_context),
                None => {
                    let mut entries = Vec::new();
                    let summary = super::super::agent_log::scan_complete_log_with_session(
                        ScanCaller::CommandAdmission,
                        store,
                        &agent.id,
                        agent.conversation.last_seq,
                        session,
                        |entry| {
                            entries.push(entry);
                            Ok(())
                        },
                    )
                    .await
                    .map_err(map_integrity_error)?;
                    let semantic = crate::store::agent_log_integrity::audit_checkpoint(
                        store, &agent, &entries, session,
                    )
                    .await
                    .map_err(map_integrity_error)?;
                    if !semantic.valid {
                        return Err(ApplyError::Domain(CommandLedgerError::Corrupt));
                    }
                    let integrity =
                        integrity_document(&agent, &summary, agent.revision, semantic.clone())
                            .map_err(|_| ApplyError::Domain(CommandLedgerError::Corrupt))?;
                    let revision = signed(agent.revision, "Agent revision")
                        .map_err(|_| CommandLedgerError::Corrupt)?;
                    let head = signed(agent.conversation.last_seq, "Agent log sequence")
                        .map_err(|_| CommandLedgerError::Corrupt)?;
                    let repaired = store
                        .agents
                        .update_one(
                            doc! {
                                "_id": agent.id.as_str(),
                                "revision": revision,
                                "last_seq": head,
                                "record.conversation.last_seq": head,
                            },
                            doc! { "$set": { "agent_log_integrity_v1": integrity } },
                        )
                        .session(&mut *session)
                        .await?;
                    if repaired.matched_count != 1 {
                        return Err(CommandLedgerError::Conflict.into());
                    }
                    semantic.answer_context()
                }
            },
        };
        layer2_core::sandbox::agent_io::require_answer_context(request, &agent, waiting, answer)?;
    }
    if store
        .agent_commands
        .find_one(doc! { "_id": request.envelope.command_id.as_str() })
        .session(&mut *session)
        .await?
        .is_some()
    {
        return Err(CommandLedgerError::Conflict.into());
    }
    let previous = store
        .agent_command_orders
        .find_one(doc! { "_id": agent.id.as_str() })
        .session(&mut *session)
        .await?;
    let order = match previous {
        Some(previous) => {
            require_version(&previous).map_err(|_| CommandLedgerError::Corrupt)?;
            let previous_order = previous
                .get_i64("last_order")
                .map_err(|_| CommandLedgerError::Corrupt)?;
            if previous_order < 1 {
                return Err(CommandLedgerError::Corrupt.into());
            }
            let order = previous_order
                .checked_add(1)
                .ok_or(CommandLedgerError::Corrupt)?;
            let updated = store
                .agent_command_orders
                .update_one(
                    doc! { "_id": agent.id.as_str(), "last_order": previous_order },
                    doc! { "$set": { "last_order": order } },
                )
                .session(&mut *session)
                .await?;
            if updated.matched_count != 1 {
                return Err(CommandLedgerError::Conflict.into());
            }
            order
        }
        None => {
            store.agent_command_orders.insert_one(doc! { "_id": agent.id.as_str(), "document_schema_version": DOCUMENT_SCHEMA_VERSION, "last_order": 1_i64 }).session(&mut *session).await?;
            1
        }
    };
    let target_turn = (!matches!(
        request.envelope.command,
        layer2_core::sandbox::agent_io::AgentCommand::Message { .. }
    ))
    .then_some(agent.conversation.active_turn)
    .flatten();
    let record = request.record(order as u64, agent.conversation.last_seq, target_turn)?;
    if let Some(evidence) = &request.admission_audit {
        let audit = layer2_core::sandbox::agent_io::CommandAuthorizationAudit::new(
            &record,
            layer2_core::sandbox::agent_io::CommandAuthorizationEffect::Admission,
            vec![evidence.clone()],
        )?;
        super::audit::append(store, &audit, session).await?;
    }
    store
        .agent_commands
        .insert_one(records::document(&record)?)
        .session(session)
        .await?;
    Ok(CommandReservationResult::Created(Box::new(record)))
}

fn map_integrity_error(error: TransactionLogError) -> ApplyError {
    match error {
        TransactionLogError::Driver(error) => ApplyError::Driver(error),
        TransactionLogError::Domain(_) => ApplyError::Domain(CommandLedgerError::Corrupt),
    }
}

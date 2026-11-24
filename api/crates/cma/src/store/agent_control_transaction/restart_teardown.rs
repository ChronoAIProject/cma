//! Publish a pending restart's Stop or Delete only after its native fence.
use super::{
    restart::ACTIVE_RESTART_FIELD,
    restart_cancel::TEARDOWN,
    restart_dispatch::{live_lease_expression, read_restart_record, DISPATCH_FIELD},
    *,
};
use crate::store::{
    agent_control_receipt::{restore_owner_stop, RESTART_PREDECESSOR_FIELD, STOP_RECEIPT_FIELD},
    agent_log_integrity::{AppendCapacity, TransactionLogError},
    agent_log_segments::{
        publication::{self, PublicationIdentity},
        transaction_scope::TransactionScope,
        writer_commit::{self, AggregateUpdate, PublicationInput, Recorded},
    },
    agents::read_agent,
    sandbox_mongo::record_bson,
    work_publication::FenceError,
};
use layer2_core::{
    sandbox::{
        store::{
            AgentControlTarget, AgentRestartTeardown, AgentRestartTeardownPublication,
            AgentWorkPublicationLineage,
        },
        AgentStatus, AgentStopReason, AgentWorkReceipt, AgentWorkStep,
    },
    seconds_from_rfc3339,
};
use layer3_adapter::{AgentCloseRequest, AgentProcessAction, AgentSlot};
use mongodb::bson::Document;

const FINISHED: &str = "control_restart_teardown_finished_v1";

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Witness {
    operation_id: String,
    request: ControlRequest,
    restart_operation_id: String,
    predecessor: mongodb::bson::Bson,
    revision: i64,
}

pub(in crate::store) async fn load(
    store: &MongoSandboxStore,
    claimed: &Agent,
) -> Result<AgentRestartTeardown, SandboxStoreError> {
    let row = store
        .agents
        .find_one(doc! {"_id": claimed.id.as_str()})
        .await
        .map_err(driver)?
        .ok_or(SandboxStoreError::NotFound)?;
    if read_agent(&row)? != *claimed || claimed.work.lease.is_none() {
        return Err(SandboxStoreError::Conflict);
    }
    read(&row, claimed).map(|(teardown, _)| teardown)
}

pub(super) fn read(
    row: &Document,
    agent: &Agent,
) -> Result<(AgentRestartTeardown, ControlOperation), SandboxStoreError> {
    if agent.status != AgentStatus::Starting || agent.work.step != AgentWorkStep::RestartTeardown {
        return Err(SandboxStoreError::Conflict);
    }
    let corrupt =
        || SandboxStoreError::Corrupt("stored Agent restart teardown is inconsistent".into());
    let witness: Witness = mongodb::bson::deserialize_from_document(
        row.get_document(TEARDOWN).map_err(|_| corrupt())?.clone(),
    )
    .map_err(|_| corrupt())?;
    let work = read_restart_record(row, agent)?;
    let open = work.dispatched_open.ok_or_else(corrupt)?;
    let target = witness.request.target();
    let reason = match &witness.request {
        ControlRequest::Stop(request) => &request.reason,
        ControlRequest::Delete(_) => &AgentStopReason::Owner,
    };
    if target.agent_id != agent.id
        || target.owner != agent.owner
        || target.sandbox_id != agent.sandbox_id
        || target.workspace_id != agent.workspace_id
        || target.worktree_id != agent.worktree_id
        || target.expected_status != AgentStatus::Starting
        || target.expected_revision < work.acceptance.revision
        || target.expected_revision.checked_add(1) != u64::try_from(witness.revision).ok()
        || u64::try_from(witness.revision).map_or(true, |value| value > agent.revision)
        || agent.requested_stop_reason.as_ref() != Some(reason)
        || witness.restart_operation_id != work.acceptance.operation_id
        || row.get(RESTART_PREDECESSOR_FIELD) != Some(&witness.predecessor)
    {
        return Err(corrupt());
    }
    let operation =
        ControlOperation::restore(witness.request, witness.operation_id).map_err(|_| corrupt())?;
    Ok((
        AgentRestartTeardown {
            operation_id: operation.id().into(),
            acceptance: work.acceptance,
            open,
        },
        operation,
    ))
}

pub(in crate::store) async fn publish(
    store: &MongoSandboxStore,
    publication: &AgentRestartTeardownPublication,
) -> Result<(), SandboxStoreError> {
    for attempt in 0..CONTROL_ATTEMPTS {
        crate::store::workspace_admission::retry::retry_delay(attempt).await;
        let mut session = store.client.start_session().await.map_err(driver)?;
        let scope = match TransactionScope::start(&mut session).await {
            Ok(scope) => scope,
            Err(error) if is_transient(&error) => continue,
            Err(error) => return Err(driver(error)),
        };
        let (settled, next, segmented) = match Box::pin(apply(store, publication, scope)).await {
            Ok(result) => result,
            Err(error) => {
                let _ = session.abort_guarded().await;
                match error {
                    TransactionLogError::Driver(error) if is_transient(&error) => continue,
                    TransactionLogError::Driver(error) => return Err(driver(error)),
                    TransactionLogError::Domain(error) => return Err(error),
                }
            }
        };
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => return Ok(()),
            CommitOutcome::Retry(_) => continue,
            CommitOutcome::Failed(error) => return Err(driver(error)),
            CommitOutcome::Unknown(error) => {
                let at = session.operation_time().ok_or_else(|| driver(error))?;
                let store = store.clone();
                return wait_for_reconciliation("Agent restart teardown", async move {
                    let mut read = store
                        .client
                        .start_session()
                        .snapshot(true)
                        .snapshot_time(at)
                        .await
                        .map_err(driver)?;
                    let Some(row) = store
                        .agents
                        .find_one(doc! {"_id": next.id.as_str(), FINISHED: settled})
                        .session(&mut read)
                        .await
                        .map_err(driver)?
                    else {
                        return Err(SandboxStoreError::Unavailable(
                            "exact Agent restart teardown publication is unconfirmed".into(),
                        ));
                    };
                    if read_agent(&row)? != next || row.contains_key(TEARDOWN) {
                        return Err(SandboxStoreError::Conflict);
                    }
                    if let Some(recorded) = segmented {
                        if !recorded.matches(&store, &mut read).await? {
                            return Err(SandboxStoreError::Unavailable(
                                "segmented Agent restart teardown publication is unconfirmed"
                                    .into(),
                            ));
                        }
                    }
                    Ok(())
                })
                .await
                .map_err(SandboxStoreError::Unavailable)?;
            }
        }
    }
    Err(SandboxStoreError::Conflict)
}

async fn apply(
    store: &MongoSandboxStore,
    input: &AgentRestartTeardownPublication,
    mut scope: TransactionScope<'_>,
) -> Result<(Document, Agent, Option<Recorded>), TransactionLogError> {
    let conflict = || TransactionLogError::Domain(SandboxStoreError::Conflict);
    let current = scope
        .control_snapshot(store, &AgentControlTarget::from(&input.claimed))
        .await
        .map_err(lineage)?;
    let (teardown, operation) = read(&current.row, &current.agent)?;
    let lease = current.agent.work.lease.as_ref().ok_or_else(conflict)?;
    let pin = &teardown.acceptance.source.runtime;
    let closed_successor = input
        .acknowledgement
        .successor_open_request_sha256
        .is_some();
    if current.agent != input.claimed
        || teardown != input.teardown
        || input.parents.lineage != AgentWorkPublicationLineage::Authorized
        || current.sandbox.revision != input.parents.sandbox_revision
        || current.workspace.revision != input.parents.workspace_revision
        || current.worktree.revision != input.parents.worktree_revision
        || current.worktree.revision != teardown.acceptance.source.worktree_revision
        || current.sandbox.runtime.as_ref().is_none_or(|runtime| {
            runtime.boot_id.as_ref() != Some(&pin.boot_id)
                || runtime.provision_generation != Some(pin.runtime.generation)
        })
        || current
            .sandbox
            .lifecycle
            .home
            .as_ref()
            .and_then(|home| home.attachment())
            .and_then(|home| home.identity().ok())
            .as_ref()
            != Some(&pin.runtime.home_identity)
        || input.acknowledgement
            != teardown.fence(lease.fence, current.worktree.path.clone(), closed_successor)
        || lease.fence.checked_add(1) != Some(current.agent.work.next_fence)
        || seconds_from_rfc3339(&input.published_at)
            .zip(seconds_from_rfc3339(&current.agent.updated_at))
            .is_none_or(|(published, updated)| published < updated)
    {
        return Err(conflict());
    }
    let (next, receipt_fields) = settled_agent(&current, &teardown, &operation, input)?;
    let settled = doc! {
        "operation_id": operation.id(),
        "proof": record_bson(&input.acknowledgement).map_err(SandboxStoreError::Corrupt)?,
        "revision": signed(next.revision, "restart teardown revision").map_err(SandboxStoreError::Corrupt)?,
    };
    let mut fields = agent_replacement_fields(&next)?;
    fields.insert(FINISHED, settled.clone());
    fields.extend(receipt_fields);
    crate::store::agent_conversation_store::carry_checkpoint_fields(
        &current.row,
        &current.agent,
        &next,
        &mut fields,
    )?;
    let number = |value, name| signed(value, name).map_err(SandboxStoreError::Corrupt);
    let mut filter = doc! {
        "_id": current.agent.id.as_str(),
        "revision": number(current.agent.revision, "Agent revision")?,
        "last_seq": number(current.agent.conversation.last_seq, "Agent log head")?,
        "work_lease_holder": &lease.holder,
        "work_lease_fence": number(lease.fence, "Agent work fence")?,
        "work_lease_expires_at": &lease.expires_at,
        TEARDOWN: current.row.get(TEARDOWN).ok_or_else(conflict)?.clone(),
        ACTIVE_RESTART_FIELD: current.row.get(ACTIVE_RESTART_FIELD).ok_or_else(conflict)?.clone(),
        DISPATCH_FIELD: current.row.get(DISPATCH_FIELD).ok_or_else(conflict)?.clone(),
        "channel_history_seal": {"$exists": false},
        "channel_reuse_cleanup_execution": {"$exists": false},
        "$expr": live_lease_expression(),
    };
    scope.fence_ordinary_work(store).await.map_err(fence)?;
    let segmented = if crate::store::agent_log_segments::read_format(&current.row)?.is_some() {
        let identity = PublicationIdentity {
            key: format!("{}:settled", operation.id()),
            fingerprint: publication::request_digest(&settled)?,
            appended_at: input.published_at.clone(),
        };
        Some(
            writer_commit::publish_existing(
                store,
                &mut scope,
                PublicationInput {
                    operation: &identity,
                    current: &current.agent,
                    next: &next,
                    logs: &[],
                    capacity: AppendCapacity::StopOrFailure,
                    result_digest: publication::result_digest(&settled)?,
                },
                AggregateUpdate { filter, fields },
            )
            .await?,
        )
    } else {
        filter.insert("document_schema_version", DOCUMENT_SCHEMA_VERSION);
        carry_integrity_for_revision(&current.row, &current.agent, &next, &mut fields)?;
        let updated = scope
            .update_one(&store.agents, filter, doc! {"$set": fields})
            .await
            .map_err(TransactionLogError::Driver)?;
        if updated.matched_count != 1 {
            return Err(conflict());
        }
        None
    };
    let session = scope.release();
    store
        .fence_agent_effect_transaction(&current.agent.id, session)
        .await
        .map_err(|error| match error {
            crate::store::agent_ownership::FenceError::Driver(error) => {
                TransactionLogError::Driver(error)
            }
            crate::store::agent_ownership::FenceError::Domain(error) => error.into(),
        })?;
    fence_control_lineage(store, &current, operation.id(), session)
        .await
        .map_err(lineage)?;
    let unset = store.agents.update_one(
        doc! {"_id": current.agent.id.as_str(), FINISHED: &settled},
        doc! {"$unset": {ACTIVE_RESTART_FIELD: "", DISPATCH_FIELD: "", TEARDOWN: "", RESTART_PREDECESSOR_FIELD: ""}},
    ).session(session).await.map_err(TransactionLogError::Driver)?;
    if unset.matched_count != 1 {
        return Err(conflict());
    }
    Ok((settled, next, segmented))
}

fn settled_agent(
    current: &ControlSnapshot,
    teardown: &AgentRestartTeardown,
    operation: &ControlOperation,
    input: &AgentRestartTeardownPublication,
) -> Result<(Agent, Document), SandboxStoreError> {
    let mut next = current.agent.clone();
    let reason = next
        .requested_stop_reason
        .clone()
        .ok_or(SandboxStoreError::Conflict)?;
    next.transition(
        AgentStatus::Stopped,
        input.published_at.clone(),
        Some(reason),
    )
    .map_err(|_| SandboxStoreError::Conflict)?;
    next.work
        .schedule(AgentWorkStep::None, input.published_at.clone());
    next.work.cleanup_debt.clear();
    let source = &teardown.acceptance.source;
    let mut fields = Document::new();
    if input
        .acknowledgement
        .successor_open_request_sha256
        .is_some()
    {
        let close = AgentCloseRequest::new(
            AgentProcessAction::Close
                .operation_id(next.id.as_str(), teardown.acceptance.next_incarnation),
            AgentSlot::new(next.id.as_str()),
        );
        next.work.receipts.extend([
            AgentWorkReceipt {
                step: AgentWorkStep::OpenConversation,
                operation_id: teardown.open.operation_id.clone(),
                request_sha256: teardown.open.request_sha256.clone(),
                completed_at: input.published_at.clone(),
            },
            AgentWorkReceipt {
                step: AgentWorkStep::CloseProcess,
                operation_id: close.operation_id,
                request_sha256: close.request_sha256,
                completed_at: input.published_at.clone(),
            },
        ]);
        if matches!(operation.request, ControlRequest::Stop(_)) {
            fields.extend(
                operation
                    .receipt_at(
                        current
                            .row
                            .get_document(TEARDOWN)
                            .map_err(|_| SandboxStoreError::Conflict)?
                            .get_i64("revision")
                            .map_err(|_| SandboxStoreError::Conflict)?,
                    )
                    .map_err(control)?,
            );
        }
    } else {
        let (stopped_at, receipt) = restore_owner_stop(&current.row).map_err(control)?;
        next.stopped_at = Some(stopped_at);
        next.stop_reason = Some(AgentStopReason::Owner);
        next.work.receipts.extend([
            source.predecessor_open.clone(),
            source.predecessor_close.clone(),
        ]);
        fields.insert(STOP_RECEIPT_FIELD, receipt);
    }
    if matches!(operation.request, ControlRequest::Delete(_)) {
        let source_revision = next.revision;
        next.request_delete(input.published_at.clone())
            .map_err(|_| SandboxStoreError::Conflict)?;
        fields.extend(
            operation
                .delete_after_restart_teardown(source_revision)
                .receipt_update(&next)
                .map_err(control)?,
        );
    }
    Ok((next, fields))
}

fn lineage(error: LineageError) -> TransactionLogError {
    match error {
        LineageError::Driver(error) => TransactionLogError::Driver(error),
        LineageError::ReceiptCorrupt(detail) => SandboxStoreError::Corrupt(detail).into(),
        LineageError::Domain(error) => control(error).into(),
    }
}

fn fence(error: FenceError) -> TransactionLogError {
    match error {
        FenceError::Driver(error) => TransactionLogError::Driver(error),
        FenceError::Domain(error) => error.into(),
    }
}

fn control(error: AgentControlStoreError) -> SandboxStoreError {
    match error {
        AgentControlStoreError::Corrupt(detail) => SandboxStoreError::Corrupt(detail),
        AgentControlStoreError::NotFound => SandboxStoreError::NotFound,
        AgentControlStoreError::Unavailable(detail) => SandboxStoreError::Unavailable(detail),
        _ => SandboxStoreError::Conflict,
    }
}

fn driver(error: mongodb::error::Error) -> SandboxStoreError {
    SandboxStoreError::Unavailable(error.to_string())
}

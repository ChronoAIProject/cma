//! Transfer restart custody to Stop or Delete. Before dispatch there is no
//! runtime effect to reconcile. After dispatch the Agent holds the live slot.
use super::restart::ACTIVE_RESTART_FIELD;
use super::restart_dispatch::{read_work, DISPATCH_FIELD};
use super::*;
use crate::store::{
    agent_control_receipt::{restore_owner_stop, RESTART_PREDECESSOR_FIELD, STOP_RECEIPT_FIELD},
    agent_log_integrity::{AppendCapacity, TransactionLogError},
    agent_log_segments::{
        publication::{self, PublicationIdentity},
        transaction_scope::TransactionScope,
        writer_commit::{self, AggregateUpdate, PublicationInput, Recorded},
    },
    sandbox_mongo::record_bson,
};
use layer2_core::sandbox::{AgentStatus, AgentStopReason, AgentWorkStep, SandboxStatus};
use layer2_core::seconds_from_rfc3339;
use mongodb::bson::Document;

const CANCELLED: &str = "control_restart_cancelled_v1";
pub(in crate::store) const TEARDOWN: &str = "control_restart_teardown_v1";

struct Applied {
    next: Agent,
    witness: Document,
    publication: Option<Recorded>,
    field: &'static str,
}

enum Attempt {
    Existing(Box<Agent>),
    Scheduled(Box<Applied>),
}

pub(super) async fn schedule(
    store: &MongoSandboxStore,
    operation: &ControlOperation,
) -> Result<AgentControlResult, AgentControlStoreError> {
    for attempt in 0..CONTROL_ATTEMPTS {
        crate::store::workspace_admission::retry::retry_delay(attempt).await;
        let mut session = store.client.start_session().await.map_err(unavailable)?;
        let scope = match TransactionScope::start(&mut session).await {
            Ok(scope) => scope,
            Err(error) if is_transient(&error) => continue,
            Err(error) => return Err(unavailable(error)),
        };
        let Applied {
            next,
            witness,
            publication,
            field,
        } = match Box::pin(apply(store, operation, scope)).await {
            Ok(Attempt::Scheduled(result)) => *result,
            Ok(Attempt::Existing(current)) => {
                let _ = session.abort_guarded().await;
                return Ok(AgentControlResult::Existing(*current));
            }
            Err(error) => {
                let _ = session.abort_guarded().await;
                match error {
                    ApplyError::Driver(error) if is_transient(&error) => continue,
                    ApplyError::Driver(error) => return Err(unavailable(error)),
                    ApplyError::Domain(error) => return Err(error),
                }
            }
        };
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => return Ok(AgentControlResult::Scheduled(next)),
            CommitOutcome::Retry(_) => continue,
            CommitOutcome::Failed(error) => return Err(unavailable(error)),
            CommitOutcome::Unknown(error) => {
                let at = session
                    .operation_time()
                    .ok_or_else(|| unknown_control(&error))?;
                let store = store.clone();
                let target = operation.request.target().clone();
                return wait_for_reconciliation("Agent restart cancellation", async move {
                    let mut session = store
                        .client
                        .start_session()
                        .snapshot(true)
                        .snapshot_time(at)
                        .await
                        .map_err(unavailable)?;
                    let observed = load_control_snapshot(&store, &target, &mut session, true)
                        .await
                        .map_err(|failure| reconciliation_error(failure, &error))?;
                    if observed.agent != next
                        || observed.row.get_document(field).ok() != Some(&witness)
                        || [
                            ACTIVE_RESTART_FIELD,
                            RESTART_PREDECESSOR_FIELD,
                            DISPATCH_FIELD,
                        ]
                        .into_iter()
                        .any(|key| observed.row.contains_key(key) != (field == TEARDOWN))
                    {
                        return Err(unknown_control("exact restart cancellation is absent"));
                    }
                    if let Some(publication) = publication {
                        if !publication
                            .matches(&store, &mut session)
                            .await
                            .map_err(store_error)?
                        {
                            return Err(unknown_control(
                                "segmented restart cancellation is absent",
                            ));
                        }
                    }
                    Ok(AgentControlResult::Scheduled(observed.agent))
                })
                .await
                .map_err(AgentControlStoreError::Unavailable)?;
            }
        }
    }
    Err(AgentControlStoreError::Conflict)
}

async fn apply(
    store: &MongoSandboxStore,
    operation: &ControlOperation,
    mut scope: TransactionScope<'_>,
) -> Result<Attempt, ApplyError> {
    let conflict = || ApplyError::Domain(AgentControlStoreError::Conflict);
    let current = scope
        .control_snapshot(store, operation.request.target())
        .await?;
    if current.row.contains_key(TEARDOWN) {
        return existing_teardown(operation, &current);
    }
    let work = read_work(&current.row, &current.agent).map_err(domain)?;
    if current.agent.revision != operation.request.target().expected_revision
        || current.agent.status != AgentStatus::Starting
        || current.agent.work.step != AgentWorkStep::RestartConversation
        || current.sandbox.status == SandboxStatus::Rotating
    {
        return Err(conflict());
    }
    let at = match &operation.request {
        ControlRequest::Stop(request) => &request.requested_at,
        ControlRequest::Delete(request) => &request.requested_at,
    };
    let requested_at = seconds_from_rfc3339(at).ok_or_else(conflict)?;
    let observed_at = seconds_from_rfc3339(&current.agent.updated_at).ok_or_else(conflict)?;
    if requested_at < observed_at && current.sandbox.status != SandboxStatus::Retiring {
        return Err(conflict());
    }
    let stamp = if requested_at < observed_at {
        &current.agent.updated_at
    } else {
        at
    };
    let (next, receipts, field) = if work.dispatched_open.is_some() {
        prepare_handoff(&current, operation, stamp)?
    } else {
        cancel_undispatched(&current, operation, stamp)?
    };
    let active = current.row.get(ACTIVE_RESTART_FIELD).ok_or_else(conflict)?;
    let predecessor = current
        .row
        .get(RESTART_PREDECESSOR_FIELD)
        .ok_or_else(conflict)?;
    let revision = signed(current.agent.revision, "Agent revision").map_err(|_| conflict())?;
    let last_seq =
        signed(current.agent.conversation.last_seq, "Agent log head").map_err(|_| conflict())?;
    let mut filter = doc! {
        "_id": current.agent.id.as_str(),
        "status": "starting",
        "revision": revision,
        "record.revision": revision,
        "last_seq": last_seq,
        "record.conversation.last_seq": last_seq,
        ACTIVE_RESTART_FIELD: active.clone(),
        RESTART_PREDECESSOR_FIELD: predecessor.clone(),
        DISPATCH_FIELD: current.row.get(DISPATCH_FIELD).cloned()
            .unwrap_or_else(|| mongodb::bson::Bson::Document(doc! {"$exists": false})),
        TEARDOWN: {"$exists": false},
        STOP_RECEIPT_FIELD: {"$exists": false},
        "channel_reuse_cleanup_execution": {"$exists": false},
        "channel_history_seal": {"$exists": false},
    };
    let witness = doc! {
        "operation_id": operation.id(),
        "request": record_bson(&operation.request).map_err(AgentControlStoreError::Corrupt)
            .map_err(ApplyError::Domain)?,
        "restart_operation_id": &work.acceptance.operation_id,
        "predecessor": predecessor.clone(),
        "revision": signed(next.revision, "cancelled restart revision").map_err(|_| conflict())?,
    };
    let mut fields = agent_replacement_fields(&next).map_err(domain)?;
    fields.insert(field, witness.clone());
    fields.extend(receipts);
    crate::store::agent_conversation_store::carry_checkpoint_fields(
        &current.row,
        &current.agent,
        &next,
        &mut fields,
    )
    .map_err(domain)?;
    scope
        .fence_ordinary_work(store)
        .await
        .map_err(|error| match error {
            crate::store::work_publication::FenceError::Domain(error) => domain(error),
            crate::store::work_publication::FenceError::Driver(error) => ApplyError::Driver(error),
        })?;
    let publication = if crate::store::agent_log_segments::read_format(&current.row)
        .map_err(domain)?
        .is_some()
    {
        let identity = PublicationIdentity {
            key: operation.id().to_owned(),
            fingerprint: publication::request_digest(&operation.request).map_err(domain)?,
            appended_at: stamp.clone(),
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
                    result_digest: publication::result_digest(&witness).map_err(domain)?,
                },
                AggregateUpdate { filter, fields },
            )
            .await
            .map_err(|error| match error {
                TransactionLogError::Domain(error) => domain(error),
                TransactionLogError::Driver(error) => ApplyError::Driver(error),
            })?,
        )
    } else {
        filter.insert("document_schema_version", DOCUMENT_SCHEMA_VERSION);
        carry_integrity_for_revision(&current.row, &current.agent, &next, &mut fields)
            .map_err(domain)?;
        let updated = scope
            .update_one(&store.agents, filter, doc! {"$set": fields})
            .await
            .map_err(ApplyError::Driver)?;
        if updated.matched_count != 1 {
            return Err(conflict());
        }
        None
    };
    let session = scope.release();
    store
        .fence_agent_restart_cancellation(&current.agent.id, session)
        .await
        .map_err(|error| match error {
            crate::store::agent_ownership::FenceError::Domain(error) => domain(error),
            crate::store::agent_ownership::FenceError::Driver(error) => ApplyError::Driver(error),
        })?;
    fence_control_lineage(store, &current, operation.id(), session).await?;
    if field == TEARDOWN {
        return Ok(Attempt::Scheduled(Box::new(Applied {
            next,
            witness,
            publication,
            field,
        })));
    }
    let removed = store
        .agents
        .update_one(
            doc! {"_id": current.agent.id.as_str(), CANCELLED: &witness},
            doc! {"$unset": {ACTIVE_RESTART_FIELD: "", RESTART_PREDECESSOR_FIELD: ""}},
        )
        .session(session)
        .await
        .map_err(ApplyError::Driver)?;
    if removed.matched_count != 1 {
        return Err(conflict());
    }
    Ok(Attempt::Scheduled(Box::new(Applied {
        next,
        witness,
        publication,
        field,
    })))
}

fn existing_teardown(
    operation: &ControlOperation,
    current: &ControlSnapshot,
) -> Result<Attempt, ApplyError> {
    let (_, stored) =
        super::restart_teardown::read(&current.row, &current.agent).map_err(domain)?;
    match (&operation.request, stored.request) {
        (ControlRequest::Stop(new), ControlRequest::Stop(old)) if new.reason == old.reason => {
            Ok(Attempt::Existing(Box::new(current.agent.clone())))
        }
        (ControlRequest::Stop(_), ControlRequest::Delete(_))
        | (ControlRequest::Delete(_), ControlRequest::Delete(_)) => {
            Ok(Attempt::Existing(Box::new(current.agent.clone())))
        }
        _ => Err(ApplyError::Domain(AgentControlStoreError::WorkConflict {
            step: current.agent.work.step,
        })),
    }
}

fn prepare_handoff(
    current: &ControlSnapshot,
    operation: &ControlOperation,
    stamp: &str,
) -> Result<(Agent, Document, &'static str), ApplyError> {
    let reason = match &operation.request {
        ControlRequest::Stop(request) => request.reason.clone(),
        ControlRequest::Delete(_) => AgentStopReason::Owner,
    };
    let mut next = current.agent.clone();
    next.request_stop(reason, stamp.to_owned())
        .map_err(|_| ApplyError::Domain(AgentControlStoreError::Conflict))?;
    // The worker claims a fence beyond every previously allocated Open fence.
    // Its pinned runtime request serializes with any Open already in transit.
    next.work.cleanup_not_before = None;
    next.work.step = AgentWorkStep::RestartTeardown;
    Ok((next, Document::new(), TEARDOWN))
}

fn cancel_undispatched(
    current: &ControlSnapshot,
    operation: &ControlOperation,
    stamp: &str,
) -> Result<(Agent, Document, &'static str), ApplyError> {
    let conflict = || ApplyError::Domain(AgentControlStoreError::Conflict);
    let (stopped_at, stop_receipt) =
        restore_owner_stop(&current.row).map_err(ApplyError::Domain)?;
    let mut next = current.agent.clone();
    next.status = AgentStatus::Stopped;
    next.stop_reason = Some(AgentStopReason::Owner);
    next.stopped_at = Some(stopped_at);
    next.updated_at = stamp.to_owned();
    next.work.schedule(AgentWorkStep::None, stamp.to_owned());
    let mut receipts = doc! {STOP_RECEIPT_FIELD: stop_receipt};
    if matches!(operation.request, ControlRequest::Delete(_)) {
        next.request_delete(stamp.to_owned())
            .map_err(|_| conflict())?;
        receipts.extend(
            operation
                .delete_after_restart_cancellation()
                .receipt_update(&next)
                .map_err(ApplyError::Domain)?,
        );
    } else {
        next.revision = next.revision.checked_add(1).ok_or_else(conflict)?;
    }
    Ok((next, receipts, CANCELLED))
}

fn domain(error: SandboxStoreError) -> ApplyError {
    ApplyError::Domain(if matches!(error, SandboxStoreError::Conflict) {
        AgentControlStoreError::Conflict
    } else {
        store_error(error)
    })
}

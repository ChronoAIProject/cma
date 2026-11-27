use super::*;
use super::{restart_publication, restart_receipt as receipts};
use crate::store::{
    agent_control_lineage::fence_restart_lineage,
    agent_control_receipt::{
        consume_restart_predecessor, has_owner_stop_receipt, matches_restart_predecessor,
        owner_stop_operation, STOP_RECEIPT_FIELD,
    },
    agent_log_integrity::{AppendCapacity, TransactionLogError},
    agent_log_segments::{
        publication::{self, PublicationIdentity},
        transaction_scope::TransactionScope,
        writer_commit::{AggregateUpdate, PublicationInput},
    },
    sandbox_mongo::{is_duplicate_key, record_bson},
    work_publication::FenceError,
};
use layer2_core::sandbox::{
    store::{AgentRestartAcceptance, AgentRestartRequest, AgentRestartSource},
    AgentLogEntry, AgentLogKind, AgentStatus, AgentWorkStep, SandboxStatus, WorkspaceStatus,
};
use layer3_adapter::{AgentCloseRequest, AgentProcessAction, AgentSlot};
use mongodb::{
    bson::{spec::BinarySubtype, Binary, Timestamp},
    options::ReadConcern,
};

pub(in crate::store) const ACTIVE_RESTART_FIELD: &str = "control_restart_active_v1";

pub(in crate::store) async fn schedule_restart(
    store: &MongoSandboxStore,
    request: AgentRestartRequest,
) -> Result<AgentRestartAcceptance, AgentControlStoreError> {
    if request.caller.subject != request.target.owner
        || !request
            .caller
            .authorization
            .allows(layer2_core::Capability::UseOwnSandboxes)
        || request.claim.subject() != &request.caller.subject
        || request.claim.operation() != format!("agent:owner-restart:{}", request.target.agent_id)
        || !layer2_core::operation::valid_idempotency_key(request.claim.key().as_bytes())
    {
        return Err(AgentControlStoreError::NotFound);
    }
    for attempt in 0..CONTROL_ATTEMPTS {
        super::super::workspace_admission::retry::retry_delay(attempt).await;
        let mut session = store.client.start_session().await.map_err(unavailable)?;
        let scope = match TransactionScope::start(&mut session).await {
            Ok(scope) => scope,
            Err(error) if is_transient(&error) => continue,
            Err(error) => return Err(unavailable(error)),
        };
        let (acceptance, publication, fresh) = match apply(store, &request, scope).await {
            Ok(result) => result,
            Err(error) => {
                let _ = session.abort_guarded().await;
                match error {
                    ApplyError::Driver(error)
                        if is_transient(&error) || is_duplicate_key(&error) =>
                    {
                        continue;
                    }
                    ApplyError::Driver(error) => return Err(unavailable(error)),
                    ApplyError::Domain(error) => return Err(error),
                }
            }
        };
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => return Ok(acceptance),
            CommitOutcome::Retry(_) => continue,
            CommitOutcome::Unknown(_) if !fresh => continue,
            CommitOutcome::Unknown(error) => {
                let at = session
                    .operation_time()
                    .ok_or_else(|| unknown_control(&error))?;
                let store = store.clone();
                return wait_for_reconciliation("agent restart admission", async move {
                    reconcile(&store, &request, acceptance, publication, at).await
                })
                .await
                .map_err(AgentControlStoreError::Unavailable)?;
            }
            CommitOutcome::Failed(error) => return Err(unavailable(error)),
        }
    }
    Err(AgentControlStoreError::Conflict)
}

async fn apply(
    store: &MongoSandboxStore,
    request: &AgentRestartRequest,
    mut scope: TransactionScope<'_>,
) -> Result<
    (
        AgentRestartAcceptance,
        Option<restart_publication::RecordedRestart>,
        bool,
    ),
    ApplyError,
> {
    let current = scope.control_snapshot(store, &request.target).await?;
    let records = receipts::records(store);
    if let Some(row) = scope
        .find_one(&records, doc! {"_id": receipts::key(&request.claim)})
        .await
        .map_err(ApplyError::Driver)?
    {
        let acceptance = receipts::decode(&request.claim, &row).map_err(ApplyError::Domain)?;
        validate_replay(&current, &acceptance).map_err(ApplyError::Domain)?;
        return Ok((acceptance, None, false));
    }

    if request.observed_process.is_none() {
        return Err(ApplyError::Domain(
            AgentControlStoreError::RestartEvidenceRequired,
        ));
    }
    let (mut next, acceptance, interruption) =
        plan(&current, request).map_err(ApplyError::Domain)?;
    // New command reservations update the Agent in the same transaction as the
    // lane order. The Agent CAS forces either admission to retry on overlap.
    let command_order = scope
        .find_one(
            &store.agent_command_orders,
            doc! {"_id": current.agent.id.as_str()},
        )
        .await
        .map_err(ApplyError::Driver)?
        .as_ref()
        .map(|row| crate::store::agent_commands::lane_records::read(row, current.agent.id.as_str()))
        .transpose()
        .map_err(|error| ApplyError::Domain(AgentControlStoreError::Corrupt(error.to_string())))?
        .map(|lane| lane.last_order);
    if command_order < current.agent.work.pre_restart_command_order {
        return Err(ApplyError::Domain(AgentControlStoreError::Corrupt(
            "Agent command order regressed before restart".into(),
        )));
    }
    next.work.pre_restart_command_order = command_order;
    scope
        .fence_ordinary_work(store)
        .await
        .map_err(|error| match error {
            FenceError::Domain(error) => ApplyError::Domain(store_error(error)),
            FenceError::Driver(error) => ApplyError::Driver(error),
        })?;
    let live = scope
        .live_agents(
            store,
            &current.agent.sandbox_id,
            &current.agent.workspace_id,
        )
        .await
        .map_err(log_error)?;
    let limit = current.sandbox.limits.max_agents_per_workspace;
    if live >= u64::from(limit) {
        return Err(ApplyError::Domain(AgentControlStoreError::LimitReached {
            limit,
            live: u8::try_from(live).unwrap_or(u8::MAX),
        }));
    }
    let row = &current.row;
    let mut logs = interruption
        .into_iter()
        .map(|kind| AgentLogEntry {
            agent_id: next.id.clone(),
            seq: next.conversation.last_seq,
            appended_at: acceptance.accepted_at.clone(),
            kind,
        })
        .collect();
    crate::store::agent_conversation_store::checkpoint::finish_published_text(
        row,
        &current.agent,
        &mut next,
        &mut logs,
    )
    .map_err(store_error)
    .map_err(ApplyError::Domain)?;
    let revision = signed(current.agent.revision, "Agent revision")
        .map_err(AgentControlStoreError::Corrupt)
        .map_err(ApplyError::Domain)?;
    let sequence = signed(current.agent.conversation.last_seq, "Agent log sequence")
        .map_err(AgentControlStoreError::Corrupt)
        .map_err(ApplyError::Domain)?;
    let filter = doc! {
        "_id": current.agent.id.as_str(),
        "revision": revision,
        "status": "stopped",
        "last_seq": sequence,
        "record.conversation.last_seq": sequence,
        STOP_RECEIPT_FIELD: {"$exists": true},
        ACTIVE_RESTART_FIELD: {"$exists": false},
        "channel_reuse_cleanup_execution": {"$exists": false},
        "channel_history_seal": {"$exists": false},
    };
    let mut fields = agent_replacement_fields(&next)
        .map_err(store_error)
        .map_err(ApplyError::Domain)?;
    fields.insert(
        ACTIVE_RESTART_FIELD,
        record_bson(&acceptance)
            .map_err(AgentControlStoreError::Corrupt)
            .map_err(ApplyError::Domain)?,
    );
    let stopped_at = current
        .agent
        .stopped_at
        .as_deref()
        .ok_or(ApplyError::Domain(AgentControlStoreError::Conflict))?;
    fields.extend(
        consume_restart_predecessor(&current.receipts, stopped_at, &acceptance)
            .map_err(ApplyError::Domain)?,
    );
    crate::store::agent_conversation_store::carry_checkpoint_fields(
        row,
        &current.agent,
        &next,
        &mut fields,
    )
    .map_err(store_error)
    .map_err(ApplyError::Domain)?;
    let identity = PublicationIdentity {
        key: acceptance.operation_id.clone(),
        fingerprint: Binary {
            subtype: BinarySubtype::Generic,
            bytes: request.claim.fingerprint().to_vec(),
        },
        appended_at: acceptance.accepted_at.clone(),
    };
    let (publication, session) = restart_publication::record(
        store,
        scope,
        row,
        PublicationInput {
            operation: &identity,
            current: &current.agent,
            next: &next,
            logs: &logs,
            capacity: if logs.len() > 1 {
                AppendCapacity::StopOrFailure
            } else {
                AppendCapacity::Normal
            },
            result_digest: publication::result_digest(&acceptance)
                .map_err(store_error)
                .map_err(ApplyError::Domain)?,
        },
        AggregateUpdate { filter, fields },
    )
    .await?;
    // Keep the existing parent write-conflict fence shared with Agent creation.
    fence_restart_lineage(store, &current, &acceptance.operation_id, session).await?;
    let removed = store
        .agents
        .update_one(
            doc! {
            "_id": current.agent.id.as_str(),
            "revision": signed(next.revision, "restart revision").map_err(AgentControlStoreError::Corrupt).map_err(ApplyError::Domain)?,
            STOP_RECEIPT_FIELD: {"$exists": true},
        },
            doc! {"$unset": {STOP_RECEIPT_FIELD: ""}},
        )
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?;
    if removed.matched_count != 1 {
        return Err(ApplyError::Domain(AgentControlStoreError::Conflict));
    }
    records
        .insert_one(receipts::encode(&request.claim, &acceptance).map_err(ApplyError::Domain)?)
        .session(session)
        .await
        .map_err(ApplyError::Driver)?;
    Ok((acceptance, publication, true))
}

fn plan(
    current: &ControlSnapshot,
    request: &AgentRestartRequest,
) -> Result<(Agent, AgentRestartAcceptance, Option<AgentLogKind>), AgentControlStoreError> {
    let source = request
        .observed_process
        .as_ref()
        .ok_or(AgentControlStoreError::Conflict)?;
    let agent = &current.agent;
    let sandbox = &current.sandbox;
    let home = sandbox
        .lifecycle
        .home
        .as_ref()
        .and_then(|home| home.attachment())
        .and_then(|home| home.identity().ok());
    if request.target.expected_status != AgentStatus::Stopped
        || agent.revision != request.target.expected_revision
        || current.workspace.status != WorkspaceStatus::Ready
        || sandbox.status != SandboxStatus::Active
        || !sandbox.credential.delivered
        || sandbox.lifecycle.operation.is_some()
        || sandbox.lifecycle.checkpoint_capture.is_some()
        || source.runtime.runtime.validate().is_err()
        || source.runtime.runtime.sandbox_id != sandbox.id.as_str()
        || home.as_ref() != Some(&source.runtime.runtime.home_identity)
        || sandbox.runtime.as_ref().is_none_or(|binding| {
            binding.boot_id.as_ref() != Some(&source.runtime.boot_id)
                || binding.provision_generation != Some(source.runtime.runtime.generation)
        })
        || current.worktree.revision != source.worktree_revision
        || current.workspace.revision != source.workspace_revision
        || sandbox.revision != source.sandbox_revision
        || agent.conversation.conversation_id.as_ref() != Some(&source.conversation_id)
        || !agent.work.receipts.contains(&source.predecessor_open)
        || !agent.work.receipts.contains(&source.predecessor_close)
    {
        return Err(AgentControlStoreError::Conflict);
    }
    let closed_at = layer2_core::seconds_from_rfc3339(&source.predecessor_close.completed_at)
        .ok_or(AgentControlStoreError::Conflict)?;
    if agent
        .stopped_at
        .as_deref()
        .and_then(layer2_core::seconds_from_rfc3339)
        .is_none_or(|stopped_at| closed_at > stopped_at)
        || agent
            .work
            .receipts
            .iter()
            .filter(|receipt| receipt.step == AgentWorkStep::CloseProcess)
            .count()
            != 1
    {
        return Err(AgentControlStoreError::Conflict);
    }
    let next_incarnation = successor(agent, source).ok_or(AgentControlStoreError::Conflict)?;
    if !agent
        .close_process_request()
        .is_ok_and(|close| close.operation_id == source.predecessor_close.operation_id)
    {
        return Err(AgentControlStoreError::Conflict);
    }
    let has_stopped_turn = agent.conversation.active_turn.is_some();
    if has_stopped_turn && !has_owner_stop_receipt(&current.row, agent) {
        return Err(AgentControlStoreError::RestartTurnUnsettled);
    }
    let (next, interruption) = agent
        .plan_owner_restart(&current.worktree, request.requested_at.clone())
        .map_err(|_| {
            if has_stopped_turn {
                AgentControlStoreError::RestartTurnUnsettled
            } else {
                AgentControlStoreError::Conflict
            }
        })?;
    let acceptance = AgentRestartAcceptance {
        operation_id: receipts::operation_id(&request.claim),
        agent_id: agent.id.clone(),
        worktree_id: agent.worktree_id.clone(),
        revision: next.revision,
        accepted_at: request.requested_at.clone(),
        source: source.clone(),
        next_incarnation,
        predecessor_stop_operation: owner_stop_operation(&current.receipts)
            .ok_or(AgentControlStoreError::Conflict)?
            .to_owned(),
    };
    Ok((next, acceptance, interruption))
}

fn validate_replay(
    snapshot: &ControlSnapshot,
    acceptance: &AgentRestartAcceptance,
) -> Result<(), AgentControlStoreError> {
    let agent = &snapshot.agent;
    if acceptance.agent_id != agent.id
        || acceptance.worktree_id != agent.worktree_id
        || acceptance.revision == 0
        || acceptance.revision > agent.revision
        || layer2_core::seconds_from_rfc3339(&acceptance.accepted_at).is_none()
        || successor(agent, &acceptance.source) != Some(acceptance.next_incarnation)
        || acceptance.source.runtime.runtime.sandbox_id != agent.sandbox_id.as_str()
        || agent.conversation.conversation_id.as_ref() != Some(&acceptance.source.conversation_id)
    {
        return Err(AgentControlStoreError::Corrupt(
            "restart acceptance does not match the Agent".into(),
        ));
    }
    if acceptance.revision == agent.revision {
        let active = snapshot.row.get(ACTIVE_RESTART_FIELD).and_then(|value| {
            mongodb::bson::deserialize_from_bson::<AgentRestartAcceptance>(value.clone()).ok()
        });
        if agent.status != AgentStatus::Starting
            || agent.work.step != AgentWorkStep::RestartConversation
            || active.as_ref() != Some(acceptance)
            || snapshot.row.contains_key(STOP_RECEIPT_FIELD)
            || !matches_restart_predecessor(&snapshot.row, acceptance)
        {
            return Err(AgentControlStoreError::Corrupt(
                "restart acceptance does not match the scheduled work".into(),
            ));
        }
    }
    Ok(())
}

fn successor(agent: &Agent, source: &AgentRestartSource) -> Option<u64> {
    let open = &source.predecessor_open;
    let close = &source.predecessor_close;
    let incarnation =
        AgentProcessAction::Open.incarnation(agent.id.as_str(), &open.operation_id)?;
    let canonical = AgentCloseRequest::new(
        AgentProcessAction::Close.operation_id(agent.id.as_str(), incarnation),
        AgentSlot::new(agent.id.as_str()),
    );
    if open.step != AgentWorkStep::OpenConversation
        || close.step != AgentWorkStep::CloseProcess
        || close.operation_id != canonical.operation_id
        || close.request_sha256 != canonical.request_sha256
        || open.request_sha256.len() != 64
        || !open
            .request_sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || layer2_core::seconds_from_rfc3339(&close.completed_at)?
            < layer2_core::seconds_from_rfc3339(&open.completed_at)?
    {
        return None;
    }
    incarnation.checked_add(1)
}

async fn reconcile(
    store: &MongoSandboxStore,
    request: &AgentRestartRequest,
    acceptance: AgentRestartAcceptance,
    publication: Option<restart_publication::RecordedRestart>,
    at: Timestamp,
) -> Result<AgentRestartAcceptance, AgentControlStoreError> {
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .snapshot_time(at)
        .await
        .map_err(unavailable)?;
    let snapshot = load_control_snapshot(store, &request.target, &mut session, true)
        .await
        .map_err(|error| match ApplyError::from(error) {
            ApplyError::Domain(error) => error,
            ApplyError::Driver(error) => unavailable(error),
        })?;
    let row = receipts::records(store)
        .find_one(doc! {"_id": receipts::key(&request.claim)})
        .read_concern(ReadConcern::snapshot())
        .session(&mut session)
        .await
        .map_err(unavailable)?
        .ok_or_else(|| unknown_control("original restart acceptance is absent"))?;
    let recorded = receipts::decode(&request.claim, &row)?;
    validate_replay(&snapshot, &recorded)?;
    if recorded != acceptance {
        return Err(unknown_control(
            "acceptance differs from the original restart proposal",
        ));
    }
    if let Some(publication) = publication {
        publication.verify(store, &mut session).await?;
    }
    Ok(recorded)
}

fn log_error(error: TransactionLogError) -> ApplyError {
    match error {
        TransactionLogError::Driver(error) => ApplyError::Driver(error),
        TransactionLogError::Domain(error) => ApplyError::Domain(store_error(error)),
    }
}

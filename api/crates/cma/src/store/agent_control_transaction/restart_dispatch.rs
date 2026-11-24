use super::*;
use crate::store::{
    agent_control_lineage::fence_restart_lineage,
    agent_control_receipt::matches_restart_predecessor,
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
use layer2_core::sandbox::{
    store::{AgentControlTarget, AgentRestartAcceptance, AgentRestartDispatch, AgentRestartWork},
    AgentCleanupDebt, AgentStatus, AgentWorkStep, SandboxStatus, WorkspaceStatus,
};
use layer3_adapter::{AgentOpenRequest, AgentProcessAction};
use mongodb::{
    bson::{Bson, Document, Timestamp},
    options::ReadConcern,
};

use super::restart::ACTIVE_RESTART_FIELD;

pub(in crate::store) const DISPATCH_FIELD: &str = "control_restart_dispatch_v1";

pub(in crate::store) async fn load_restart_connection(
    store: &MongoSandboxStore,
    agent: &Agent,
) -> Result<Option<AgentOpenRequest>, SandboxStoreError> {
    let row = store
        .agents
        .find_one(doc! {"_id": agent.id.as_str()})
        .await
        .map_err(driver_domain)?
        .ok_or(SandboxStoreError::NotFound)?;
    if read_agent(&row)? != *agent {
        return Err(SandboxStoreError::Conflict);
    }
    row.get(crate::store::agent_worker_transaction::restart_ready::CONNECTION_FIELD)
        .map(|value| {
            mongodb::bson::deserialize_from_bson(value.clone()).map_err(|_| {
                SandboxStoreError::Corrupt("stored Agent restart Open is invalid".into())
            })
        })
        .transpose()
}

pub(in crate::store) async fn load_restart_work(
    store: &MongoSandboxStore,
    claimed: &Agent,
) -> Result<AgentRestartWork, SandboxStoreError> {
    let row = store
        .agents
        .find_one(doc! {"_id": claimed.id.as_str()})
        .await
        .map_err(driver_domain)?
        .ok_or(SandboxStoreError::NotFound)?;
    if read_agent(&row)? != *claimed || claimed.work.lease.is_none() {
        return Err(SandboxStoreError::Conflict);
    }
    read_work(&row, claimed)
}

pub(in crate::store) async fn prepare_restart_dispatch(
    store: &MongoSandboxStore,
    request: &AgentRestartDispatch,
) -> Result<Agent, SandboxStoreError> {
    for attempt in 0..CONTROL_ATTEMPTS {
        crate::store::workspace_admission::retry::retry_delay(attempt).await;
        let mut session = store.client.start_session().await.map_err(driver_domain)?;
        let scope = match TransactionScope::start(&mut session).await {
            Ok(scope) => scope,
            Err(error) if is_transient(&error) => continue,
            Err(error) => return Err(driver_domain(error)),
        };
        let (next, publication, fresh) = match apply(store, request, scope).await {
            Ok(output) => output,
            Err(error) => {
                let _ = session.abort_guarded().await;
                match error {
                    DispatchError::Driver(error) if is_transient(&error) => continue,
                    DispatchError::Driver(error) => return Err(driver_domain(error)),
                    DispatchError::Domain(error) => return Err(error),
                }
            }
        };
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => return Ok(next),
            CommitOutcome::Retry(_) => continue,
            CommitOutcome::Unknown(_) if !fresh => continue,
            CommitOutcome::Unknown(error) => {
                let at = session
                    .operation_time()
                    .ok_or_else(|| driver_domain(error))?;
                let store = store.clone();
                let request = request.clone();
                return wait_for_reconciliation("agent restart dispatch", async move {
                    reconcile(&store, &request, &next, publication, at).await
                })
                .await
                .map_err(SandboxStoreError::Unavailable)?;
            }
            CommitOutcome::Failed(error) => return Err(driver_domain(error)),
        }
    }
    Err(SandboxStoreError::Conflict)
}

async fn apply(
    store: &MongoSandboxStore,
    request: &AgentRestartDispatch,
    mut scope: TransactionScope<'_>,
) -> Result<(Agent, Option<Recorded>, bool), DispatchError> {
    let target = AgentControlTarget::from(&request.claimed);
    let current = scope
        .control_snapshot(store, &target)
        .await
        .map_err(lineage_error)?;
    if current.agent != request.claimed {
        return Err(SandboxStoreError::Conflict.into());
    }
    let work = read_work(&current.row, &current.agent)?;
    if work != request.work {
        return Err(SandboxStoreError::Conflict.into());
    }
    let lease = current
        .agent
        .work
        .lease
        .as_ref()
        .ok_or(SandboxStoreError::Conflict)?;
    let stamp = layer2_core::seconds_from_rfc3339(&request.dispatched_at)
        .ok_or(SandboxStoreError::Conflict)?;
    if layer2_core::seconds_from_rfc3339(&lease.expires_at).is_none_or(|expires| expires <= stamp)
        || layer2_core::seconds_from_rfc3339(&current.agent.updated_at)
            .is_none_or(|updated| stamp < updated)
        || !valid_open(&current.agent, &work.acceptance, &request.open)
        || request.open.options
            != crate::sandbox::runtime_options(&current.agent, &current.sandbox, &current.worktree)
        || current.sandbox.status != SandboxStatus::Active
        || current.sandbox.lifecycle.operation.is_some()
        || current.sandbox.lifecycle.checkpoint_capture.is_some()
        || !current.sandbox.credential.delivered
        || current.sandbox.runtime.as_ref().is_none_or(|binding| {
            binding.boot_id.as_ref() != Some(&work.acceptance.source.runtime.boot_id)
                || binding.provision_generation
                    != Some(work.acceptance.source.runtime.runtime.generation)
        })
        || current
            .sandbox
            .lifecycle
            .home
            .as_ref()
            .and_then(|home| home.attachment())
            .and_then(|home| home.identity().ok())
            .as_ref()
            != Some(&work.acceptance.source.runtime.runtime.home_identity)
        || current.worktree.revision != work.acceptance.source.worktree_revision
        || current.workspace.status != WorkspaceStatus::Ready
        || current
            .agent
            .validate_prerequisites(&current.worktree)
            .is_err()
    {
        return Err(SandboxStoreError::Conflict.into());
    }
    scope
        .fence_ordinary_work(store)
        .await
        .map_err(|error| match error {
            FenceError::Domain(error) => DispatchError::Domain(error),
            FenceError::Driver(error) => DispatchError::Driver(error),
        })?;

    let marker = format!(
        "{}:dispatch:{}",
        work.acceptance.operation_id,
        uuid::Uuid::new_v4().simple()
    );
    if let Some(dispatched) = work.dispatched_open {
        if dispatched != request.open
            || scope
                .find_one(
                    &store.agents,
                    doc! {
                        "_id": current.agent.id.as_str(),
                        "$expr": live_lease_expression(),
                    },
                )
                .await
                .map_err(DispatchError::Driver)?
                .is_none()
        {
            return Err(SandboxStoreError::Conflict.into());
        }
        let session = scope.release();
        fence_dispatch_owner(store, &current.agent, session).await?;
        fence_restart_lineage(store, &current, &marker, session)
            .await
            .map_err(lineage_error)?;
        return Ok((current.agent, None, false));
    }

    let mut next = current.agent.clone();
    next.revision = next
        .revision
        .checked_add(1)
        .ok_or(SandboxStoreError::Conflict)?;
    next.updated_at.clone_from(&request.dispatched_at);
    next.work.receipts.retain(|receipt| {
        !matches!(
            receipt.step,
            AgentWorkStep::OpenConversation | AgentWorkStep::CloseProcess,
        )
    });
    next.work.cleanup_debt.push(AgentCleanupDebt {
        step: AgentWorkStep::OpenConversation,
        operation_id: request.open.operation_id.clone(),
        request_sha256: request.open.request_sha256.clone(),
    });
    let mut fields = agent_replacement_fields(&next)?;
    fields.insert(
        DISPATCH_FIELD,
        record_bson(&request.open).map_err(SandboxStoreError::Corrupt)?,
    );
    crate::store::agent_conversation_store::carry_checkpoint_fields(
        &current.row,
        &current.agent,
        &next,
        &mut fields,
    )?;
    let revision =
        signed(current.agent.revision, "Agent revision").map_err(SandboxStoreError::Corrupt)?;
    let fence =
        signed(lease.fence, "restart dispatch fence").map_err(SandboxStoreError::Corrupt)?;
    let sequence = signed(next.conversation.last_seq, "Agent log sequence")
        .map_err(SandboxStoreError::Corrupt)?;
    let mut filter = doc! {
        "_id": next.id.as_str(),
        "revision": revision,
        "last_seq": sequence,
        "record.conversation.last_seq": sequence,
        "work_lease_holder": &lease.holder,
        "work_lease_fence": fence,
        "work_lease_expires_at": &lease.expires_at,
        "channel_reuse_cleanup_execution": {"$exists": false},
        "channel_history_seal": {"$exists": false},
        DISPATCH_FIELD: {"$exists": false},
        "$expr": live_lease_expression(),
    };
    let publication = if crate::store::agent_log_segments::read_format(&current.row)?.is_some() {
        let identity = PublicationIdentity {
            key: format!("{}:dispatch", work.acceptance.operation_id),
            fingerprint: publication::request_digest(&request.open)?,
            appended_at: request.dispatched_at.clone(),
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
                    capacity: AppendCapacity::Normal,
                    result_digest: publication::result_digest(&next)?,
                },
                AggregateUpdate { filter, fields },
            )
            .await
            .map_err(log_error)?,
        )
    } else {
        filter.insert("document_schema_version", DOCUMENT_SCHEMA_VERSION);
        carry_integrity_for_revision(&current.row, &current.agent, &next, &mut fields)?;
        let updated = scope
            .update_one(&store.agents, filter, doc! {"$set": fields})
            .await
            .map_err(DispatchError::Driver)?;
        if updated.matched_count != 1 {
            return Err(SandboxStoreError::Conflict.into());
        }
        None
    };
    let session = scope.release();
    fence_dispatch_owner(store, &current.agent, session).await?;
    fence_restart_lineage(store, &current, &marker, session)
        .await
        .map_err(lineage_error)?;
    Ok((next, publication, true))
}

async fn fence_dispatch_owner(
    store: &MongoSandboxStore,
    agent: &Agent,
    session: &mut mongodb::ClientSession,
) -> Result<(), DispatchError> {
    store
        .fence_agent_effect_transaction(&agent.id, session)
        .await
        .map_err(|error| match error {
            crate::store::agent_ownership::FenceError::Domain(error) => {
                DispatchError::Domain(error)
            }
            crate::store::agent_ownership::FenceError::Driver(error) => {
                DispatchError::Driver(error)
            }
        })
}

pub(in crate::store) fn live_lease_expression() -> Document {
    doc! {"$gt": [
        {"$dateFromString": {
            "dateString": "$work_lease_expires_at",
            "onError": Bson::Null,
            "onNull": Bson::Null,
        }},
        "$$NOW",
    ]}
}

pub(in crate::store) fn read_work(
    row: &Document,
    agent: &Agent,
) -> Result<AgentRestartWork, SandboxStoreError> {
    if agent.status != AgentStatus::Starting
        || agent.work.step != AgentWorkStep::RestartConversation
        || agent.requested_stop_reason.is_some()
    {
        return Err(SandboxStoreError::Conflict);
    }
    read_restart_record(row, agent)
}

pub(super) fn read_restart_record(
    row: &Document,
    agent: &Agent,
) -> Result<AgentRestartWork, SandboxStoreError> {
    let invalid =
        || SandboxStoreError::Corrupt("stored restart and its Agent work disagree".into());
    let acceptance: AgentRestartAcceptance = mongodb::bson::deserialize_from_bson(
        row.get(ACTIVE_RESTART_FIELD).ok_or_else(invalid)?.clone(),
    )
    .map_err(|_| invalid())?;
    if acceptance.agent_id != agent.id
        || acceptance.worktree_id != agent.worktree_id
        || acceptance.revision > agent.revision
        || acceptance.source.runtime.runtime.sandbox_id != agent.sandbox_id.as_str()
        || agent.conversation.conversation_id.as_ref() != Some(&acceptance.source.conversation_id)
        || !matches_restart_predecessor(row, &acceptance)
    {
        return Err(invalid());
    }
    let dispatched_open: Option<AgentOpenRequest> = row
        .get(DISPATCH_FIELD)
        .map(|value| mongodb::bson::deserialize_from_bson(value.clone()).map_err(|_| invalid()))
        .transpose()?;
    match &dispatched_open {
        Some(open)
            if valid_open(agent, &acceptance, open)
                && matches!(agent.work.cleanup_debt.as_slice(), [debt]
                if debt.step == AgentWorkStep::OpenConversation
                && debt.operation_id == open.operation_id && debt.request_sha256 == open.request_sha256)
                && !agent.work.receipts.iter().any(|receipt| {
                    matches!(
                        receipt.step,
                        AgentWorkStep::OpenConversation | AgentWorkStep::CloseProcess
                    )
                }) => {}
        None if agent.work.cleanup_debt.is_empty()
            && agent
                .work
                .receipts
                .contains(&acceptance.source.predecessor_open)
            && agent
                .work
                .receipts
                .contains(&acceptance.source.predecessor_close) => {}
        _ => return Err(invalid()),
    }
    Ok(AgentRestartWork {
        acceptance,
        dispatched_open,
    })
}

fn valid_open(agent: &Agent, acceptance: &AgentRestartAcceptance, open: &AgentOpenRequest) -> bool {
    acceptance.next_incarnation > 1
        && AgentProcessAction::Open.incarnation(agent.id.as_str(), &open.operation_id)
            == Some(acceptance.next_incarnation)
        && open.options.slot.as_str() == agent.id.as_str()
        && open.has_valid_sha256()
        && open.resume_conversation.as_ref() == Some(&acceptance.source.conversation_id)
        && open.restart_runtime.as_ref() == Some(&acceptance.source.runtime)
}

async fn reconcile(
    store: &MongoSandboxStore,
    request: &AgentRestartDispatch,
    expected: &Agent,
    publication: Option<Recorded>,
    at: Timestamp,
) -> Result<Agent, SandboxStoreError> {
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .snapshot_time(at)
        .await
        .map_err(driver_domain)?;
    let row = store
        .agents
        .find_one(doc! {"_id": expected.id.as_str()})
        .read_concern(ReadConcern::snapshot())
        .session(&mut session)
        .await
        .map_err(driver_domain)?
        .ok_or(SandboxStoreError::NotFound)?;
    let agent = read_agent(&row)?;
    let observed = read_work(&row, &agent)?;
    if agent != *expected
        || observed.acceptance != request.work.acceptance
        || observed.dispatched_open.as_ref() != Some(&request.open)
    {
        return Err(SandboxStoreError::Conflict);
    }
    if let Some(publication) = publication {
        if !publication.matches(store, &mut session).await? {
            return Err(SandboxStoreError::Conflict);
        }
    }
    Ok(agent)
}

enum DispatchError {
    Domain(SandboxStoreError),
    Driver(mongodb::error::Error),
}

impl From<SandboxStoreError> for DispatchError {
    fn from(value: SandboxStoreError) -> Self {
        Self::Domain(value)
    }
}

fn lineage_error(error: LineageError) -> DispatchError {
    match error {
        LineageError::Driver(error) => DispatchError::Driver(error),
        LineageError::ReceiptCorrupt(detail)
        | LineageError::Domain(AgentControlStoreError::Corrupt(detail)) => {
            SandboxStoreError::Corrupt(detail).into()
        }
        LineageError::Domain(AgentControlStoreError::NotFound) => {
            SandboxStoreError::NotFound.into()
        }
        LineageError::Domain(AgentControlStoreError::Unavailable(detail)) => {
            SandboxStoreError::Unavailable(detail).into()
        }
        LineageError::Domain(_) => SandboxStoreError::Conflict.into(),
    }
}

fn log_error(error: TransactionLogError) -> DispatchError {
    match error {
        TransactionLogError::Driver(error) => DispatchError::Driver(error),
        TransactionLogError::Domain(error) => error.into(),
    }
}

fn driver_domain(error: mongodb::error::Error) -> SandboxStoreError {
    SandboxStoreError::Unavailable(error.to_string())
}

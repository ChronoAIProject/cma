use layer2_core::sandbox::agent_io::{
    command_ready_for_claim, AdmittedAgentCommand, AgentCommand, CommandClaim, CommandClaimRequest,
    CommandLane, CommandLedgerError,
};
use layer2_core::{sandbox::Agent, AgentId};
use mongodb::bson::doc;
use mongodb::ClientSession;

use super::super::agents::read_agent;
use super::super::sandbox_mongo::MongoSandboxStore;
use super::worker::Change;
use super::{lane_records, records, ApplyError};

pub(in crate::store) async fn apply(
    store: &MongoSandboxStore,
    change: &Change,
    session: &mut ClientSession,
) -> Result<super::worker::TransactionResult, ApplyError> {
    if let Change::Reconcile(id) = change {
        let (command, outcome) = super::reconcile::apply_observed(store, id, session).await?;
        return Ok((Some(command), Some(outcome)));
    }
    apply_mutation(store, change, session)
        .await
        .map(|command| (command, None))
}

async fn apply_mutation(
    store: &MongoSandboxStore,
    change: &Change,
    session: &mut ClientSession,
) -> Result<Option<AdmittedAgentCommand>, ApplyError> {
    let (claim, now) = match change {
        Change::Reconcile(_) => unreachable!("reconciliation handled before mutations"),
        Change::Claim { request, observed } => {
            return claim_next(store, request, observed.as_ref(), session).await
        }
        Change::Renew { claim, now, .. }
        | Change::Release { claim, now }
        | Change::Reject { claim, now, .. } => (claim, now),
    };
    let (mut command, mut lane) = load_claim(store, claim, now, session).await?;
    let revision = command.revision;
    match change {
        Change::Renew { expires_at, .. } => {
            let previous = &command.work.lease.as_ref().unwrap().expires_at;
            if layer2_core::seconds_from_rfc3339(expires_at)
                < layer2_core::seconds_from_rfc3339(previous)
            {
                return Err(CommandLedgerError::Invalid.into());
            }
            command.work.lease.as_mut().unwrap().expires_at = expires_at.clone();
            lane.lease.as_mut().unwrap().expires_at = expires_at.clone();
        }
        Change::Release { .. } => {
            command.work.lease = None;
            lane.lease = None;
        }
        Change::Reject { code, .. } => {
            command.reject_before_dispatch(claim, *code, now.clone())?;
            lane.lease = None;
        }
        Change::Claim { .. } | Change::Reconcile(_) => unreachable!(),
    }
    command.revision = revision.checked_add(1).ok_or(CommandLedgerError::Corrupt)?;
    let agent = load_agent(store, &claim.agent_id, session).await?;
    if !fence_parent(store, agent.as_ref(), session).await? {
        return Err(CommandLedgerError::StaleClaim.into());
    }
    write(store, &command, revision, &lane, session).await?;
    Ok(Some(command))
}

pub(super) async fn load_claim(
    store: &MongoSandboxStore,
    claim: &CommandClaim,
    now: &str,
    session: &mut ClientSession,
) -> Result<(AdmittedAgentCommand, CommandLane), ApplyError> {
    cma_am_agent_commands::transaction::load_claim(
        &store.agent_commands,
        &store.agent_command_orders,
        claim,
        now,
        session,
    )
    .await
}

async fn claim_next(
    store: &MongoSandboxStore,
    request: &CommandClaimRequest,
    observed: Option<&(layer2_core::ids::AgentCommandId, u64)>,
    session: &mut ClientSession,
) -> Result<Option<AdmittedAgentCommand>, ApplyError> {
    let Some(lane) = store
        .agent_command_orders
        .find_one(doc! { "_id": request.agent_id.as_str() })
        .session(&mut *session)
        .await?
    else {
        return Ok(None);
    };
    let mut lane = lane_records::read(&lane, request.agent_id.as_str())?;
    if lane
        .lease
        .as_ref()
        .is_some_and(|lease| lease.live_at(&request.now))
    {
        return Ok(None);
    }
    let agent = load_agent(store, &request.agent_id, session).await?;
    let mut candidates = store
        .agent_commands
        .find(doc! { "agent_id": request.agent_id.as_str(),
        "status": { "$in": ["queued", "waiting_action"] } })
        .sort(doc! { "agent_order": 1 })
        .session(&mut *session)
        .await?;
    let mut ordered = Vec::new();
    while let Some(document) = candidates.next(&mut *session).await {
        ordered.push(records::read(&document?)?);
    }
    ordered.sort_by_key(AdmittedAgentCommand::queue_priority);
    let mut message_held = false;
    for mut command in ordered {
        if agent
            .as_ref()
            .is_some_and(|agent| command.predates_owner_restart(agent))
        {
            continue;
        }
        message_held |= command.holds_message_order(&request.now);
        if message_held
            && matches!(
                command.receipt.envelope.command,
                AgentCommand::Message { .. }
            )
        {
            continue;
        }
        if !command_ready_for_claim(&command, agent.as_ref(), &request.now) {
            continue;
        }
        if observed.is_some_and(|(id, revision)| {
            id != &command.receipt.envelope.command_id || *revision != command.revision
        }) {
            return Ok(None);
        }
        let revision = command.revision;
        if !lane.claim(&mut command, request)? {
            return Ok(None);
        }
        if !fence_parent(store, agent.as_ref(), session).await? {
            // An empty selection permits queued preparation in the worker.
            return Err(CommandLedgerError::StaleClaim.into());
        }
        write(store, &command, revision, &lane, session).await?;
        return Ok(Some(command));
    }
    Ok(None)
}

pub(super) async fn load_agent(
    store: &MongoSandboxStore,
    id: &AgentId,
    session: &mut ClientSession,
) -> Result<Option<Agent>, ApplyError> {
    store
        .agents
        .find_one(doc! { "_id": id.as_str() })
        .session(session)
        .await?
        .as_ref()
        .map(read_agent)
        .transpose()
        .map_err(|_| CommandLedgerError::Corrupt.into())
}

pub(super) async fn fence_parent(
    store: &MongoSandboxStore,
    agent: Option<&Agent>,
    session: &mut ClientSession,
) -> Result<bool, ApplyError> {
    // An orphan command remains claimable for terminal refusal.
    let Some(agent) = agent else { return Ok(true) };
    if write_parent_fence(store, agent, session).await? {
        return Ok(true);
    }
    Ok(store
        .sandboxes
        .find_one(doc! { "_id": agent.sandbox_id.as_str() })
        .projection(doc! { "_id": 1 })
        .session(session)
        .await?
        .is_none())
}

pub(super) async fn write_parent_fence(
    store: &MongoSandboxStore,
    agent: &Agent,
    session: &mut ClientSession,
) -> Result<bool, ApplyError> {
    let updated = crate::store::work_lease_update::with_fence(
        &store.sandboxes,
        doc! {
            "_id": agent.sandbox_id.as_str(),
            "document_schema_version": crate::store::work_schema::versions(),
            "owner": agent.owner.as_str(),
            "record.owner": agent.owner.as_str(),
        },
        None,
        "agent_command_worker_fence",
        session,
    )
    .await?;
    Ok(updated.matched_count == 1)
}

pub(in crate::store) async fn write(
    store: &MongoSandboxStore,
    command: &AdmittedAgentCommand,
    expected_revision: u64,
    lane: &CommandLane,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    super::super::budget::release_unexecuted(store, command, session)
        .await
        .map_err(|error| match error {
            super::super::agent_conversation_store::ApplyError::Driver(error) => {
                ApplyError::Driver(error)
            }
            _ => ApplyError::Domain(CommandLedgerError::Unavailable),
        })?;
    // Terminal Trigger projection is completed by the scoped transaction root.
    // Dispatch binding and reconciliation already have dispatch evidence;
    // queued input edits are nonterminal and require no first-message event.
    cma_am_agent_commands::transaction::write(
        &store.agent_commands,
        &store.agent_command_orders,
        command,
        expected_revision,
        lane,
        session,
    )
    .await
}

//! Ordinary publication in the root's exclusive snapshot transaction.
//! Raw session access stays inside this audited adapter. Domain helpers invoked
//! here never start, restart, commit or abort a transaction.
use super::{
    capture, publication, read_format,
    transaction_scope::TransactionScope,
    writer_commit::{self, AggregateUpdate, PublicationInput},
    writer_plan,
};
use crate::store::{
    agent_conversation_store::{self as domain, ApplyError, ConversationReceipt},
    agent_log_integrity::AppendCapacity,
    agent_log_telemetry::ScanCaller,
    agents::{agent_replacement_fields, read_agent},
    sandbox_mongo::MongoSandboxStore,
};
use layer2_core::sandbox::{
    store::{
        AgentConversationCapacitySettlement, AgentConversationMutation, AgentMessageCheckpoint,
        SandboxStoreError,
    },
    Agent,
};
use mongodb::bson::doc;

pub(in crate::store) mod checkpoint;

pub(in crate::store) async fn apply(
    caller: ScanCaller,
    store: &MongoSandboxStore,
    mutation: &AgentConversationMutation,
    capacity: Option<&AgentConversationCapacitySettlement>,
    checkpoint: Option<&AgentMessageCheckpoint>,
    scope: &mut TransactionScope<'_>,
) -> Result<ConversationReceipt, ApplyError> {
    let row = scope
        .find_one(&store.agents, doc! {"_id":mutation.agent.id.as_str()})
        .await
        .map_err(ApplyError::Driver)?
        .ok_or(ApplyError::Domain(SandboxStoreError::NotFound))?;
    if read_format(&row).map_err(ApplyError::Domain)?.is_none() {
        return domain::publication::apply_mutation_with_capacity(
            caller,
            store,
            mutation,
            scope.session(),
            capacity,
        )
        .await;
    }
    let current = read_agent(&row).map_err(ApplyError::Domain)?;
    let captured = capture::capture_transaction(store, &current, scope).await?;
    domain::publication::before(store, &current, mutation, scope.session()).await?;
    if let Some(proof) = capacity {
        proof
            .validate_snapshot(
                &current,
                domain::checkpoint::read(&row)
                    .map_err(ApplyError::Domain)?
                    .as_ref(),
            )
            .map_err(ApplyError::Domain)?;
        // A closed rollover gate is uncertainty, never deterministic capacity.
        // Only a batch that cannot fit a fresh normal segment permits settlement.
        match writer_plan::needs_normal_rollover(captured.active_head(), proof.refused()) {
            Err(SandboxStoreError::AgentLogCapacityExceeded) => {}
            Ok(_) => return Err(ApplyError::Domain(SandboxStoreError::Conflict)),
            Err(error) => return Err(ApplyError::Domain(error)),
        }
    } else {
        domain::validate_mutation(&current, captured.through(), mutation)
            .map_err(ApplyError::Domain)?;
    }
    domain::publication::require_cleanup(&row, &current, mutation)?;
    let logs = domain::conversation_logs(mutation).map_err(ApplyError::Domain)?;
    let command_dispatch = domain::publication::bind_domain(
        store,
        &current,
        captured.projection().latest_turn,
        mutation,
        capacity.is_some(),
        scope.session(),
    )
    .await?;
    let operation = identity(mutation, capacity.is_some(), checkpoint)?;
    let mut fields = agent_replacement_fields(&mutation.agent).map_err(ApplyError::Domain)?;
    domain::carry_checkpoint_fields(&row, &current, &mutation.agent, &mut fields)
        .map_err(ApplyError::Domain)?;
    let recorded = writer_commit::publish_captured(
        store,
        scope,
        &captured,
        PublicationInput {
            operation: &operation,
            current: &current,
            next: &mutation.agent,
            logs: &logs,
            capacity: if capacity.is_some() {
                AppendCapacity::StopOrFailure
            } else {
                AppendCapacity::Normal
            },
            result_digest: result_digest(mutation, checkpoint)?,
        },
        AggregateUpdate {
            filter: doc! {"_id":current.id.as_str()},
            fields,
        },
    )
    .await?;
    crate::store::suggestions::enqueue(store, &mutation.agent, &logs, scope.session()).await?;
    Ok(ConversationReceipt {
        segmented: Some(recorded),
        command_dispatch,
        agent: mutation.agent.clone(),
        logs,
    })
}

fn identity(
    mutation: &AgentConversationMutation,
    capacity: bool,
    checkpoint: Option<&AgentMessageCheckpoint>,
) -> Result<publication::PublicationIdentity, ApplyError> {
    Ok(publication::PublicationIdentity {
        key: format!(
            "conversation:{}:{}",
            mutation.expected_agent_revision, mutation.expected_last_seq
        ),
        fingerprint: publication::request_digest(&(
            &mutation.agent,
            &mutation.agent_logs,
            &mutation.command_claim,
            mutation.expected_agent_revision,
            mutation.expected_last_seq,
            mutation.runtime_generation,
            &mutation.appended_at,
            capacity,
            checkpoint,
        ))
        .map_err(ApplyError::Domain)?,
        appended_at: mutation.appended_at.clone(),
    })
}

fn result_digest(
    mutation: &AgentConversationMutation,
    checkpoint: Option<&AgentMessageCheckpoint>,
) -> Result<mongodb::bson::Binary, ApplyError> {
    publication::result_digest(&(
        &mutation.agent,
        &mutation.agent_logs,
        &mutation.command_claim,
        checkpoint,
    ))
    .map_err(ApplyError::Domain)
}

pub(super) async fn require_replay(
    store: &MongoSandboxStore,
    mutation: Option<&AgentConversationMutation>,
    checkpoint: &AgentMessageCheckpoint,
    capacity: bool,
    scope: &mut TransactionScope<'_>,
) -> Result<(), ApplyError> {
    let Some(mutation) = mutation else {
        return Ok(());
    };
    let row = scope
        .find_one(&store.agents, doc! {"_id":mutation.agent.id.as_str()})
        .await
        .map_err(ApplyError::Driver)?
        .ok_or(ApplyError::Domain(SandboxStoreError::NotFound))?;
    if read_format(&row).map_err(ApplyError::Domain)?.is_none() {
        return Ok(());
    }
    let operation = identity(mutation, capacity, Some(checkpoint))?;
    let logs = domain::conversation_logs(mutation).map_err(ApplyError::Domain)?;
    let digest = result_digest(mutation, Some(checkpoint))?;
    match super::writer_reconcile::reconcile_transaction(
        store,
        &operation,
        &mutation.agent,
        &logs,
        &digest,
        scope.session(),
    )
    .await
    .map_err(ApplyError::Domain)?
    {
        super::writer_reconcile::Outcome::Recorded(_) => Ok(()),
        super::writer_reconcile::Outcome::Unknown => {
            Err(ApplyError::Domain(SandboxStoreError::Unavailable(
                "checkpoint has no exact original segmented publication receipt".into(),
            )))
        }
    }
}

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

pub(in crate::store) mod adoption;
pub(in crate::store) mod checkpoint;

pub(in crate::store) enum PublicationSource {
    Conversation,
    Checkpoint,
    StaleTurn,
    TurnPolicy,
}

pub(in crate::store) async fn apply(
    source: PublicationSource,
    store: &MongoSandboxStore,
    mutation: &AgentConversationMutation,
    capacity: Option<&AgentConversationCapacitySettlement>,
    checkpoint: Option<&AgentMessageCheckpoint>,
    adoption: Option<&adoption::Prepared>,
    scope: &mut TransactionScope<'_>,
) -> Result<ConversationReceipt, ApplyError> {
    if mutation.agent.conversation.turn_timeout.is_some() {
        crate::store::turn_timeouts::floor::retain(store, scope.session())
            .await
            .map_err(ApplyError::Driver)?;
    }
    let row = scope
        .find_one(&store.agents, doc! {"_id":mutation.agent.id.as_str()})
        .await
        .map_err(ApplyError::Driver)?
        .ok_or(ApplyError::Domain(SandboxStoreError::NotFound))?;
    let current = read_agent(&row).map_err(ApplyError::Domain)?;
    if !matches!(source, PublicationSource::Checkpoint)
        && current.work.step.fences_conversation_output()
    {
        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
    }
    if !matches!(source, PublicationSource::TurnPolicy) {
        layer2_core::sandbox::turn_timeout::validate_ordinary_publication(&current, mutation)
            .map_err(ApplyError::Domain)?;
    }
    let exact_checkpoint = if matches!(
        source,
        PublicationSource::StaleTurn | PublicationSource::TurnPolicy
    ) {
        Some(checkpoint.ok_or(ApplyError::Domain(SandboxStoreError::Conflict))?)
    } else {
        None
    };
    if read_format(&row).map_err(ApplyError::Domain)?.is_none() && adoption.is_none() {
        let caller = match source {
            PublicationSource::Conversation => ScanCaller::ConversationPublication,
            PublicationSource::Checkpoint
            | PublicationSource::StaleTurn
            | PublicationSource::TurnPolicy => ScanCaller::Checkpoint,
        };
        return Box::pin(domain::publication::apply_mutation_with_capacity(
            caller,
            store,
            mutation,
            scope.session(),
            capacity,
            exact_checkpoint,
        ))
        .await;
    }
    Box::pin(apply_segmented(
        store,
        mutation,
        capacity,
        checkpoint,
        adoption,
        scope,
        (row, exact_checkpoint),
    ))
    .await
}

// Keep the two format implementations in separate futures. A legacy call must
// not reserve the segmented planner's temporary stack frames while it polls.
async fn apply_segmented(
    store: &MongoSandboxStore,
    mutation: &AgentConversationMutation,
    capacity: Option<&AgentConversationCapacitySettlement>,
    checkpoint: Option<&AgentMessageCheckpoint>,
    adoption: Option<&adoption::Prepared>,
    scope: &mut TransactionScope<'_>,
    source: (mongodb::bson::Document, Option<&AgentMessageCheckpoint>),
) -> Result<ConversationReceipt, ApplyError> {
    let (row, exact_checkpoint) = source;
    let current = read_agent(&row).map_err(ApplyError::Domain)?;
    let captured = Box::pin(capture::capture_transaction(store, &current, scope)).await?;
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
    if exact_checkpoint.is_some()
        && writer_plan::needs_normal_rollover(captured.active_head(), &logs)
            .map_err(ApplyError::Domain)?
    {
        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
    }
    let command_dispatch = Box::pin(domain::publication::bind_domain(
        store,
        &current,
        captured.projection().latest_turn,
        mutation,
        capacity.is_some(),
        scope.session(),
    ))
    .await?;
    let operation = identity(mutation, capacity.is_some(), checkpoint)?;
    let mut fields = agent_replacement_fields(&mutation.agent).map_err(ApplyError::Domain)?;
    if let Some(value) = exact_checkpoint {
        domain::checkpoint::set_field(&mut fields, value).map_err(ApplyError::Domain)?;
    } else {
        domain::carry_checkpoint_fields(&row, &current, &mutation.agent, &mut fields)
            .map_err(ApplyError::Domain)?;
    }
    let recorded = Box::pin(writer_commit::publish_prepared(
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
        adoption.map(|value| (&value.bootstrap, &value.admission)),
    ))
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

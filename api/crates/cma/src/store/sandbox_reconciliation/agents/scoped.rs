//! Scoped segmented Agent publication, composed by the owning transaction root.
use layer2_core::sandbox::{
    store::{SandboxRuntimeLossAgentMutation, SandboxRuntimeLossRequest, SandboxStoreError},
    Agent, AgentLogEntry,
};
use mongodb::bson::doc;

use super::{materialize_logs, ApplyError};
use crate::store::{
    agent_capacity_recovery::source::fence_filter_scoped,
    agent_log_integrity::AppendCapacity,
    agent_log_segments::{
        capture::CapturedLog,
        publication::{self, PublicationIdentity},
        transaction_scope::TransactionScope,
        writer_commit::{self, AggregateUpdate, PublicationInput, Recorded},
    },
    agents::agent_replacement_fields,
    sandbox_mongo::MongoSandboxStore,
};

pub(in crate::store::sandbox_reconciliation) struct PublishedAgent {
    pub agent: Agent,
    pub logs: Vec<AgentLogEntry>,
    pub recorded: Option<Recorded>,
}

/// The caller aborts every error. Captured proof belongs to this scope. No
/// part of the Agent publication can be moved to a separate transaction.
pub(in crate::store::sandbox_reconciliation) async fn publish_agent(
    store: &MongoSandboxStore,
    current: &Agent,
    source: &CapturedLog,
    change: Option<&SandboxRuntimeLossAgentMutation>,
    request: &SandboxRuntimeLossRequest,
    scope: &mut TransactionScope<'_>,
) -> Result<PublishedAgent, ApplyError> {
    if source.agent() != current {
        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
    }
    let mut filter = fence_filter_scoped(store, current, scope).await?;
    let Some(change) = change else {
        let result = scope
            .update_one(
                &store.agents,
                filter,
                doc! {"$inc": {"sandbox_recovery_fence": 1_i64}},
            )
            .await
            .map_err(ApplyError::Driver)?;
        if result.matched_count != 1 {
            return Err(ApplyError::Domain(SandboxStoreError::Conflict));
        }
        return Ok(PublishedAgent {
            agent: current.clone(),
            logs: Vec::new(),
            recorded: None,
        });
    };
    let batch = materialize_logs(change, &request.observed_at)?;
    let restart_fields =
        super::restart::restrict_to_retired_runtime(current, request, &mut filter)?;
    let operation = PublicationIdentity {
        key: format!(
            "runtime-loss:{}:{}:{}",
            request.sandbox_id, request.expected_revision, current.id
        ),
        fingerprint: publication::request_digest(&(
            "runtime-loss/v1",
            &request.sandbox_id,
            request.expected_revision,
            &request.expected_runtime,
            request.retain_runtime,
            request.interruption_cause,
            &request.observed_at,
            &current.id,
        ))
        .map_err(ApplyError::Domain)?,
        appended_at: request.observed_at.clone(),
    };
    scope
        .settle_runtime_loss_budget(store, current, change, &request.observed_at)
        .await?;
    let recorded = writer_commit::publish_captured(
        store,
        scope,
        source,
        PublicationInput {
            operation: &operation,
            current,
            next: &change.agent,
            logs: &batch,
            capacity: AppendCapacity::StopOrFailure,
            result_digest: publication::result_digest(&(&change.agent, &batch))
                .map_err(ApplyError::Domain)?,
        },
        AggregateUpdate {
            filter,
            fields: agent_replacement_fields(&change.agent).map_err(ApplyError::Domain)?,
        },
    )
    .await?;
    if let Some(unset) = restart_fields {
        let updated = scope
            .update_one(
                &store.agents,
                doc! {"_id": current.id.as_str(), "revision": crate::store::sandbox_mongo::signed(
                    change.agent.revision, "retired restart revision",
                ).map_err(|error| ApplyError::Domain(SandboxStoreError::Corrupt(error)))?},
                doc! {"$unset": unset},
            )
            .await
            .map_err(ApplyError::Driver)?;
        if updated.matched_count != 1 {
            return Err(ApplyError::Domain(SandboxStoreError::Conflict));
        }
    }
    Ok(PublishedAgent {
        agent: change.agent.clone(),
        logs: batch,
        recorded: Some(recorded),
    })
}

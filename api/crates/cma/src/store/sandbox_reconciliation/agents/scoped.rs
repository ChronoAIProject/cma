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
        writer_commit::{self, AggregateUpdate, PublicationInput},
    },
    agents::agent_replacement_fields,
    sandbox_mongo::MongoSandboxStore,
};

/// The caller aborts every error. Captured proof belongs to this scope. No
/// part of the Agent publication can be moved to a separate transaction.
pub(in crate::store::sandbox_reconciliation) async fn publish_agent(
    store: &MongoSandboxStore,
    current: &Agent,
    source: &CapturedLog,
    change: Option<&SandboxRuntimeLossAgentMutation>,
    request: &SandboxRuntimeLossRequest,
    scope: &mut TransactionScope<'_>,
) -> Result<(Agent, Vec<AgentLogEntry>), ApplyError> {
    if source.agent() != current {
        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
    }
    let filter = fence_filter_scoped(store, current, scope).await?;
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
        return Ok((current.clone(), Vec::new()));
    };
    let batch = materialize_logs(change, &request.observed_at)?;
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
    let prepared = writer_commit::prepare(
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
        None,
        None,
    )
    .await?;
    writer_commit::persist(
        store,
        scope,
        prepared,
        AggregateUpdate {
            filter,
            fields: agent_replacement_fields(&change.agent).map_err(ApplyError::Domain)?,
        },
    )
    .await?;
    Ok((change.agent.clone(), batch))
}

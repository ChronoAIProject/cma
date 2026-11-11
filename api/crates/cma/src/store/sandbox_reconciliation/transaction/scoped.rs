//! V2 evidence and publications precede release into unchanged legacy writes.
use super::*;
use crate::store::{
    agent_capacity_recovery::{lineage, source::capture_segmented_scoped},
    agent_log_segments::transaction_scope::TransactionScope,
    sandbox_reconciliation::agents::{self, validate_agents},
};

pub(in crate::store::sandbox_reconciliation) async fn apply_scoped(
    store: &MongoSandboxStore,
    request: &SandboxRuntimeLossRequest,
    mut scope: TransactionScope<'_>,
) -> Result<RecoveryApplication, ApplyError> {
    let row = scope
        .find_one(&store.sandboxes, doc! {"_id": request.sandbox_id.as_str()})
        .await
        .map_err(ApplyError::Driver)?
        .ok_or(ApplyError::Domain(SandboxStoreError::NotFound))?;
    let current = read_sandbox(&row).map_err(ApplyError::Domain)?;
    let last_seq = row
        .get_i64("last_seq")
        .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?;
    let last_seq = unsigned(last_seq, "sandbox event sequence")
        .map_err(|error| ApplyError::Domain(SandboxStoreError::Corrupt(error)))?;
    let all = lineage::agents(store, &current.id, &mut scope).await?;
    let evidence = all
        .iter()
        .map(|agent| (agent.clone(), agent.conversation.last_seq))
        .collect::<Vec<_>>();
    let mut segmented = Vec::new();
    let mut legacy = Vec::new();
    for agent in all {
        if let Some(source) = capture_segmented_scoped(store, &agent, &mut scope).await? {
            source
                .projection()
                .validate_aggregate(&agent)
                .map_err(|error| ApplyError::Domain(SandboxStoreError::Corrupt(error)))?;
            segmented.push((agent, source));
        } else {
            legacy.push(agent);
        }
    }
    let mutation = match apply_sandbox_runtime_loss(&current, &evidence, request)
        .map_err(ApplyError::Domain)?
    {
        SandboxRuntimeLossApplication::Complete(outcome) => {
            // Legacy source is still fully validated before a terminal result.
            validate_agents(store, legacy, scope.release()).await?;
            return Ok(RecoveryApplication::Complete(outcome));
        }
        SandboxRuntimeLossApplication::Persist(mutation) => mutation,
    };
    let mut published = Vec::new();
    let mut logs = Vec::new();
    for (current, source) in segmented {
        let change = mutation
            .agents
            .iter()
            .find(|change| change.agent.id == current.id);
        let (agent, batch) =
            agents::scoped::publish_agent(store, &current, &source, change, request, &mut scope)
                .await?;
        published.push(agent);
        logs.extend(batch);
    }
    let session = scope.release();
    // Legacy rows never receive adoption permission from the presence of a v2
    // sibling. Their original validator and append capacity rules still apply.
    let legacy = validate_agents(store, legacy, session).await?;
    let (legacy, legacy_logs) =
        publish_agents(store, legacy, &mutation.agents, request, session).await?;
    published.extend(legacy);
    logs.extend(legacy_logs);
    published.sort_by(|left, right| left.id.cmp(&right.id));
    logs.sort_by(|left, right| (&left.agent_id, left.seq).cmp(&(&right.agent_id, right.seq)));
    persist_sandbox(
        store, request, &current, last_seq, mutation, published, logs, session,
    )
    .await
}

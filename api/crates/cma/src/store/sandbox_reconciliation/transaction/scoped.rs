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
    retirement: Option<&RetirementAction>,
    mut scope: TransactionScope<'_>,
) -> Result<RecoveryApplication, ApplyError> {
    let row = measure("sandbox_read", async {
        scope
            .find_one(&store.sandboxes, doc! {"_id": request.sandbox_id.as_str()})
            .await
    })
    .await
    .map_err(ApplyError::Driver)?
    .ok_or(ApplyError::Domain(SandboxStoreError::NotFound))?;
    let current = read_sandbox(&row).map_err(ApplyError::Domain)?;
    validate_retirement(&row, &current, request, retirement)?;
    if store.runtime_loss_bounded_proof_enabled {
        if let Some(outcome) =
            precheck_sandbox_runtime_loss(&current, request).map_err(ApplyError::Domain)?
        {
            return Ok(RecoveryApplication::Complete(outcome));
        }
    }
    let last_seq = row
        .get_i64("last_seq")
        .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?;
    let last_seq = unsigned(last_seq, "sandbox event sequence")
        .map_err(|error| ApplyError::Domain(SandboxStoreError::Corrupt(error)))?;
    let all = measure(
        "agent_rows",
        lineage::agents(store, &current.id, &mut scope),
    )
    .await?;
    super::super::telemetry::agent_counts(&all);
    if store.runtime_loss_bounded_proof_enabled {
        if let Some(outcome) =
            precheck_runtime_loss_agents(&current, &all, request).map_err(ApplyError::Domain)?
        {
            return Ok(RecoveryApplication::Complete(outcome));
        }
    }
    let evidence = all
        .iter()
        .map(|agent| (agent.clone(), agent.conversation.last_seq))
        .collect::<Vec<_>>();
    validate_retirement_children(&current, &evidence, retirement)?;
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
    let mutation = match apply_sandbox_runtime_loss_with_policy(
        &current,
        &evidence,
        request,
        store.runtime_rotation_recovery_enabled,
    )
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
    let mut receipts = Vec::new();
    for (current, source) in segmented {
        let change = mutation
            .agents
            .iter()
            .find(|change| change.agent.id == current.id);
        let result =
            agents::scoped::publish_agent(store, &current, &source, change, request, &mut scope)
                .await?;
        published.push(result.agent);
        logs.extend(result.logs);
        receipts.extend(result.recorded);
    }
    let session = scope.release();
    // Legacy rows never receive adoption permission from the presence of a v2
    // sibling. Their original validator and append capacity rules still apply.
    let legacy = validate_agents(store, legacy, session).await?;
    let (legacy, legacy_logs) = measure(
        "agent_writes",
        publish_agents(store, legacy, &mutation.agents, request, session),
    )
    .await?;
    published.extend(legacy);
    logs.extend(legacy_logs);
    published.sort_by(|left, right| left.id.cmp(&right.id));
    logs.sort_by(|left, right| (&left.agent_id, left.seq).cmp(&(&right.agent_id, right.seq)));
    let mut result = persist_sandbox(
        store, request, retirement, &current, last_seq, *mutation, published, logs, session,
    )
    .await?;
    if let RecoveryApplication::Persist(receipt) = &mut result {
        receipt.segmented = receipts;
    }
    Ok(result)
}

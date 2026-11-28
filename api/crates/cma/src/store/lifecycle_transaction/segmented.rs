//! Every touched segmented child uses the same publication transaction.
use super::*;
use crate::store::agent_log_segments::{
    self as segments, publication,
    writer_commit::{self, AggregateUpdate, PublicationInput, Recorded},
};

pub(super) async fn publish(
    store: &MongoSandboxStore,
    agents: &[Agent],
    mutation: &SandboxLifecycleMutation,
    scope: &mut TransactionScope<'_>,
) -> Result<BTreeMap<layer2_core::AgentId, Recorded>, ApplyError> {
    let mut receipts = BTreeMap::new();
    for change in &mutation.agents {
        let current = agents
            .iter()
            .find(|a| a.id == change.agent.id)
            .ok_or(ApplyError::Domain(SandboxStoreError::NotFound))?;
        let row = scope
            .find_one(&store.agents, doc! {"_id":current.id.as_str()})
            .await
            .map_err(ApplyError::Driver)?
            .ok_or(ApplyError::Domain(SandboxStoreError::NotFound))?;
        if segments::read_format(&row)
            .map_err(ApplyError::Domain)?
            .is_none()
        {
            continue;
        }
        let mut logs = materialize_logs(current, change, &mutation.appended_at)?;
        let mut next = change.agent.clone();
        crate::store::agent_conversation_store::checkpoint::finish_published_text(
            &row, current, &mut next, &mut logs,
        )
        .map_err(ApplyError::Domain)?;
        let operation = publication::PublicationIdentity {
            key: format!(
                "lifecycle:{}:{}",
                mutation.sandbox.revision, change.expected_revision
            ),
            fingerprint: publication::request_digest(&(
                &mutation.sandbox,
                &change.agent,
                &change.logs,
                &mutation.events,
                &mutation.appended_at,
            ))
            .map_err(ApplyError::Domain)?,
            appended_at: mutation.appended_at.clone(),
        };
        let mut fields = agent_replacement_fields(&next).map_err(ApplyError::Domain)?;
        crate::store::agent_conversation_store::carry_checkpoint_fields(
            &row,
            current,
            &next,
            &mut fields,
        )
        .map_err(ApplyError::Domain)?;
        let recorded = writer_commit::publish_existing(store, scope, PublicationInput {
            operation: &operation, current, next: &next, logs: &logs,
            capacity: AppendCapacity::StopOrFailure,
            result_digest: publication::result_digest(&next).map_err(ApplyError::Domain)?,
        }, AggregateUpdate { filter: doc! {
            "_id":current.id.as_str(),"sandbox_id":current.sandbox_id.as_str(),
            "owner":current.owner.as_str(),
            "revision":signed(change.expected_revision,"lifecycle revision").map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?,
        }, fields }).await?;
        if receipts.insert(current.id.clone(), recorded).is_some() {
            return Err(ApplyError::Domain(SandboxStoreError::Conflict));
        }
    }
    Ok(receipts)
}

//! The original worker CAS and domain result join the shared log publication.
use super::*;
use crate::store::agent_log_segments::{
    publication,
    writer_commit::{self, AggregateUpdate, PublicationInput, Recorded},
};

pub(super) async fn publish(
    store: &MongoSandboxStore,
    row: &Document,
    current: &Agent,
    mutation: &AgentWorkMutation,
    scope: &mut TransactionScope<'_>,
) -> Result<Recorded, ApplyError> {
    let (next, logs) = final_agent_and_logs(current, mutation)?;
    // Bind the original submitted mutation, never a freshly read head or lease.
    let fingerprint = publication::request_digest(&(
        &mutation.agent,
        &mutation.agent_logs,
        &mutation.sandbox_event,
        mutation.worktree.as_ref().map(|v| &v.worktree),
        mutation.expected_agent_revision,
        &mutation.holder,
        mutation.fence,
        &mutation.appended_at,
    ))
    .map_err(ApplyError::Domain)?;
    let operation = publication::PublicationIdentity {
        key: format!("worker:{}", mutation.expected_agent_revision),
        fingerprint,
        appended_at: mutation.appended_at.clone(),
    };
    let lease = current
        .work
        .lease
        .as_ref()
        .ok_or(ApplyError::Domain(SandboxStoreError::Conflict))?;
    let mut fields = agent_replacement_fields(&next).map_err(ApplyError::Domain)?;
    super::super::agent_conversation_store::carry_checkpoint_fields(
        row,
        current,
        &next,
        &mut fields,
    )
    .map_err(ApplyError::Domain)?;
    Ok(writer_commit::publish_existing(store, scope, PublicationInput {
        operation: &operation, current, next: &next, logs: &logs,
        capacity: publication_capacity(current, mutation),
        result_digest: publication::result_digest(&next).map_err(ApplyError::Domain)?,
    }, AggregateUpdate { filter: doc! {
        "_id":current.id.as_str(),
        "revision":signed(mutation.expected_agent_revision,"worker revision").map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?,
        "work_lease_holder":&mutation.holder,
        "work_lease_fence":signed(mutation.fence,"worker fence").map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?,
        "work_lease_expires_at":&lease.expires_at,
        "work_next_fence":signed(current.work.next_fence,"next fence").map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?,
    }, fields }).await?)
}

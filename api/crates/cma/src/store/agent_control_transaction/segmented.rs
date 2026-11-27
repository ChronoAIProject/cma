//! The original control receipt and lineage fences share segmented publication.
use super::*;
use crate::store::agent_log_integrity::{AppendCapacity, TransactionLogError};
use crate::store::agent_log_segments::{
    publication,
    transaction_scope::TransactionScope,
    writer_commit::{self, AggregateUpdate, PublicationInput, Recorded},
};

pub(super) async fn schedule(
    store: &MongoSandboxStore,
    operation: &ControlOperation,
) -> Result<AgentControlResult, AgentControlStoreError> {
    for attempt in 0..CONTROL_ATTEMPTS {
        crate::store::contention::backoff(attempt).await;
        let mut session = match store.client.start_session().await {
            Ok(session) => session,
            Err(error) if is_transient(&error) => continue,
            Err(error) => return Err(unavailable(error)),
        };
        let scope = match TransactionScope::start(&mut session).await {
            Ok(scope) => scope,
            Err(error) if is_transient(&error) => continue,
            Err(error) => return Err(unavailable(error)),
        };
        let (result, receipt) = match apply(store, operation, scope).await {
            Ok(value) => value,
            Err(ApplyError::Driver(error)) if is_transient(&error) => {
                let _ = session.abort_guarded().await;
                continue;
            }
            Err(error) => {
                let _ = session.abort_guarded().await;
                return Err(match error {
                    ApplyError::Domain(error) => error,
                    ApplyError::Driver(error) => unavailable(error),
                });
            }
        };
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => return Ok(result),
            CommitOutcome::Retry(_) => continue,
            CommitOutcome::Unknown(error) => {
                let Some(receipt) = receipt else { continue };
                let at = session.operation_time().ok_or_else(|| {
                    AgentControlStoreError::Unavailable(
                        "segmented control commit has no original operation time".into(),
                    )
                })?;
                let operation = operation.clone();
                let store = store.clone();
                return wait_for_reconciliation("segmented control commit", async move {
                    reconcile_control(&store, &operation, &receipt.next, Some(&receipt), at, error)
                        .await
                })
                .await
                .map_err(AgentControlStoreError::Unavailable)?;
            }
            CommitOutcome::Failed(error) => return Err(unavailable(error)),
        }
    }
    Err(AgentControlStoreError::Unavailable(
        "segmented control retry budget exhausted".into(),
    ))
}

async fn apply(
    store: &MongoSandboxStore,
    operation: &ControlOperation,
    mut scope: TransactionScope<'_>,
) -> Result<(AgentControlResult, Option<Recorded>), ApplyError> {
    let row = scope
        .find_one(
            &store.agents,
            doc! {"_id":operation.request.target().agent_id.as_str()},
        )
        .await
        .map_err(ApplyError::Driver)?
        .ok_or(ApplyError::Domain(AgentControlStoreError::NotFound))?;
    let current = crate::store::agents::read_agent(&row)
        .map_err(store_error)
        .map_err(ApplyError::Domain)?;
    let planned = operation
        .request
        .plan(current.clone())
        .map_err(ApplyError::Domain)?;
    let AgentControlResult::Scheduled(next) = planned else {
        // Even an Existing result must pass the original complete lineage read.
        let session = scope.release();
        load_control_snapshot(store, operation.request.target(), session, false).await?;
        return Ok((planned, None));
    };
    let appended_at = match &operation.request {
        ControlRequest::Stop(r) => &r.requested_at,
        ControlRequest::Delete(r) => &r.requested_at,
    };
    let identity = publication::PublicationIdentity {
        key: operation.id().into(),
        fingerprint: publication::request_digest(&operation.request)
            .map_err(store_error)
            .map_err(ApplyError::Domain)?,
        appended_at: appended_at.clone(),
    };
    let mut fields = agent_replacement_fields(&next)
        .map_err(store_error)
        .map_err(ApplyError::Domain)?;
    crate::store::agent_conversation_store::carry_checkpoint_fields(
        &row,
        &current,
        &next,
        &mut fields,
    )
    .map_err(store_error)
    .map_err(ApplyError::Domain)?;
    fields.extend(
        operation
            .receipt_update(&next)
            .map_err(ApplyError::Domain)?,
    );
    let recorded = writer_commit::publish_existing(
        store,
        &mut scope,
        PublicationInput {
            operation: &identity,
            current: &current,
            next: &next,
            logs: &[],
            capacity: AppendCapacity::StopOrFailure,
            result_digest: publication::result_digest(&next)
                .map_err(store_error)
                .map_err(ApplyError::Domain)?,
        },
        AggregateUpdate {
            filter: doc! {"_id":current.id.as_str(),operation.receipt_field():{"$exists":false}},
            fields,
        },
    )
    .await
    .map_err(|error| match error {
        TransactionLogError::Domain(error) => ApplyError::Domain(store_error(error)),
        TransactionLogError::Driver(error) => ApplyError::Driver(error),
    })?;
    let session = scope.release();
    let snapshot = load_control_snapshot(store, operation.request.target(), session, false).await?;
    if snapshot.agent != next
        || snapshot.sandbox.status == layer2_core::sandbox::SandboxStatus::Rotating
    {
        return Err(ApplyError::Domain(AgentControlStoreError::Conflict));
    }
    fence_control_lineage(store, &snapshot, operation.id(), session).await?;
    Ok((AgentControlResult::Scheduled(next), Some(recorded)))
}

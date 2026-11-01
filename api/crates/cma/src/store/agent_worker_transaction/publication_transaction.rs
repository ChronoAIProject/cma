//! Commit or abort one publication snapshot, including prepared eligibility.
use super::*;

pub(super) async fn commit(
    store: &MongoSandboxStore,
    mutation: AgentWorkMutation,
    observed: Option<&layer2_core::sandbox::store::PreparedAgentOpenContext>,
) -> Result<bool, SandboxStoreError> {
    ensure_append_batch_size(mutation.agent_logs.len())?;
    let mut timing = contention::PublicationTiming::new(&mutation);
    let _writer = store.agent_writes.lock(&mutation.agent.id).await;
    timing.acquired();
    let mut last_transient = None;
    let jitter = uuid::Uuid::new_v4().as_u128() as u64;
    for attempt in 0..TRANSACTION_ATTEMPTS {
        timing.attempt();
        contention::backoff(attempt, jitter).await;
        let mut session = match store.client.start_session().await {
            Ok(session) => session,
            Err(error) if is_transient(&error) => {
                last_transient = Some(error);
                continue;
            }
            Err(error) => return Err(SandboxStoreError::Unavailable(error.to_string())),
        };
        if let Err(error) = start_transaction(&mut session).await {
            if is_transient(&error) {
                last_transient = Some(error);
                continue;
            }
            return Err(SandboxStoreError::Unavailable(error.to_string()));
        }
        let publication = if observed.is_some() {
            apply_publication_observed(store, &mutation, observed, &mut session).await
        } else {
            apply_publication(store, &mutation, &mut session).await
        };
        let receipt = match publication {
            Ok(receipt) => receipt,
            Err(ApplyError::PreparedIneligible) => {
                let _ = session.abort_transaction().await;
                return Ok(false);
            }
            Err(ApplyError::Domain(error)) => {
                let _ = session.abort_transaction().await;
                return Err(error);
            }
            Err(ApplyError::Driver(error)) if is_transient(&error) => {
                let _ = session.abort_transaction().await;
                last_transient = Some(error);
                continue;
            }
            Err(ApplyError::Driver(error)) => {
                let _ = session.abort_transaction().await;
                return Err(SandboxStoreError::Unavailable(error.to_string()));
            }
        };
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => {
                timing.committed();
                return Ok(true);
            }
            CommitOutcome::Retry(error) => last_transient = Some(error),
            CommitOutcome::Unknown(error) => {
                let snapshot_time = session.operation_time().ok_or_else(|| {
                    SandboxStoreError::Unavailable(format!(
                        "agent work publication commit remained unknown without an operation time: {error}"
                    ))
                })?;
                let commit_error = error.to_string();
                let unresolved_error = commit_error.clone();
                let store = store.clone();
                return wait_for_reconciliation("agent work publication commit", async move {
                    if super::super::agent_worker_reconcile::reconcile_publication_after_unknown(
                        &store,
                        &receipt,
                        snapshot_time,
                    )
                    .await?
                    {
                        Ok(true)
                    } else {
                        Err(SandboxStoreError::Unavailable(format!(
                            "agent work publication commit remained unknown: {unresolved_error}"
                        )))
                    }
                })
                .await
                .map_err(|deadline| {
                    SandboxStoreError::Unavailable(format!(
                        "{deadline}; ambiguous commit: {commit_error}"
                    ))
                })?;
            }
            CommitOutcome::Failed(error) => {
                return Err(SandboxStoreError::Unavailable(error.to_string()));
            }
        }
    }
    Err(contention::exhausted(&mutation, last_transient.as_ref()))
}

use super::*;
use crate::store::transaction_terminal::GuardedTerminal as _;

#[async_trait::async_trait]
impl SandboxTransactionStore for MongoSandboxStore {
    async fn commit(&self, transaction: SandboxTransaction) -> Result<(), SandboxStoreError> {
        let wake = matches!(&transaction,
            SandboxTransaction::SaveSandboxAndAppend { sandbox, event_at, .. }
            if sandbox.work.lease.is_none() && sandbox.work.attempts == 0
                && sandbox.work.is_due(event_at)
                && matches!(sandbox.work.step,
                    layer2_core::sandbox::SandboxWorkStep::RunLifecycleOperation
                    | layer2_core::sandbox::SandboxWorkStep::StopAgents))
            || matches!(
                &transaction,
                SandboxTransaction::SaveWorkspaceAndAppend {
                    repository_runtime: Some(_),
                    ..
                }
            );
        let parent_ready = match &transaction {
            SandboxTransaction::SaveSandboxAndAppend { sandbox, event, .. } => {
                sandbox.status == layer2_core::sandbox::SandboxStatus::Active
                    && sandbox.credential.delivered
                    && matches!(event, SandboxEventKind::StatusChanged { from, to }
                        if *from != layer2_core::sandbox::SandboxStatus::Active
                            && *to == layer2_core::sandbox::SandboxStatus::Active)
            }
            SandboxTransaction::SaveWorkspaceAndAppend {
                workspace, event, ..
            } => {
                workspace.status == WorkspaceStatus::Ready
                    && matches!(event, SandboxEventKind::WorkspaceReady { .. })
            }
            _ => false,
        };
        self.commit_aggregate_transaction(transaction).await?;
        // The successful return includes positively reconciled ambiguous commits.
        // A hint grants no authority; discovery and claims still use durable state.
        if wake {
            self.sandbox_admitted.notify_one();
        }
        if parent_ready {
            self.agent_admitted.notify_one();
            self.agent_parent_ready.notify_waiters();
            self.access_admitted.notify_one();
        }
        Ok(())
    }
}

impl MongoSandboxStore {
    async fn commit_aggregate_transaction(
        &self,
        transaction: SandboxTransaction,
    ) -> Result<(), SandboxStoreError> {
        validate_transaction(&transaction)?;
        let mut last_transient = None;
        for attempt in 0..TRANSACTION_ATTEMPTS {
            if attempt > 0 {
                tokio::task::yield_now().await;
            }
            let mut session = match self.client.start_session().await {
                Ok(session) => session,
                Err(error) if is_transient(&error) => {
                    last_transient = Some(error.to_string());
                    continue;
                }
                Err(error) => return Err(SandboxStoreError::Unavailable(error.to_string())),
            };
            if let Err(error) = start_transaction(&mut session).await {
                if is_transient(&error) {
                    last_transient = Some(error.to_string());
                    continue;
                }
                return Err(SandboxStoreError::Unavailable(error.to_string()));
            }
            let receipt = match apply_transaction(self, &mut session, &transaction).await {
                Ok(receipt) => receipt,
                Err(ApplyError::Domain(error)) => {
                    let _ = session.abort_guarded().await;
                    return Err(error);
                }
                Err(ApplyError::Driver(error)) if is_transient(&error) => {
                    let _ = session.abort_guarded().await;
                    last_transient = Some(error.to_string());
                    continue;
                }
                Err(ApplyError::Driver(error)) => {
                    let _ = session.abort_guarded().await;
                    return Err(SandboxStoreError::Unavailable(error.to_string()));
                }
            };
            if let Err(error) = command_revival::require_current(&transaction) {
                let _ = session.abort_guarded().await;
                return Err(error);
            }
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(()),
                CommitOutcome::Retry(error) => last_transient = Some(error.to_string()),
                CommitOutcome::Unknown(error) => {
                    let snapshot_time = session.operation_time().ok_or_else(|| {
                        SandboxStoreError::Unavailable(format!(
                            "sandbox transaction commit remained unknown without an operation time: {error}"
                        ))
                    })?;
                    let commit_error = error.to_string();
                    let unresolved = commit_error.clone();
                    let store = self.clone();
                    return wait_for_reconciliation("sandbox transaction", async move {
                        if reconcile(&store, &transaction, &receipt, snapshot_time).await? {
                            Ok(())
                        } else {
                            Err(SandboxStoreError::Unavailable(format!(
                                "sandbox transaction commit remained unknown: {unresolved}"
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
        Err(SandboxStoreError::Unavailable(format!(
            "sandbox transaction exhausted its retry budget: {}",
            last_transient.unwrap_or_else(|| "no driver result".into())
        )))
    }
}

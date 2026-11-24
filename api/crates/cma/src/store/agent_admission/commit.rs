//! Catalogue and snapshot admissions share the current transaction and replay protocol.
use super::*;
use crate::store::transaction_terminal::GuardedTerminal as _;
use mongodb::error::UNKNOWN_TRANSACTION_COMMIT_RESULT;

impl MongoSandboxStore {
    pub(super) fn admit_agent_with_reuse(
        &self,
        admission: AgentAdmission,
        reuse_existing: bool,
        catalogue: bool,
        initiation: Option<layer2_core::trigger_domain::LaunchInitiation>,
    ) -> impl std::future::Future<Output = Result<AgentAdmissionResult, SandboxStoreError>> + '_
    {
        Box::pin(self.admit_agent_with_reuse_inner(
            admission,
            reuse_existing,
            catalogue,
            initiation,
        ))
    }

    async fn admit_agent_with_reuse_inner(
        &self,
        admission: AgentAdmission,
        reuse_existing: bool,
        catalogue: bool,
        initiation: Option<layer2_core::trigger_domain::LaunchInitiation>,
    ) -> Result<AgentAdmissionResult, SandboxStoreError> {
        let result = self
            .commit_agent_admission(admission, reuse_existing, catalogue, initiation)
            .await?;
        // No work or authority travels in this hint; the scheduler discovers and claims.
        self.agent_admitted.notify_one();
        Ok(result)
    }

    fn commit_agent_admission(
        &self,
        admission: AgentAdmission,
        reuse_existing: bool,
        catalogue: bool,
        initiation: Option<layer2_core::trigger_domain::LaunchInitiation>,
    ) -> impl std::future::Future<Output = Result<AgentAdmissionResult, SandboxStoreError>> + '_
    {
        Box::pin(self.commit_agent_admission_inner(
            admission,
            reuse_existing,
            catalogue,
            initiation,
        ))
    }

    async fn commit_agent_admission_inner(
        &self,
        admission: AgentAdmission,
        reuse_existing: bool,
        catalogue: bool,
        initiation: Option<layer2_core::trigger_domain::LaunchInitiation>,
    ) -> Result<AgentAdmissionResult, SandboxStoreError> {
        let mut last_transient = None;
        let mut retries = retry::AdmissionRetries::new();
        while retries.next().await {
            let mut session = match self.client.start_session().await {
                Ok(session) => session,
                Err(error) if is_transient(&error) => {
                    last_transient = Some(error.to_string());
                    continue;
                }
                Err(error) => return Err(SandboxStoreError::Unavailable(error.to_string())),
            };
            let mut scope = match TransactionScope::start(&mut session).await {
                Ok(scope) => scope,
                Err(error) => {
                    if is_transient(&error) {
                        last_transient = Some(error.to_string());
                        continue;
                    }
                    return Err(SandboxStoreError::Unavailable(error.to_string()));
                }
            };
            let applied = Box::pin(apply_catalogue(
                self,
                &admission,
                reuse_existing,
                catalogue,
                initiation.as_ref(),
                &mut scope,
            ))
            .await;
            let session = scope.release();
            let receipt = match applied {
                Ok(AdmissionReceipt::Existing(creation)) => {
                    match commit_transaction(session).await {
                        CommitOutcome::Committed => {
                            return Ok(AgentAdmissionResult::Existing(*creation));
                        }
                        CommitOutcome::Retry(error) => {
                            last_transient = Some(error.to_string());
                            continue;
                        }
                        CommitOutcome::Unknown(_) => {
                            // Never re-run an admission whose commit may have applied.
                            return Box::pin(
                                super::super::agent_admission_records::reconcile_existing(
                                    self,
                                    &admission,
                                    *creation,
                                    session.operation_time(),
                                ),
                            )
                            .await;
                        }
                        CommitOutcome::Failed(error) => {
                            return Err(SandboxStoreError::Unavailable(error.to_string()));
                        }
                    }
                }
                Ok(AdmissionReceipt::Reused(receipt)) => match commit_transaction(session).await {
                    CommitOutcome::Committed => {
                        return Ok(AgentAdmissionResult::Existing(receipt.creation))
                    }
                    CommitOutcome::Retry(error) => {
                        last_transient = Some(error.to_string());
                        continue;
                    }
                    CommitOutcome::Unknown(_) => {
                        return Box::pin(reuse_reconciliation::after_unknown(
                            self,
                            &admission,
                            receipt,
                            session.operation_time(),
                        ))
                        .await;
                    }
                    CommitOutcome::Failed(error) => {
                        return Err(SandboxStoreError::Unavailable(error.to_string()))
                    }
                },
                Ok(AdmissionReceipt::Created(receipt)) => receipt,
                Err(ApplyError::Domain(error)) => {
                    let _ = session.abort_guarded().await;
                    return Err(error);
                }
                Err(ApplyError::AgentDuplicate) => {
                    let replay_time = session.operation_time();
                    let _ = session.abort_guarded().await;
                    let replay_time = replay_time.ok_or_else(|| {
                        SandboxStoreError::Unavailable(
                            "agent admission duplicate replay has no Mongo operation time".into(),
                        )
                    })?;
                    let store = self.clone();
                    return wait_for_reconciliation(
                        "agent admission duplicate replay",
                        async move {
                            Box::pin(replay_after_duplicate(&store, &admission, replay_time))
                                .await?
                                .ok_or(SandboxStoreError::Duplicate)
                        },
                    )
                    .await
                    .map_err(SandboxStoreError::Unavailable)?;
                }
                Err(ApplyError::Driver(error))
                    if error.contains_label(UNKNOWN_TRANSACTION_COMMIT_RESULT) =>
                {
                    let _ = session.abort_guarded().await;
                    return Err(SandboxStoreError::Unavailable(
                        "Agent admission transaction outcome is unavailable".into(),
                    ));
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
            match commit_transaction(session).await {
                CommitOutcome::Committed => {
                    return Ok(AgentAdmissionResult::Created(receipt.creation));
                }
                CommitOutcome::Retry(error) => last_transient = Some(error.to_string()),
                CommitOutcome::Unknown(error) => {
                    let reconciliation_time = session.operation_time().ok_or_else(|| {
                        SandboxStoreError::Unavailable(format!(
                            "agent admission commit remained unknown with no Mongo operation time: {error}"
                        ))
                    })?;
                    let commit_error = error.to_string();
                    let unresolved_error = commit_error.clone();
                    let store = self.clone();
                    return wait_for_reconciliation("agent admission commit", async move {
                        if Box::pin(reconcile_created(&store, &receipt, reconciliation_time))
                            .await?
                        {
                            Ok(AgentAdmissionResult::Created(receipt.creation))
                        } else {
                            Err(SandboxStoreError::Unavailable(format!(
                                "agent admission commit remained unknown: {unresolved_error}"
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
            "agent admission transaction exhausted its retry budget: {}",
            last_transient.unwrap_or_else(|| "no driver result".into())
        )))
    }
}

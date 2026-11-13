//! Retry catalogue and snapshot Agent admissions without changing durable replay.
use super::*;

impl MongoSandboxStore {
    pub(super) async fn admit_agent_with_reuse(
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

    async fn commit_agent_admission(
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
            if let Err(error) = start_transaction(&mut session).await {
                if is_transient(&error) {
                    last_transient = Some(error.to_string());
                    continue;
                }
                return Err(SandboxStoreError::Unavailable(error.to_string()));
            }
            let receipt = match apply_catalogue_admission(
                self,
                &admission,
                reuse_existing,
                catalogue,
                initiation.as_ref(),
                &mut session,
            )
            .await
            {
                Ok(AdmissionReceipt::Existing(creation)) => {
                    match commit_transaction(&mut session).await {
                        CommitOutcome::Committed => {
                            return Ok(AgentAdmissionResult::Existing(*creation));
                        }
                        CommitOutcome::Retry(error) | CommitOutcome::Unknown(error) => {
                            last_transient = Some(error.to_string());
                            continue;
                        }
                        CommitOutcome::Failed(error) => {
                            return Err(SandboxStoreError::Unavailable(error.to_string()));
                        }
                    }
                }
                Ok(AdmissionReceipt::Created(receipt)) => receipt,
                Err(ApplyError::Domain(error)) => {
                    let _ = session.abort_transaction().await;
                    return Err(error);
                }
                Err(ApplyError::AgentDuplicate) => {
                    let replay_time = session.operation_time();
                    let _ = session.abort_transaction().await;
                    let replay_time = replay_time.ok_or_else(|| {
                        SandboxStoreError::Unavailable(
                            "agent admission duplicate replay has no Mongo operation time".into(),
                        )
                    })?;
                    let store = self.clone();
                    return wait_for_reconciliation(
                        "agent admission duplicate replay",
                        async move {
                            replay_after_duplicate(&store, &admission, replay_time)
                                .await?
                                .ok_or(SandboxStoreError::Duplicate)
                        },
                    )
                    .await
                    .map_err(SandboxStoreError::Unavailable)?;
                }
                Err(ApplyError::Driver(error)) if is_transient(&error) => {
                    let _ = session.abort_transaction().await;
                    last_transient = Some(error.to_string());
                    continue;
                }
                Err(ApplyError::Driver(error)) => {
                    let _ = session.abort_transaction().await;
                    return Err(SandboxStoreError::Unavailable(error.to_string()));
                }
            };
            match commit_transaction(&mut session).await {
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
                        if reconcile_created(&store, &receipt, reconciliation_time).await? {
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

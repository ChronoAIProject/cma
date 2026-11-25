//! Retry catalogue and snapshot workspace admissions with the same transaction.
use super::*;

impl MongoSandboxStore {
    pub(super) async fn commit_workspace_admission(
        &self,
        admission: WorkspaceAdmission,
        catalogue: bool,
    ) -> Result<WorkspaceAdmissionResult, SandboxStoreError> {
        validate_fresh(&admission.workspace, admission.kind)?;
        let mut last_transient = None;
        for attempt in 0..TRANSACTION_ATTEMPTS {
            retry_delay(attempt).await;
            let mut session = self
                .client
                .start_session()
                .await
                .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
            if let Err(error) = start_transaction(&mut session).await {
                if is_transient(&error) {
                    last_transient = Some(error.to_string());
                    continue;
                }
                return Err(SandboxStoreError::Unavailable(error.to_string()));
            }
            let receipt = match apply_admission(self, &admission, catalogue, &mut session).await {
                Ok(receipt) => receipt,
                Err(ApplyError::Domain(error)) => {
                    let _ = session.abort_guarded().await;
                    return Err(error);
                }
                Err(ApplyError::Retry(error)) | Err(ApplyError::Driver(error))
                    if is_transient(&error) =>
                {
                    let _ = session.abort_guarded().await;
                    last_transient = Some(error.to_string());
                    continue;
                }
                Err(ApplyError::Retry(error)) => {
                    let _ = session.abort_guarded().await;
                    last_transient = Some(error.to_string());
                    continue;
                }
                Err(ApplyError::Driver(error)) => {
                    let _ = session.abort_guarded().await;
                    return Err(SandboxStoreError::Unavailable(error.to_string()));
                }
            };
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(receipt.result()),
                CommitOutcome::Retry(error) => last_transient = Some(error.to_string()),
                CommitOutcome::Unknown(error) => {
                    return if reconcile_admission(self, &receipt).await? {
                        Ok(receipt.result())
                    } else {
                        Err(SandboxStoreError::Unavailable(format!(
                            "workspace admission commit remained unknown: {error}"
                        )))
                    };
                }
                CommitOutcome::Failed(error) => {
                    return Err(SandboxStoreError::Unavailable(error.to_string()));
                }
            }
        }
        Err(exhausted("workspace admission", last_transient))
    }
}

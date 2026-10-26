//! Prepared health probes may briefly own capacity; no transaction waits for them.
use std::time::Duration;

use super::*;
use crate::store::sandbox_mongo::{
    commit_transaction, is_transient, start_transaction, CommitOutcome,
};

const TRANSACTION_ATTEMPTS: usize = 16;
const PREPARED_WAIT: Duration = Duration::from_millis(2_500);
const PREPARED_POLL: Duration = Duration::from_millis(50);

impl MongoSandboxStore {
    pub(super) async fn commit_sandbox_admission(
        &self,
        admission: SandboxAdmission,
    ) -> Result<SandboxAdmissionResult, SandboxStoreError> {
        self.commit_sandbox_admission_observing(admission, layer2_core::now_rfc3339)
            .await
    }

    // The explicit observation clock keeps the immutable admission event separate
    // from live lease/health decisions, including deterministic persistence tests.
    pub(in crate::store) async fn commit_sandbox_admission_observing<F>(
        &self,
        admission: SandboxAdmission,
        observe: F,
    ) -> Result<SandboxAdmissionResult, SandboxStoreError>
    where
        F: Fn() -> String + Send + Sync,
    {
        admission.validate()?;
        let started = tokio::time::Instant::now();
        let mut last_transient = None;
        let mut failures = 0;
        let mut prepared_lease_waits = 0_u32;
        let mut delay = Duration::ZERO;
        loop {
            if failures >= TRANSACTION_ATTEMPTS {
                return Err(SandboxStoreError::Unavailable(format!(
                    "sandbox admission exhausted its retry budget: {}",
                    last_transient.unwrap_or_else(|| "no driver result".into()),
                )));
            }
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            let mut session = self
                .client
                .start_session()
                .await
                .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
            if let Err(error) = start_transaction(&mut session).await {
                if is_transient(&error) {
                    last_transient = Some(error.to_string());
                    failures += 1;
                    delay = admission_retry_delay(failures);
                    continue;
                }
                return Err(SandboxStoreError::Unavailable(error.to_string()));
            }
            let wait_for_lease = started.elapsed() < PREPARED_WAIT;
            let receipt =
                match apply(self, &admission, &mut session, &observe, wait_for_lease).await {
                    Ok(receipt) => receipt,
                    Err(AdmissionError::PreparedLeaseBusy) => {
                        let _ = session.abort_transaction().await;
                        prepared_lease_waits += 1;
                        delay = PREPARED_POLL.min(PREPARED_WAIT.saturating_sub(started.elapsed()));
                        continue;
                    }
                    Err(AdmissionError::Domain(error)) => {
                        let _ = session.abort_transaction().await;
                        return Err(error);
                    }
                    Err(AdmissionError::Retry(error)) | Err(AdmissionError::Driver(error))
                        if is_transient(&error) =>
                    {
                        let _ = session.abort_transaction().await;
                        last_transient = Some(error.to_string());
                        failures += 1;
                        delay = admission_retry_delay(failures);
                        continue;
                    }
                    Err(AdmissionError::Retry(error)) => {
                        let _ = session.abort_transaction().await;
                        last_transient = Some(error.to_string());
                        failures += 1;
                        delay = admission_retry_delay(failures);
                        continue;
                    }
                    Err(AdmissionError::Duplicate) => {
                        let _ = session.abort_transaction().await;
                        return match durable_replay(self, &admission).await? {
                            Some(sandbox) => Ok(SandboxAdmissionResult::Existing(sandbox)),
                            None => Err(SandboxStoreError::Duplicate),
                        };
                    }
                    Err(AdmissionError::Driver(error)) => {
                        let _ = session.abort_transaction().await;
                        return Err(SandboxStoreError::Unavailable(error.to_string()));
                    }
                };
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => {
                    let outcome = match &receipt {
                        AdmissionReceipt::Created {
                            prepared_outcome, ..
                        } => *prepared_outcome,
                        AdmissionReceipt::Existing(_) => "replay",
                    };
                    let sandbox_id = match &receipt {
                        AdmissionReceipt::Created { sandbox, .. }
                        | AdmissionReceipt::Existing(sandbox) => &sandbox.id,
                    };
                    tracing::info!(outcome, sandbox_id = %sandbox_id, prepared_lease_waits,
                        admission_elapsed_ms = started.elapsed().as_millis(),
                        "native prepared admission completed");
                    self.warmup_changed.notify_one();
                    return Ok(receipt.into_result());
                }
                CommitOutcome::Retry(error) => {
                    last_transient = Some(error.to_string());
                    failures += 1;
                    delay = admission_retry_delay(failures);
                }
                CommitOutcome::Unknown(error) => {
                    return reconcile(self, &admission, &receipt).await?.ok_or_else(|| {
                        SandboxStoreError::Unavailable(format!(
                            "sandbox admission commit remained unknown: {error}",
                        ))
                    });
                }
                CommitOutcome::Failed(error) => {
                    return Err(SandboxStoreError::Unavailable(error.to_string()));
                }
            }
        }
    }
}

//! Prepared health probes may briefly own capacity; no transaction waits for them.
use crate::store::sandbox_events::read_event;
use crate::store::sandboxes::read_sandbox;
use crate::store::transaction_terminal::GuardedTerminal as _;
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
        self.commit_sandbox_admission_inner(admission, observe, false)
            .await
    }

    pub(super) async fn commit_catalogue_sandbox_admission(
        &self,
        admission: SandboxAdmission,
    ) -> Result<SandboxAdmissionResult, SandboxStoreError> {
        self.commit_sandbox_admission_inner(admission, layer2_core::now_rfc3339, true)
            .await
    }

    async fn commit_sandbox_admission_inner<F>(
        &self,
        admission: SandboxAdmission,
        observe: F,
        catalogue: bool,
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
            let receipt = match apply(
                self,
                &admission,
                &mut session,
                &observe,
                wait_for_lease,
                failures > 0,
                catalogue,
            )
            .await
            {
                Ok(receipt) => receipt,
                Err(AdmissionError::PreparedLeaseBusy) => {
                    let _ = session.abort_guarded().await;
                    prepared_lease_waits += 1;
                    delay = PREPARED_POLL.min(PREPARED_WAIT.saturating_sub(started.elapsed()));
                    continue;
                }
                Err(AdmissionError::Domain(error)) => {
                    let _ = session.abort_guarded().await;
                    return Err(error);
                }
                Err(AdmissionError::Retry(error)) | Err(AdmissionError::Driver(error))
                    if is_transient(&error) =>
                {
                    let _ = session.abort_guarded().await;
                    last_transient = Some(error.to_string());
                    failures += 1;
                    delay = admission_retry_delay(failures);
                    continue;
                }
                Err(AdmissionError::Retry(error)) => {
                    let _ = session.abort_guarded().await;
                    last_transient = Some(error.to_string());
                    failures += 1;
                    delay = admission_retry_delay(failures);
                    continue;
                }
                Err(AdmissionError::Duplicate) => {
                    let _ = session.abort_guarded().await;
                    return match durable_replay(self, &admission).await? {
                        Some(sandbox) => Ok(SandboxAdmissionResult::Existing(sandbox)),
                        None => Err(SandboxStoreError::Duplicate),
                    };
                }
                Err(AdmissionError::Driver(error)) => {
                    let _ = session.abort_guarded().await;
                    return Err(SandboxStoreError::Unavailable(error.to_string()));
                }
            };
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => {
                    self.observe_prepared_admission(
                        &receipt,
                        started.elapsed(),
                        prepared_lease_waits,
                    );
                    return Ok(receipt.into_result());
                }
                CommitOutcome::Retry(error) => {
                    last_transient = Some(error.to_string());
                    failures += 1;
                    delay = admission_retry_delay(failures);
                }
                CommitOutcome::Unknown(error) => {
                    let result = reconcile(self, &admission, &receipt)
                        .await?
                        .ok_or_else(|| {
                            SandboxStoreError::Unavailable(format!(
                                "sandbox admission commit remained unknown: {error}",
                            ))
                        })?;
                    // Reconciliation can prove a different concurrent winner.
                    // Only our positively reconciled creation consumed this receipt.
                    match &result {
                        SandboxAdmissionResult::Created(_) => self.observe_prepared_admission(
                            &receipt,
                            started.elapsed(),
                            prepared_lease_waits,
                        ),
                        SandboxAdmissionResult::Existing(sandbox) => self
                            .observe_prepared_admission(
                                &AdmissionReceipt::Existing(sandbox.clone()),
                                started.elapsed(),
                                prepared_lease_waits,
                            ),
                    }
                    return Ok(result);
                }
                CommitOutcome::Failed(error) => {
                    return Err(SandboxStoreError::Unavailable(error.to_string()));
                }
            }
        }
    }

    fn observe_prepared_admission(
        &self,
        receipt: &AdmissionReceipt,
        elapsed: Duration,
        prepared_lease_waits: u32,
    ) {
        let (sandbox, outcome) = match receipt {
            AdmissionReceipt::Created {
                sandbox,
                prepared_outcome,
                ..
            } => (sandbox, *prepared_outcome),
            AdmissionReceipt::Existing(sandbox) => (sandbox, "replay"),
        };
        let class = if sandbox
            .profile
            .content
            .environment_variables
            .iter()
            .any(|entry| entry.name == "CMA_MACHINE_INCLUDED" && entry.value == "1")
        {
            "machine"
        } else {
            "plain"
        };
        let origin = match sandbox.origin {
            SandboxOrigin::Lifecycle { .. } => "lifecycle",
            SandboxOrigin::Trigger { .. } => "trigger",
            SandboxOrigin::Manual => "manual",
        };
        metrics::counter!("cma_native_prepared_admissions_total", "outcome" => outcome, "class" => class, "origin" => origin).increment(1);
        metrics::histogram!("cma_native_prepared_admission_seconds", "outcome" => outcome, "class" => class, "origin" => origin).record(elapsed.as_secs_f64());
        tracing::info!(outcome, class, origin, sandbox_id = %sandbox.id, prepared_lease_waits,
            admission_elapsed_ms = elapsed.as_millis(), "native prepared admission completed");
        if outcome == "hit" {
            // The native worker has its own permit: legacy cleanup must not
            // consume the refill hint. No authority or target is carried here.
            self.native_prepared_changed.notify_one();
        }
        self.warmup_changed.notify_one();
    }
}

async fn reconcile(
    store: &MongoSandboxStore,
    admission: &SandboxAdmission,
    receipt: &AdmissionReceipt,
) -> Result<Option<SandboxAdmissionResult>, SandboxStoreError> {
    if let Some(current) = durable_replay(store, admission).await? {
        return match receipt {
            AdmissionReceipt::Created { sandbox, event, .. } if current.id == sandbox.id => {
                let stored_event = store
                    .sandbox_events
                    .find_one(doc! { "_id": format!("{}:{:020}", event.sandbox_id, 1) })
                    .await
                    .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
                    .as_ref()
                    .map(read_event)
                    .transpose()?;
                Ok((stored_event.as_ref() == Some(event))
                    .then_some(SandboxAdmissionResult::Created(current)))
            }
            AdmissionReceipt::Created { .. } | AdmissionReceipt::Existing(_) => {
                Ok(Some(SandboxAdmissionResult::Existing(current)))
            }
        };
    }
    let AdmissionReceipt::Created { sandbox, event, .. } = receipt else {
        return Ok(None);
    };
    let stored_sandbox = store
        .sandboxes
        .find_one(doc! { "_id": sandbox.id.as_str() })
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
        .as_ref()
        .map(read_sandbox)
        .transpose()?;
    let stored_event = store
        .sandbox_events
        .find_one(doc! { "_id": format!("{}:{:020}", event.sandbox_id, 1) })
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
        .as_ref()
        .map(read_event)
        .transpose()?;
    Ok(
        (stored_sandbox.as_ref() == Some(sandbox) && stored_event.as_ref() == Some(event))
            .then(|| SandboxAdmissionResult::Created(sandbox.clone())),
    )
}

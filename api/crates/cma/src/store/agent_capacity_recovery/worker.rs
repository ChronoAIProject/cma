//! One accepted claim, one restricted attachment and one completion attempt.
use std::time::{Duration, SystemTime};

use layer2_core::{
    sandbox::agent_capacity_recovery::{
        activation::RecoveryActivation,
        ports::{
            CapacityRecoveryRuntime, ClaimOutcome, CommitOutcome, CompletedAttachment,
            RecoveryError,
        },
        state::{
            RecoveryClaim, RecoveryConfirmation, RecoveryPhase, RecoveryRequest, RecoveryWork,
        },
    },
    Caller,
};

use super::service::RecoveryService;

/// Each operation obtains fresh current authority. The worker cannot turn a
/// discovery candidate or a prior authorization into permission to activate.
#[async_trait::async_trait]
pub(super) trait AcceptedWork: Send + Sync {
    async fn claim(
        &self,
        request: &RecoveryRequest,
        holder: &str,
        now: &str,
        expires: &str,
    ) -> Result<ClaimOutcome, RecoveryError>;
    async fn complete(
        &self,
        claim: &RecoveryClaim,
        confirmation: &RecoveryConfirmation,
        now: &str,
    ) -> Result<CommitOutcome<RecoveryWork>, RecoveryError>;
    async fn retry(
        &self,
        claim: &RecoveryClaim,
        now: &str,
        retry_at: &str,
    ) -> Result<CommitOutcome<RecoveryWork>, RecoveryError>;
    async fn reconcile(
        &self,
        expected: &RecoveryWork,
    ) -> Result<CommitOutcome<RecoveryWork>, RecoveryError>;
    async fn activation(
        &self,
        _completed: &RecoveryWork,
    ) -> Result<RecoveryActivation, RecoveryError> {
        Err(RecoveryError::Unavailable)
    }
    async fn require_activation(&self, completed: &RecoveryWork) -> Result<(), RecoveryError>;
}

impl RecoveryService {
    pub(crate) async fn run_one<R: CapacityRecoveryRuntime>(
        &self,
        runtime: &R,
        caller: &Caller,
        request: &RecoveryRequest,
        holder: &str,
    ) -> Result<CommitOutcome<RecoveryWork>, RecoveryError> {
        run(
            &super::worker_store::AuthorizedWork {
                service: self,
                caller,
            },
            runtime,
            request,
            holder,
            SystemTime::now(),
        )
        .await
    }
}

pub(super) async fn run<S: AcceptedWork, R: CapacityRecoveryRuntime>(
    store: &S,
    runtime: &R,
    request: &RecoveryRequest,
    holder: &str,
    at: SystemTime,
) -> Result<CommitOutcome<RecoveryWork>, RecoveryError> {
    request.validate()?;
    let started = tokio::time::Instant::now();
    let now = layer2_core::rfc3339_at(at);
    let expires = layer2_core::rfc3339_at(at + Duration::from_secs(60));
    let claim = match store.claim(request, holder, &now, &expires).await? {
        ClaimOutcome::Claimed(claim) => claim,
        ClaimOutcome::Waiting(work) | ClaimOutcome::Terminal(work) => {
            work.validate()?;
            if work.request != *request {
                return Err(RecoveryError::Conflict);
            }
            if work.phase == RecoveryPhase::Completed {
                let activation = store.activation(&work).await?;
                let attached = tokio::time::timeout(
                    Duration::from_secs(15),
                    runtime.reattach_completed(&activation),
                )
                .await
                .map_err(|_| RecoveryError::Unavailable)??;
                if store.activation(&work).await? != activation {
                    return Err(RecoveryError::Conflict);
                }
                if let CompletedAttachment::Reattached {
                    attachment,
                    confirmation,
                } = attached
                {
                    activation.require_confirmation(&confirmation)?;
                    runtime.acknowledge(attachment, &work).await?;
                }
            }
            return Ok(CommitOutcome::Recorded(*work));
        }
        ClaimOutcome::Unknown => return Ok(CommitOutcome::Unknown),
    };
    claim.work.validate()?;
    if claim.work.request != *request || claim.agent.id != request.agent_id {
        return Err(RecoveryError::Conflict);
    }
    // This timeout is shorter than the claim, including bounded store work.
    // A timeout drops the attachment without input or acknowledgement.
    let attached =
        tokio::time::timeout(Duration::from_secs(15), runtime.reattach_existing(&claim)).await;
    let (attachment, confirmation) = match attached {
        Ok(Ok(attached)) => attached,
        Ok(Err(RecoveryError::Unavailable)) | Err(_) => {
            let now = at + started.elapsed();
            return store
                .retry(
                    &claim,
                    &layer2_core::rfc3339_at(now),
                    &layer2_core::rfc3339_at(now + Duration::from_secs(30)),
                )
                .await;
        }
        Ok(Err(error)) => return Err(error),
    };
    confirmation.validate_for(&claim)?;
    let now = layer2_core::rfc3339_at(at + started.elapsed());
    let mut expected = claim.work.clone();
    expected.phase = RecoveryPhase::Completed;
    expected.revision = expected
        .revision
        .checked_add(1)
        .ok_or(RecoveryError::InvalidEvidence)?;
    expected.lease = None;
    expected.completed_at = Some(now.clone());
    let outcome = store.complete(&claim, &confirmation, &now).await?;
    let outcome = if matches!(outcome, CommitOutcome::Unknown) {
        // Only an exact immutable receipt can settle this attempted result.
        // The completion mutation itself is never repeated here.
        store.reconcile(&expected).await?
    } else {
        outcome
    };
    if let CommitOutcome::Recorded(completed) = &outcome {
        if completed != &expected {
            return Err(RecoveryError::InvalidEvidence);
        }
        store.require_activation(completed).await?;
        runtime.acknowledge(attachment, completed).await?;
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests;

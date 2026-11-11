//! Default-off orchestration over the frozen storage boundary and restricted runtime.
use super::ports::*;
use super::state::{RecoveryPhase, RecoveryRequest, RecoveryWork};
use crate::Caller;
use std::time::{Duration, SystemTime};

/// This process feature is independent of the two durable admission gates.
/// Turning it off blocks new admission but retains accepted-work settlement.
#[derive(Debug, Clone, Copy, Default)]
pub struct CapacityRecoveryFeatures {
    pub enabled: bool,
}

pub struct CapacityRecoveryService<S, R> {
    store: S,
    runtime: R,
    features: CapacityRecoveryFeatures,
}

impl<S: CapacityRecoveryStore, R: CapacityRecoveryRuntime> CapacityRecoveryService<S, R> {
    pub fn new(store: S, runtime: R) -> Self {
        Self {
            store,
            runtime,
            features: CapacityRecoveryFeatures::default(),
        }
    }

    pub fn with_features(mut self, features: CapacityRecoveryFeatures) -> Self {
        self.features = features;
        self
    }

    pub async fn admit(
        &self,
        caller: &Caller,
        request: &RecoveryRequest,
    ) -> Result<CommitOutcome<RecoveryWork>, RecoveryError> {
        request.validate()?;
        match self.store.lookup(caller, request).await? {
            RecoveryLookup::Recorded(work) => {
                work.validate()?;
                if &work.request != request {
                    return Err(RecoveryError::Conflict);
                }
                return Ok(CommitOutcome::Recorded(*work));
            }
            RecoveryLookup::Unknown => return Ok(CommitOutcome::Unknown),
            RecoveryLookup::NotStarted => {}
        }
        if !self.features.enabled {
            return Err(RecoveryError::Disabled);
        }
        let snapshot = self.store.capture(caller, request).await?;
        let plan = snapshot
            .context()
            .plan(&snapshot.original_failure, &request.requested_at)
            .map_err(RecoveryError::Disposition)?;
        let work = RecoveryWork::prepare(request.clone(), &plan, &snapshot.context())?;
        let next_agent = plan
            .fenced_agent(&snapshot.context(), &request.requested_at)
            .map_err(RecoveryError::Disposition)?;
        self.store.admit(caller, snapshot, work, next_agent).await
    }

    async fn activate_completed(
        &self,
        caller: &Caller,
        completed: &RecoveryWork,
    ) -> Result<(), RecoveryError> {
        let activation = self.store.activation(caller, completed).await?;
        let attached = self.runtime.reattach_completed(&activation).await?;
        // Re-read authority, parents and the immutable result after native IO.
        if self.store.activation(caller, completed).await? != activation {
            return Err(RecoveryError::Conflict);
        }
        match attached {
            CompletedAttachment::AlreadyActivated => Ok(()),
            CompletedAttachment::Reattached {
                attachment,
                confirmation,
            } => {
                activation.require_confirmation(&confirmation)?;
                self.runtime.acknowledge(attachment, completed).await
            }
        }
    }

    pub async fn resume(
        &self,
        caller: &Caller,
        request: &RecoveryRequest,
        holder: &str,
        now: &str,
        expires_at: &str,
        retry_at: &str,
    ) -> Result<CommitOutcome<RecoveryWork>, RecoveryError> {
        request.validate()?;
        let started = tokio::time::Instant::now();
        let at = SystemTime::UNIX_EPOCH
            .checked_add(Duration::from_secs(
                crate::seconds_from_rfc3339(now).ok_or(RecoveryError::InvalidEvidence)?,
            ))
            .ok_or(RecoveryError::InvalidEvidence)?;
        // The independent process feature and admission gates never turn an
        // accepted operation into an unknown or cancel its required settlement.
        let claim = match self
            .store
            .claim(caller, request, holder, now, expires_at)
            .await?
        {
            ClaimOutcome::Claimed(claim) => claim,
            ClaimOutcome::Waiting(work) | ClaimOutcome::Terminal(work) => {
                work.validate()?;
                if &work.request != request {
                    return Err(RecoveryError::Conflict);
                }
                if work.phase == RecoveryPhase::Completed {
                    self.activate_completed(caller, &work).await?;
                }
                return Ok(CommitOutcome::Recorded(*work));
            }
            ClaimOutcome::Unknown => return Ok(CommitOutcome::Unknown),
        };
        claim.work.validate()?;
        if &claim.work.request != request {
            return Err(RecoveryError::Conflict);
        }
        let (attachment, confirmation) = match self.runtime.reattach_existing(&claim).await {
            Ok(result) => result,
            Err(RecoveryError::Unavailable) => {
                let current = crate::rfc3339_at(at + started.elapsed());
                return self.store.retry(caller, &claim, &current, retry_at).await;
            }
            Err(error) => return Err(error),
        };
        confirmation.validate_for(&claim)?;
        // Claiming and reconnecting can consume the lease. Never publish using
        // the timestamp from before either await, even if the process clock moves.
        let current = crate::rfc3339_at(at + started.elapsed());
        let outcome = self
            .store
            .complete(caller, &claim, &confirmation, &current)
            .await?;
        if let CommitOutcome::Recorded(completed) = &outcome {
            completed.validate()?;
            // A receipt for another lease attempt cannot authorize this
            // attachment, even when it names the same request/native target.
            let mut expected = claim.work.clone();
            expected.phase = RecoveryPhase::Completed;
            expected.revision = expected
                .revision
                .checked_add(1)
                .ok_or(RecoveryError::InvalidEvidence)?;
            expected.lease = None;
            expected.completed_at = Some(current);
            if completed != &expected {
                return Err(RecoveryError::InvalidEvidence);
            }
            let activation = self.store.activation(caller, completed).await?;
            activation.require_confirmation(&confirmation)?;
            self.runtime.acknowledge(attachment, completed).await?;
        }
        // Unknown/known-abort drops the attachment without acknowledging it.
        // Runtime input is never sent, even if the earlier reconnect timed out.
        Ok(outcome)
    }
}

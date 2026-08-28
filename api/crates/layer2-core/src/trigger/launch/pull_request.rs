use super::*;
use crate::{seconds_from_rfc3339, FailureClass, PullRequestRecord, PullRequestState};

/// Normal provider-status cadence for an open pull request. The scheduler may
/// run more often, but an individual Launch will not consume GitHub reads more
/// frequently than this durable deadline.
pub const PULL_REQUEST_STATUS_INTERVAL_SECONDS: u64 = 60;

impl Launch {
    /// Move the next read-only status observation to the front of the durable
    /// queue. This command carries no provider authority and performs no read
    /// or write itself; the fenced worker remains the only provider caller.
    pub fn request_pull_request_status_refresh(
        &mut self,
        now: impl Into<String>,
        correlation_id: impl Into<String>,
    ) -> Result<(), TriggerValidationError> {
        let now = now.into();
        timestamp(&now, "pull_request.status_sync.requested_at")?;
        if self.state != LaunchState::PrOpen {
            return Err(TriggerValidationError::field(
                "pull_request.status_sync",
                "can be requested only for a linked open pull request",
            ));
        }
        let record = self.pull_request.as_mut().ok_or_else(|| {
            TriggerValidationError::field(
                "pull_request.status_sync",
                "requires a retained provider pull request",
            )
        })?;
        if record.state != PullRequestState::Open {
            return Err(TriggerValidationError::field(
                "pull_request.status_sync",
                "a closed or merged pull request has no refreshable open status",
            ));
        }
        record.status_sync.next_attempt_at = None;
        // A missing deadline means "due now" only when no suspended failure is
        // retained. Clearing the safe failure is the durable user request that
        // re-enables exactly one provider observation.
        record.status_sync.last_failure = None;
        self.updated_at = now.clone();
        self.push_event(
            now,
            correlation_id,
            LaunchEventKind::PullRequestStatusRefreshRequested,
        )
    }

    /// Whether this Launch is due for a browser-independent provider status
    /// read. A missing deadline is treated as due for compatibility with
    /// records created before status reconciliation was introduced.
    pub fn pull_request_status_due(&self, now: &str) -> Result<bool, TriggerValidationError> {
        if self.state != LaunchState::PrOpen
            || self
                .pull_request
                .as_ref()
                .is_none_or(|record| record.state != PullRequestState::Open)
        {
            return Ok(false);
        }
        timestamp(now, "pull_request.status_sync.now")?;
        let next = self
            .pull_request
            .as_ref()
            .and_then(|record| record.status_sync.next_attempt_at.as_deref());
        match next {
            None => Ok(self
                .pull_request
                .as_ref()
                .is_some_and(|record| record.status_sync.last_failure.is_none())),
            Some(next) => Ok(timestamp(next, "pull_request.status_sync.next_attempt_at")?
                <= timestamp(now, "pull_request.status_sync.now")?),
        }
    }

    /// Apply one positively identified, read-only provider observation under
    /// the shared Launch worker fence. The immutable user intent and provider
    /// identity cannot change during a status refresh.
    pub fn observe_pull_request_status(
        &mut self,
        lease: &WorkerLease,
        mut observed: PullRequestRecord,
        next_attempt_at: Option<String>,
        provider_request_id: Option<String>,
        now: impl Into<String>,
        correlation_id: impl Into<String>,
    ) -> Result<(), TriggerValidationError> {
        let mut candidate = self.clone();
        let now = now.into();
        validate_status_lease(&candidate, lease, &now)?;
        if candidate.state != LaunchState::PrOpen {
            return Err(TriggerValidationError::field(
                "pull_request.status_sync",
                "can refresh only a Launch with a linked pull request",
            ));
        }
        let current = candidate.pull_request.as_ref().ok_or_else(|| {
            TriggerValidationError::field(
                "pull_request.status_sync",
                "requires a retained provider pull request",
            )
        })?;
        current.validate_intent(&candidate.id)?;
        observed.validate_intent(&candidate.id)?;
        if current.state != PullRequestState::Open
            || !matches!(
                observed.state,
                PullRequestState::Open
                    | PullRequestState::ClosedUnmerged
                    | PullRequestState::Merged
            )
            || observed.intent != current.intent
            || observed.confirmation != current.confirmation
            || observed.intent_key != current.intent_key
            || observed.created_at != current.created_at
            || observed.provider_id != current.provider_id
            || observed.provider_node_id != current.provider_node_id
            || observed.number != current.number
            || observed.canonical_url != current.canonical_url
            || observed.provider_created_at != current.provider_created_at
        {
            return Err(TriggerValidationError::field(
                "pull_request.status_sync",
                "does not preserve the exact linked pull-request identity",
            ));
        }
        let next_attempt_at = match observed.state {
            PullRequestState::Open => {
                let next = next_attempt_at.ok_or_else(|| {
                    TriggerValidationError::field(
                        "pull_request.status_sync.next_attempt_at",
                        "is required while the provider pull request remains open",
                    )
                })?;
                if timestamp(&next, "pull_request.status_sync.next_attempt_at")?
                    <= timestamp(&now, "pull_request.status_sync.now")?
                {
                    return Err(TriggerValidationError::field(
                        "pull_request.status_sync.next_attempt_at",
                        "must be later than the current provider observation",
                    ));
                }
                Some(next)
            }
            PullRequestState::ClosedUnmerged | PullRequestState::Merged => {
                if next_attempt_at.is_some() {
                    return Err(TriggerValidationError::field(
                        "pull_request.status_sync.next_attempt_at",
                        "must be absent after the provider pull request closes",
                    ));
                }
                None
            }
            PullRequestState::Creating
            | PullRequestState::Updating
            | PullRequestState::RecoveryRequired => unreachable!("guarded above"),
        };
        observed.updated_at = now.clone();
        observed.status_sync.consecutive_failures = 0;
        observed.status_sync.last_failure = None;
        observed.status_sync.next_attempt_at = next_attempt_at;
        observed.status_sync.provider_request_id = provider_request_id;
        candidate.pull_request = Some(observed);
        let correlation_id = correlation_id.into();
        candidate.updated_at = now.clone();
        let refreshed = candidate
            .pull_request
            .as_ref()
            .expect("the provider observation was retained");
        candidate.push_event(
            now.clone(),
            correlation_id.clone(),
            LaunchEventKind::PullRequestStatusChanged {
                state: refreshed.state,
                mergeability: refreshed.mergeability,
                checks: refreshed.checks,
                failure: None,
            },
        )?;
        if candidate
            .pull_request
            .as_ref()
            .is_some_and(|record| record.state == PullRequestState::Merged)
        {
            candidate.transition(LaunchState::Completed, now, correlation_id)?;
        } else if candidate
            .pull_request
            .as_ref()
            .is_some_and(|record| record.state == PullRequestState::ClosedUnmerged)
        {
            // The Launch and provider link remain readable, but this attempt is
            // permanently over. Revoke the old session's GitHub capability so
            // a New Launch starts from fresh authority rather than renewing it.
            candidate.request_capability_revocation(now, correlation_id)?;
        }
        *self = candidate;
        Ok(())
    }

    /// Retain a safe provider-read failure and its bounded retry deadline
    /// without turning an already-open PR into another write effect.
    pub fn defer_pull_request_status(
        &mut self,
        lease: &WorkerLease,
        mut failure: LaunchFailure,
        next_attempt_at: Option<String>,
        provider_request_id: Option<String>,
        now: impl Into<String>,
    ) -> Result<(), TriggerValidationError> {
        let mut candidate = self.clone();
        let now = now.into();
        validate_status_lease(&candidate, lease, &now)?;
        if candidate.state != LaunchState::PrOpen {
            return Err(TriggerValidationError::field(
                "pull_request.status_sync",
                "can defer only a linked open pull request",
            ));
        }
        let record = candidate.pull_request.as_mut().ok_or_else(|| {
            TriggerValidationError::field(
                "pull_request.status_sync",
                "requires a retained provider pull request",
            )
        })?;
        if record.state != PullRequestState::Open
            || failure.correlation_id.trim().is_empty()
            || failure.code.trim().is_empty()
            || failure.detail.trim().is_empty()
            || failure.remediation.trim().is_empty()
            || timestamp(
                &failure.observed_at,
                "pull_request.status_sync.failure.observed_at",
            )? > timestamp(&now, "pull_request.status_sync.now")?
        {
            return Err(TriggerValidationError::field(
                "pull_request.status_sync.failure",
                "is not a safe failure for the linked open pull request",
            ));
        }
        if matches!(
            failure.class,
            FailureClass::RetryableTransient | FailureClass::ResourceQuota
        ) {
            let retry_at = next_attempt_at.clone().ok_or_else(|| {
                TriggerValidationError::field(
                    "pull_request.status_sync.next_attempt_at",
                    "a retryable provider read requires a durable deadline",
                )
            })?;
            if timestamp(&retry_at, "pull_request.status_sync.next_attempt_at")?
                <= timestamp(&now, "pull_request.status_sync.now")?
            {
                return Err(TriggerValidationError::field(
                    "pull_request.status_sync.next_attempt_at",
                    "must be later than the failed provider read",
                ));
            }
            failure.retry_at = Some(retry_at);
        } else {
            if next_attempt_at.is_some() {
                return Err(TriggerValidationError::field(
                    "pull_request.status_sync.next_attempt_at",
                    "must be absent for a provider read requiring user or operator action",
                ));
            }
            failure.retry_at = None;
        }
        record.status_sync.consecutive_failures = record
            .status_sync
            .consecutive_failures
            .checked_add(1)
            .ok_or_else(|| {
                TriggerValidationError::field(
                    "pull_request.status_sync.consecutive_failures",
                    "is exhausted",
                )
            })?;
        record.status_sync.next_attempt_at = next_attempt_at;
        record.status_sync.last_failure = Some(failure);
        record.status_sync.provider_request_id = provider_request_id;
        record.updated_at = now.clone();
        candidate.updated_at = now;
        let failure = record
            .status_sync
            .last_failure
            .clone()
            .expect("the status failure was retained");
        let state = record.state;
        let mergeability = record.mergeability;
        let checks = record.checks;
        candidate.push_event(
            candidate.updated_at.clone(),
            failure.correlation_id.clone(),
            LaunchEventKind::PullRequestStatusChanged {
                state,
                mergeability,
                checks,
                failure: Some(failure),
            },
        )?;
        *self = candidate;
        Ok(())
    }
}

fn validate_status_lease(
    launch: &Launch,
    lease: &WorkerLease,
    now: &str,
) -> Result<(), TriggerValidationError> {
    let current = launch.lease.as_ref().ok_or_else(|| {
        TriggerValidationError::field("worker_lease", "is missing from the Launch")
    })?;
    if current != lease
        || lease.fencing_token != launch.fencing_counter
        || timestamp(&lease.expires_at, "worker_lease.expires_at")?
            <= timestamp(now, "worker_lease.now")?
    {
        return Err(TriggerValidationError::field(
            "worker_lease",
            "is expired or no longer owns the Launch fence",
        ));
    }
    Ok(())
}

fn timestamp(value: &str, field: &str) -> Result<u64, TriggerValidationError> {
    seconds_from_rfc3339(value)
        .ok_or_else(|| TriggerValidationError::field(field, "is not a CMA timestamp"))
}

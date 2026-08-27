use super::{
    CapabilityRenewalRecord, Launch, LaunchEventKind, LaunchState, SessionCredentialRetirement,
    WorkerLease,
};
use crate::trigger::{CapabilityRefusal, CapabilityRevocationState, TriggerValidationError};

impl Launch {
    /// Whether durable session-key custody still needs terminal settlement.
    ///
    /// Every Launch reserves a deterministic session id. Retiring even when no
    /// session link was committed also fences a late credential mint whose
    /// provider response crossed cancellation.
    pub fn session_credential_requires_retirement(&self) -> bool {
        self.state.is_terminal() && self.session_credential_retirement.is_none()
    }

    /// Whether this Launch has permanently ended and must surrender GitHub
    /// authorization while leaving its session and conversation intact.
    pub fn capability_requires_revocation(&self) -> bool {
        self.reach.as_ref().is_some_and(|reach| {
            reach.state == CapabilityRevocationState::RevocationPending
                || (self.state.is_terminal() && reach.state != CapabilityRevocationState::Revoked)
        })
    }

    /// Atomically attach a freshly acknowledged runtime policy without
    /// changing the user-visible orchestration step.
    pub fn record_capability_renewal(
        &mut self,
        lease: &WorkerLease,
        renewal: CapabilityRenewalRecord,
        now: impl Into<String>,
        correlation_id: impl Into<String>,
    ) -> Result<(), TriggerValidationError> {
        let CapabilityRenewalRecord {
            reach,
            connection,
            authority,
            branch,
        } = renewal;
        let now = now.into();
        self.validate_capability_lease(lease, &now)?;
        let admission = self.capability_authority_admission_for(lease)?.clone();
        reach.validate()?;
        connection.validate_for(
            &self
                .revision
                .content
                .repository_access
                .required_scope_classes,
        )?;
        if reach.state != CapabilityRevocationState::Active
            || reach.policy_fence != lease.fencing_token
            || reach.service_id != connection.service_id
            || authority.subject != self.owner
            || authority.service_id != connection.service_id
            || authority.detects_drift(&connection)
            || reach.policy_generation != admission.target_policy_generation
            || reach.expires_at_epoch_seconds != admission.target_policy_expires_at_epoch_seconds
            || connection != admission.snapshot.connection
            || authority != admission.snapshot.authority
            || branch
                != *admission.snapshot.branch.as_ref().ok_or_else(|| {
                    TriggerValidationError::field(
                        "capability_authority_admission.branch",
                        "is missing from the committed renewal authority",
                    )
                })?
            || self
                .reach
                .as_ref()
                .is_some_and(|current| reach.policy_generation <= current.policy_generation)
        {
            return Err(TriggerValidationError::field(
                "reach",
                "the renewal does not advance this Launch's exact authority and fence",
            ));
        }
        let retained_branch = self.branch.as_ref().ok_or_else(|| {
            TriggerValidationError::field("branch", "the capability has no retained Launch branch")
        })?;
        if branch.name != retained_branch.name
            || branch.expected_head != retained_branch.expected_head
            || branch.created_at != retained_branch.created_at
        {
            return Err(TriggerValidationError::field(
                "branch",
                "the renewal observed a different Launch branch identity",
            ));
        }
        super::super::validate_sha("branch.current_head", &branch.current_head)?;
        self.connection = Some(connection);
        self.authority = Some(authority);
        self.branch = Some(branch);
        self.reach = Some(reach.clone());
        self.capability_authority_admission = None;
        self.updated_at = now;
        self.push_event(
            self.updated_at.clone(),
            correlation_id,
            LaunchEventKind::ReachChanged { reach },
        )
    }

    /// Project the latest closed runtime refusal without treating readiness as
    /// the complete per-call audit stream.
    ///
    /// Returns whether durable Launch state changed, so a maintenance worker
    /// can avoid an otherwise empty revision and event.
    pub fn record_capability_refusal_observation(
        &mut self,
        lease: &WorkerLease,
        refusal: Option<CapabilityRefusal>,
        now: impl Into<String>,
        correlation_id: impl Into<String>,
    ) -> Result<bool, TriggerValidationError> {
        let now = now.into();
        self.validate_capability_lease(lease, &now)?;
        let reach = self.reach.as_mut().ok_or_else(|| {
            TriggerValidationError::field("reach", "cannot observe an absent capability")
        })?;
        if reach.last_refusal == refusal {
            return Ok(false);
        }
        reach.last_refusal = refusal;
        reach.validate()?;
        let reach = reach.clone();
        self.updated_at = now.clone();
        self.push_event(now, correlation_id, LaunchEventKind::ReachChanged { reach })?;
        Ok(true)
    }

    /// Mark a lapsed active policy without claiming provider revocation.
    pub fn record_capability_expired(
        &mut self,
        lease: &WorkerLease,
        now_epoch_seconds: u64,
        now: impl Into<String>,
        correlation_id: impl Into<String>,
    ) -> Result<(), TriggerValidationError> {
        let now = now.into();
        self.validate_capability_lease(lease, &now)?;
        let Some(reach) = self.reach.as_mut() else {
            return Ok(());
        };
        if reach.state != CapabilityRevocationState::Active
            || reach.expires_at_epoch_seconds > now_epoch_seconds
        {
            return Ok(());
        }
        reach.state = CapabilityRevocationState::Expired;
        let reach = reach.clone();
        self.updated_at = now;
        self.push_event(
            self.updated_at.clone(),
            correlation_id,
            LaunchEventKind::ReachChanged { reach },
        )
    }

    /// Persist a permanent revoke intent before either enforcement boundary is
    /// contacted. Idempotent so cancellation and takeover can converge.
    pub fn request_capability_revocation(
        &mut self,
        now: impl Into<String>,
        correlation_id: impl Into<String>,
    ) -> Result<(), TriggerValidationError> {
        self.capability_authority_admission = None;
        let Some(reach) = self.reach.as_mut() else {
            return Ok(());
        };
        if reach.state == CapabilityRevocationState::Revoked
            || reach.state == CapabilityRevocationState::RevocationPending
        {
            return Ok(());
        }
        let now = now.into();
        reach.state = CapabilityRevocationState::RevocationPending;
        reach.revocation_requested_at = Some(now.clone());
        reach.revocation_effective_at = None;
        let reach = reach.clone();
        self.updated_at = now.clone();
        self.push_event(now, correlation_id, LaunchEventKind::ReachChanged { reach })
    }

    /// Record enforcement acknowledgement. `revoked` is reachable only after
    /// the runtime tombstone or provider credential retirement is proven.
    #[allow(clippy::too_many_arguments)]
    pub fn record_capability_revocation(
        &mut self,
        lease: &WorkerLease,
        generation: u64,
        fence: u64,
        runtime_audience: String,
        tombstone_acknowledged: bool,
        agent_key_revocation_confirmed: bool,
        now: impl Into<String>,
        correlation_id: impl Into<String>,
    ) -> Result<(), TriggerValidationError> {
        let now = now.into();
        self.validate_capability_lease(lease, &now)?;
        let reach = self.reach.as_mut().ok_or_else(|| {
            TriggerValidationError::field("reach", "cannot revoke an absent capability")
        })?;
        if !matches!(
            reach.state,
            CapabilityRevocationState::RevocationPending | CapabilityRevocationState::Revoked
        ) || generation <= reach.policy_generation
            || fence != lease.fencing_token
            || runtime_audience.trim().is_empty()
            || (!tombstone_acknowledged && !agent_key_revocation_confirmed)
        {
            return Err(TriggerValidationError::field(
                "reach",
                "the revocation acknowledgement does not advance the current fence",
            ));
        }
        reach.policy_generation = generation;
        reach.policy_fence = fence;
        reach.runtime_audience = runtime_audience;
        reach.refreshed_at = now.clone();
        reach.tombstone_acknowledged |= tombstone_acknowledged;
        reach.agent_key_revocation_confirmed |= agent_key_revocation_confirmed;
        reach.state = CapabilityRevocationState::Revoked;
        reach.revocation_effective_at = Some(now.clone());
        reach.validate()?;
        let reach = reach.clone();
        self.updated_at = now.clone();
        self.push_event(now, correlation_id, LaunchEventKind::ReachChanged { reach })
    }

    /// Persist positive credential-custody settlement independently of Reach.
    pub fn record_session_credential_retirement(
        &mut self,
        lease: &WorkerLease,
        retirement: SessionCredentialRetirement,
    ) -> Result<(), TriggerValidationError> {
        self.validate_capability_lease(lease, &retirement.settled_at)?;
        let expected = crate::trigger_session_id(&self.id)?;
        if retirement.session_id != expected
            || self
                .session
                .as_ref()
                .is_some_and(|session| session.session_id != retirement.session_id)
        {
            return Err(TriggerValidationError::field(
                "session_credential_retirement",
                "does not settle this Launch's deterministic session identity",
            ));
        }
        if let Some(current) = self.session_credential_retirement.as_ref() {
            if current == &retirement {
                return Ok(());
            }
            return Err(TriggerValidationError::field(
                "session_credential_retirement",
                "cannot replace positive custody settlement evidence",
            ));
        }
        self.updated_at = retirement.settled_at.clone();
        self.session_credential_retirement = Some(retirement);
        Ok(())
    }

    fn validate_capability_lease(
        &self,
        lease: &WorkerLease,
        now: &str,
    ) -> Result<(), TriggerValidationError> {
        let lease_expiry = crate::seconds_from_rfc3339(&lease.expires_at).ok_or_else(|| {
            TriggerValidationError::field("worker_lease.expires_at", "is not a CMA timestamp")
        })?;
        let now = crate::seconds_from_rfc3339(now).ok_or_else(|| {
            TriggerValidationError::field("capability.updated_at", "is not a CMA timestamp")
        })?;
        if self.lease.as_ref() != Some(lease)
            || self.fencing_counter != lease.fencing_token
            || lease_expiry <= now
        {
            return Err(TriggerValidationError::field(
                "worker_lease",
                "does not own this Launch capability mutation",
            ));
        }
        Ok(())
    }
}

pub(super) fn request_after_terminal_transition(
    launch: &mut Launch,
    to: LaunchState,
    now: &str,
    correlation_id: &str,
) -> Result<(), TriggerValidationError> {
    if to.is_terminal() {
        launch.request_capability_revocation(now, correlation_id)?;
    }
    Ok(())
}

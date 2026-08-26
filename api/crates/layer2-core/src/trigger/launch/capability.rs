use super::{BranchRecord, Launch, LaunchEventKind, LaunchState, WorkerLease};
use crate::trigger::{
    AuthorityProvenance, CapabilityRevocationState, ConnectionSnapshot, ReachStatus,
    TriggerValidationError,
};

impl Launch {
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
        reach: ReachStatus,
        connection: ConnectionSnapshot,
        authority: AuthorityProvenance,
        branch: BranchRecord,
        now: impl Into<String>,
        correlation_id: impl Into<String>,
    ) -> Result<(), TriggerValidationError> {
        let now = now.into();
        self.validate_capability_lease(lease, &now)?;
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
            || branch.canonical_url != retained_branch.canonical_url
        {
            return Err(TriggerValidationError::field(
                "branch",
                "the renewal observed a different Launch branch",
            ));
        }
        super::super::validate_sha("branch.current_head", &branch.current_head)?;
        self.connection = Some(connection);
        self.authority = Some(authority);
        self.branch = Some(branch);
        self.reach = Some(reach.clone());
        self.updated_at = now;
        self.push_event(
            self.updated_at.clone(),
            correlation_id,
            LaunchEventKind::ReachChanged { reach },
        )
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

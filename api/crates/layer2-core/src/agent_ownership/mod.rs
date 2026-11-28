//! Internal, versioned authority for an Agent transport across manager instances.
//!
//! These records are deliberately separate from the public Agent aggregate.
//! A token identifies an authority; possession alone is never proof it is live.

mod placement;
pub use placement::candidates;
mod heartbeat_retry;
mod lease_operation;
mod policy;
pub use policy::{MaintenancePolicy, MaintenanceStep, RetirementIntent, Settlement};
mod authority;
pub use authority::{execution_authority, execution_owner, Authority};
mod cleanup_pacing;
pub use cleanup_pacing::BackgroundHandoffError;
mod coordinator;
mod observations;
mod receiver_binding;
pub mod receiver_diagnostics;
mod transport;
pub use coordinator::ForeignReconciliationOwners;
pub use coordinator::{bounded, OwnershipCoordinator};
#[cfg(feature = "qualification-hooks")]
pub use coordinator::{qualification_owner_digest, CapturedOwner, RenewalWindow};
pub use observations::{observe_transport, observed_transport, transport_observation_scope};

use crate::AgentId;
pub use layer3_adapter::owner_protocol::OwnershipFailure;

pub const PROTOCOL_VERSION: u32 = 1;
// The receiver starts its shorter monotonic deadline before this lease is
// renewed at Mongo. Takeover still requires actual expiry or proved revocation.
pub const DEFAULT_LEASE_MS: u64 = 15_000;
pub const HEARTBEAT_INTERVAL_MS: u64 = 3_000;
pub const STORE_DEADLINE_MS: u64 = 2_000;
pub const MAX_RENEWAL_BATCH: usize = 8;
pub const DEFAULT_MAX_OWNED_AGENTS: u32 = 256;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Member {
    pub instance_id: String,
    pub protocol: u32,
    pub relay_address: String,
    pub expires_at_ms: i64,
    pub draining: bool,
    pub owned_agents: u32,
    pub max_owned_agents: u32,
}

impl Member {
    pub fn is_live(&self, now_ms: i64) -> bool {
        self.expires_at_ms > now_ms
    }

    pub fn can_claim(&self, now_ms: i64) -> bool {
        self.is_live(now_ms)
            && !self.draining
            && self.protocol == PROTOCOL_VERSION
            && self.owned_agents < self.max_owned_agents
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OwnershipToken {
    pub agent_id: AgentId,
    pub instance_id: String,
    pub epoch: u64,
    pub protocol: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OwnershipLease {
    pub token: OwnershipToken,
    pub expires_at_ms: i64,
}

/// A read-only lease observation with lifetime measured by the store clock.
pub struct ReconciliationOwner {
    pub token: OwnershipToken,
    pub remaining_ms: u64,
}

/// A partial match is usable for fallback only after a definite transaction abort.
#[derive(Debug, PartialEq, Eq)]
pub enum RenewalBatch {
    Committed(Vec<OwnershipLease>),
    PartialMatchAborted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Membership {
    /// Store time, not the caller's wall clock. Expiry never depends on TTL cleanup.
    pub now_ms: i64,
    pub members: Vec<Member>,
}

impl Membership {
    /// Never split one deployment into independent ownership protocol partitions.
    pub fn require_compatible(&self) -> Result<(), OwnershipError> {
        if self
            .members
            .iter()
            .any(|member| member.is_live(self.now_ms) && member.protocol != PROTOCOL_VERSION)
        {
            return Err(OwnershipError::IncompatibleProtocol);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OwnershipError {
    #[error("Agent transport ownership is no longer current")]
    Stale,
    #[error("Agent Manager ownership protocol is incompatible")]
    IncompatibleProtocol,
    #[error("Agent Manager ownership record is invalid: {0}")]
    Invalid(String),
    #[error("Agent Manager ownership store is unavailable: {0}")]
    Unavailable(String),
}

/// The production adapter uses majority writes and transaction fences. A released
/// owner row retains its epoch forever; acquisition must never reset that counter.
#[async_trait::async_trait]
pub trait OwnershipStore: Send + Sync {
    /// Independent rollout control; disabling diagnostics never disables fencing.
    fn receiver_diagnostics_enabled(&self) -> bool {
        false
    }

    async fn register(&self, member: Member) -> Result<Membership, OwnershipError>;
    /// Expired membership cannot be resurrected. A restarted process uses a new ID.
    async fn heartbeat(
        &self,
        instance_id: &str,
        draining: bool,
    ) -> Result<Membership, OwnershipError>;
    async fn membership(&self) -> Result<Membership, OwnershipError>;
    async fn owner(&self, agent_id: &AgentId) -> Result<Option<OwnershipLease>, OwnershipError>;
    /// A page of observations may defer background work. Missing observations
    /// always fall back to ordinary acquisition.
    async fn reconciliation_owners(
        &self,
        _ids: &[&AgentId],
    ) -> Result<Vec<ReconciliationOwner>, OwnershipError> {
        Ok(Vec::new())
    }
    /// Permanent custody, including expired and released epochs. Absence of a
    /// live owner never authorizes returning an opted-in Agent to legacy mode.
    async fn has_owner_history(&self, agent_id: &AgentId) -> Result<bool, OwnershipError>;
    /// Returns None for a live peer owner, another preferred member, or capacity.
    /// Missing process-local attachment is never evidence for takeover.
    /// Errors other than Unavailable prove no acquisition committed. If a
    /// store cannot establish the result, it must return Unavailable.
    async fn acquire(
        &self,
        agent_id: &AgentId,
        instance_id: &str,
    ) -> Result<Option<OwnershipLease>, OwnershipError>;
    /// Order a read of exact acquisition custody after any in-flight write
    /// from a completed or cancelled acquisition task. Return this member's
    /// token even when its lease has expired.
    async fn reconcile_acquisition(
        &self,
        _agent_id: &AgentId,
        _instance_id: &str,
    ) -> Result<Option<OwnershipToken>, OwnershipError> {
        Err(OwnershipError::Unavailable(
            "acquisition custody could not be reconciled".into(),
        ))
    }
    async fn renew(&self, token: &OwnershipToken) -> Result<OwnershipLease, OwnershipError>;
    fn supports_batch_renewal(&self) -> bool {
        false
    }
    async fn renew_batch(
        &self,
        _tokens: &[OwnershipToken],
    ) -> Result<RenewalBatch, OwnershipError> {
        Err(OwnershipError::Invalid(
            "batch renewal is unsupported".into(),
        ))
    }
    /// Call only after revoking local and receiving-runtime authority.
    async fn release(&self, token: &OwnershipToken) -> Result<(), OwnershipError>;
    async fn check(&self, token: &OwnershipToken) -> Result<(), OwnershipError>;
    /// Check the exact owner and its serving member and return the shorter
    /// remaining lifetime using the store clock. This read never renews either.
    async fn live_lease_remaining_ms(&self, token: &OwnershipToken) -> Result<u64, OwnershipError> {
        let membership = self.membership().await?;
        membership.require_compatible()?;
        let member = membership
            .members
            .iter()
            .find(|member| {
                member.instance_id == token.instance_id
                    && !member.draining
                    && member.protocol == PROTOCOL_VERSION
                    && member.is_live(membership.now_ms)
            })
            .ok_or(OwnershipError::Stale)?;
        let lease = self
            .owner(&token.agent_id)
            .await?
            .filter(|lease| lease.token == *token)
            .ok_or(OwnershipError::Stale)?;
        lease
            .expires_at_ms
            .min(member.expires_at_ms)
            .checked_sub(membership.now_ms)
            .and_then(|remaining| u64::try_from(remaining).ok())
            .filter(|remaining| *remaining > 0)
            .ok_or(OwnershipError::Stale)
    }

    /// Atomically retain the first diagnostic for this exact, live owner token.
    /// This must not renew/release authority or change Agent work or retry state.
    async fn record_receiver_failure(
        &self,
        _token: &OwnershipToken,
        _failure: OwnershipFailure,
    ) -> Result<(), OwnershipError> {
        Err(OwnershipError::Unavailable(
            "receiver diagnostics are not supported".into(),
        ))
    }
}

#[cfg(test)]
mod coordinator_tests;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
#[cfg(test)]
mod tests;

#[cfg(test)]
mod coordinator_race_tests;

#[cfg(test)]
mod receiver_loss_tests;

#[cfg(test)]
mod containment_diagnostic_tests;
#[cfg(test)]
mod containment_test_support;
#[cfg(test)]
mod containment_tests;
#[cfg(test)]
mod drain_containment_tests;
#[cfg(test)]
mod receiver_settlement_containment_tests;
#[cfg(test)]
mod retirement_containment_tests;

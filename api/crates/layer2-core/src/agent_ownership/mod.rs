//! Internal, versioned authority for an Agent transport across manager instances.
//!
//! These records are deliberately separate from the public Agent aggregate.
//! A token identifies an authority; possession alone is never proof it is live.

mod placement;
pub use placement::candidates;
mod authority;
pub use authority::{execution_authority, execution_owner, Authority};
mod coordinator;
mod observations;
mod transport;
pub use coordinator::{bounded, OwnershipCoordinator};
pub use observations::{observe_transport, observed_transport, transport_observation_scope};

use crate::AgentId;

pub const PROTOCOL_VERSION: u32 = 1;
pub const DEFAULT_LEASE_MS: u64 = 30_000;
pub const HEARTBEAT_INTERVAL_MS: u64 = 5_000;
pub const STORE_DEADLINE_MS: u64 = 2_000;
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
    async fn register(&self, member: Member) -> Result<Membership, OwnershipError>;
    /// Expired membership cannot be resurrected. A restarted process uses a new ID.
    async fn heartbeat(
        &self,
        instance_id: &str,
        draining: bool,
    ) -> Result<Membership, OwnershipError>;
    async fn membership(&self) -> Result<Membership, OwnershipError>;
    async fn owner(&self, agent_id: &AgentId) -> Result<Option<OwnershipLease>, OwnershipError>;
    /// Permanent custody, including expired and released epochs. Absence of a
    /// live owner never authorizes returning an opted-in Agent to legacy mode.
    async fn has_owner_history(&self, agent_id: &AgentId) -> Result<bool, OwnershipError>;
    /// Returns None for a live peer owner, another preferred member, or capacity.
    /// Missing process-local attachment is never evidence for takeover.
    async fn acquire(
        &self,
        agent_id: &AgentId,
        instance_id: &str,
    ) -> Result<Option<OwnershipLease>, OwnershipError>;
    async fn renew(&self, token: &OwnershipToken) -> Result<OwnershipLease, OwnershipError>;
    /// Call only after revoking local and receiving-runtime authority.
    async fn release(&self, token: &OwnershipToken) -> Result<(), OwnershipError>;
    async fn check(&self, token: &OwnershipToken) -> Result<(), OwnershipError>;
}

#[cfg(test)]
mod coordinator_tests;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
#[cfg(test)]
mod tests;

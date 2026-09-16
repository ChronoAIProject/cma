use super::*;
use crate::bridge::BridgeControlPlaneLease;
use layer2_core::{
    bridge::allocation::{AllocationError, AllocationRecord},
    sandbox::Sandbox,
};

/// The scheduling transaction is shared; authority and lifecycle ownership
/// remain distinct for a Channel root and an ordinary reused parent.
pub(super) enum RevivalPermit<'a> {
    Created(&'a ChannelRevivalAuthority),
    Reused(&'a ChannelReuseRevivalAuthority),
}
impl RevivalPermit<'_> {
    pub fn allocation(&self) -> &AllocationRecord {
        match self {
            Self::Created(p) => p.allocation(),
            Self::Reused(p) => p.allocation(),
        }
    }
    pub fn sandbox(&self) -> &Sandbox {
        match self {
            Self::Created(p) => p.sandbox(),
            Self::Reused(p) => p.sandbox(),
        }
    }
    pub fn human(&self) -> &BridgeControlPlaneLease {
        match self {
            Self::Created(p) => p.human(),
            Self::Reused(p) => p.human(),
        }
    }
    pub fn require_current(&self) -> Result<(), AllocationError> {
        match self {
            Self::Created(p) => p.require_current(),
            Self::Reused(p) => p.require_current(),
        }
    }
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Created(_) => "channel_created",
            Self::Reused(_) => "ordinary_reused",
        }
    }
    pub async fn fence(
        &self,
        allocations: &MongoBridgeAllocationStore,
        session: &mut ClientSession,
    ) -> Result<(), bridge_allocations::MutationError> {
        match self {
            Self::Created(p) => allocations.fence_channel_revival(p, session).await,
            Self::Reused(p) => allocations.fence_channel_reuse_revival(p, session).await,
        }
    }
}

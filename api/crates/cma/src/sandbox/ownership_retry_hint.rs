//! Advisory reconnect pacing; this port cannot authorize runtime effects.
use layer2_core::{agent_ownership::OwnershipToken, sandbox::Agent, AgentId};
use layer3_adapter::owner_protocol::FirstOwnershipFailure;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AdvisoryDecision {
    Proceed,
    Hold,
    Unavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HintWriteResult {
    Recorded,
    AlreadyPresent,
    NotApplicable,
    Stale,
    Unavailable,
}

/// Only the store constructs an observation. Its original token, clock and CAS
/// inputs stay immutable when the worker future is cancelled or the write retries.
#[async_trait::async_trait]
pub(crate) trait CapturedOwnershipProbe: Send + Sync {
    async fn record(&self, failure: &FirstOwnershipFailure) -> HintWriteResult;
}

#[async_trait::async_trait]
pub(crate) trait OwnershipRetryHints: Send + Sync {
    async fn preflight(&self, id: &AgentId) -> AdvisoryDecision;
    async fn capture(
        &self,
        claimed: &Agent,
        owner: &OwnershipToken,
    ) -> Option<Arc<dyn CapturedOwnershipProbe>>;
}

use layer2_core::{
    agent_ownership::OwnershipCoordinator, sandbox::agent_io::CommandDiscoveryWake, AgentId,
};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;
use tokio::time::Instant;

pub(super) const MIN_WAKE_SPACING: Duration = Duration::from_millis(50);

pub(crate) struct OwnedDiscoveryWake {
    ownership: Arc<OwnershipCoordinator>,
    signal: Arc<Notify>,
}

impl OwnedDiscoveryWake {
    pub(crate) fn new(ownership: Arc<OwnershipCoordinator>, signal: Arc<Notify>) -> Arc<Self> {
        Arc::new(Self { ownership, signal })
    }
}

#[async_trait::async_trait]
impl CommandDiscoveryWake for OwnedDiscoveryWake {
    async fn committed_cancel(&self, agent_id: &AgentId) {
        // No acquisition or remote IO on this fast path. A remote owner, drain
        // or expired lease falls back to polling; the worker still fences every
        // execution if ownership changes after this advisory observation.
        if self.ownership.local(agent_id).await.is_some() && self.ownership.is_ready() {
            self.signal.notify_one();
            tracing::debug!(agent_sha256 = %format!("{:x}", Sha256::digest(agent_id.as_str())), "Committed Cancel notified owner-local command discovery");
        } else {
            tracing::debug!(agent_sha256 = %format!("{:x}", Sha256::digest(agent_id.as_str())), "Committed Cancel discovery deferred to owner polling");
        }
    }
}

pub(super) async fn notified(signal: &Option<Arc<Notify>>, not_before: Instant) {
    let Some(signal) = signal else {
        return std::future::pending().await;
    };
    // Keep the single Notify permit pending throughout pacing/backpressure.
    // Sleeping after consumption would lose the hint when select cancels us.
    tokio::time::sleep_until(not_before).await;
    signal.notified().await;
}

#[cfg(test)]
#[path = "discovery_wake_tests.rs"]
mod tests;

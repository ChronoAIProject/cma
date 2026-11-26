//! Typed observations for non-default local qualification builds.

use crate::{AgentId, OperationClaim};
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlOperation {
    Stop,
    Delete,
}

impl ControlOperation {
    fn name(self) -> &'static str {
        match self {
            Self::Stop => "stop",
            Self::Delete => "delete",
        }
    }
}

/// Immutable selector for one subject and original idempotency request.
/// Qualification transport receives only the versioned digest.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ControlContext {
    digest: [u8; 32],
    agent_digest: [u8; 32],
}

impl ControlContext {
    pub fn from_digests(digest: [u8; 32], agent_digest: [u8; 32]) -> Self {
        Self {
            digest,
            agent_digest,
        }
    }

    pub fn digest(&self) -> [u8; 32] {
        self.digest
    }

    pub fn agent_digest(&self) -> [u8; 32] {
        self.agent_digest
    }

    /// The public handler supplies only its newly acquired claim.
    pub fn from_claim(
        operation: ControlOperation,
        agent: &AgentId,
        claim: &OperationClaim,
    ) -> Option<Self> {
        if claim.operation() != format!("agent:{}:{}", operation.name(), agent.as_str()) {
            return None;
        }
        let mut encoder = Sha256::new();
        encoder.update(b"cma-qualification/control-after-schedule/v1");
        for field in [
            claim.operation().as_bytes(),
            claim.subject().as_str().as_bytes(),
            claim.key().as_bytes(),
            claim.fingerprint(),
        ] {
            encoder.update((field.len() as u64).to_be_bytes());
            encoder.update(field);
        }
        Some(Self::from_digests(
            encoder.finalize().into(),
            agent_digest(agent),
        ))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScheduleEffect {
    Scheduled,
    Existing,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ControlSchedule {
    pub agent_revision: u64,
    pub effect: ScheduleEffect,
}

/// Runs only after durable scheduling and before the existing completion path.
#[async_trait::async_trait]
pub trait ControlCheckpoint: Send + Sync {
    async fn after_schedule(&self, context: ControlContext, outcome: ControlSchedule);
}

pub(crate) fn agent_digest(agent: &AgentId) -> [u8; 32] {
    let mut encoder = Sha256::new();
    encoder.update(b"cma-qualification/agent/v1\0");
    encoder.update(agent.as_str().as_bytes());
    encoder.finalize().into()
}

impl From<&crate::sandbox::store::AgentControlResult> for ControlSchedule {
    fn from(result: &crate::sandbox::store::AgentControlResult) -> Self {
        use crate::sandbox::store::AgentControlResult;
        Self {
            agent_revision: result.agent().revision,
            effect: match result {
                AgentControlResult::Scheduled(_) => ScheduleEffect::Scheduled,
                AgentControlResult::Existing(_) => ScheduleEffect::Existing,
            },
        }
    }
}

/// Instrument only an already captured local renewal invocation.
pub trait RenewalHook: Send + Sync {
    fn selected(&self, authority: &std::sync::Arc<crate::agent_ownership::Authority>) -> bool;

    fn take_renewal(
        &self,
        authority: &std::sync::Arc<crate::agent_ownership::Authority>,
        window: crate::agent_ownership::RenewalWindow,
    ) -> Option<RenewalAttempt>;
}

pub(crate) type RenewalResult =
    Result<crate::agent_ownership::OwnershipLease, crate::agent_ownership::OwnershipError>;

pub struct RenewalAttempt {
    pub(crate) write: std::pin::Pin<Box<dyn std::future::Future<Output = RenewalResult> + Send>>,
    pub(crate) waiter: RenewalWaiter,
}

impl RenewalAttempt {
    pub fn new(
        write: impl std::future::Future<Output = RenewalResult> + Send + 'static,
        on_caller_drop: impl FnOnce() + Send + 'static,
    ) -> Self {
        Self {
            write: Box::pin(write),
            waiter: RenewalWaiter(Some(Box::new(on_caller_drop))),
        }
    }
}

/// Dropping this waiter does not cancel or consume the store write.
pub(crate) struct RenewalWaiter(Option<Box<dyn FnOnce() + Send>>);

impl RenewalWaiter {
    pub(crate) async fn deliver(
        mut self,
        result: RenewalResult,
        window: crate::agent_ownership::RenewalWindow,
        token: &crate::agent_ownership::OwnershipToken,
    ) -> RenewalResult {
        if result
            .as_ref()
            .is_ok_and(|lease| window.accepts(token, lease))
        {
            std::future::pending::<()>().await;
        }
        self.0.take();
        result
    }
}

impl Drop for RenewalWaiter {
    fn drop(&mut self) {
        if let Some(on_caller_drop) = self.0.take() {
            on_caller_drop();
        }
    }
}

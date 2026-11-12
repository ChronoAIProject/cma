//! Protocol-neutral personal recovery commands for application composition.
//! Discovery metadata is deliberately not accepted as caller authority.
use async_trait::async_trait;
use serde::Serialize;

use super::{
    ports::{CommitOutcome, RecoveryError},
    state::{RecoveryPhase, RecoveryRequest, RecoveryWork},
    CapacityRecoveryDisposition,
};
use crate::{AgentId, Caller};

#[derive(Debug, Serialize)]
pub struct RecoveryStatus {
    pub agent_id: AgentId,
    pub disposition: CapacityRecoveryDisposition,
    pub phase: Option<RecoveryPhase>,
    pub attempts: u32,
    pub retry_at: Option<String>,
    /// An identity for the original request, never an authorization token.
    pub operation_key: Option<String>,
}

/// Implementations freshly authorize each operation through resource access,
/// then recheck current policy/lineage in the storage transaction. A retained
/// Caller must be revalidated; owner IDs from discovery are not Callers.
/// Transport adapters must not serialize RecoveryWork's private native facts.
#[async_trait]
pub trait CapacityRecoveryCommands: Send + Sync {
    async fn status(
        &self,
        caller: &Caller,
        agent_id: &AgentId,
        request: Option<&RecoveryRequest>,
    ) -> Result<RecoveryStatus, RecoveryError>;

    /// New work is default-off. Exact accepted work remains available after
    /// admission closes; an absent carrier never establishes a known abort.
    async fn admit(
        &self,
        caller: &Caller,
        request: &RecoveryRequest,
    ) -> Result<CommitOutcome<RecoveryWork>, RecoveryError>;

    /// Execute one bounded accepted-work attempt using the configured worker
    /// holder. Completed retries finish attachment activation at the original
    /// fence. Never send input, reopen a turn or fall back to ordinary reconnect.
    async fn resume(
        &self,
        caller: &Caller,
        request: &RecoveryRequest,
    ) -> Result<CommitOutcome<RecoveryWork>, RecoveryError>;
}

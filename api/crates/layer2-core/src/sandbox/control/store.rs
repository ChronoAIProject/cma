use std::sync::Arc;

use crate::ids::SandboxCommandId;
use crate::sandbox::store::{Page, PageRequest};

use super::{
    SandboxControlAuthorization, SandboxControlError, SandboxControlFailure, SandboxControlRecord,
};

#[async_trait::async_trait]
pub trait SandboxControlStore: Send + Sync {
    /// Exact accepted retries remain available while new revival is paused.
    async fn lookup_sandbox_control(
        &self,
        _candidate: &SandboxControlRecord,
    ) -> Result<Option<SandboxControlRecord>, SandboxControlError> {
        Ok(None)
    }
    /// Persist a new record and admission evidence atomically with its current
    /// resource-policy guard. Replay never changes the old command or deadline.
    async fn reserve_sandbox_control(
        &self,
        record: SandboxControlRecord,
        authorization: Arc<SandboxControlAuthorization>,
    ) -> Result<SandboxControlRecord, SandboxControlError>;
    async fn sandbox_control(
        &self,
        id: &SandboxCommandId,
    ) -> Result<Option<SandboxControlRecord>, SandboxControlError>;
    async fn queued_sandbox_controls(
        &self,
        page: &PageRequest,
    ) -> Result<Page<SandboxControlRecord>, SandboxControlError>;
    /// Applies only the model-planned transition. Command completion, the exact
    /// Sandbox write/event and execution audit share a transaction. Competing
    /// workers use the record revision as their fence; no runtime IO occurs here.
    async fn apply_sandbox_control(
        &self,
        record: &SandboxControlRecord,
        authorization: Arc<SandboxControlAuthorization>,
    ) -> Result<SandboxControlRecord, SandboxControlError>;
    async fn reject_sandbox_control(
        &self,
        record: &SandboxControlRecord,
        reason: SandboxControlFailure,
    ) -> Result<SandboxControlRecord, SandboxControlError>;
}

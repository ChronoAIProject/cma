//! Provider-owned graceful recovery of an unpublished runtime incarnation.
use serde::{Deserialize, Serialize};

use crate::{AdapterError, Result, RuntimeAdoptionRequest, RuntimeHandle, RuntimeStatusRequest};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoveryConversation {
    pub agent_id: String,
    pub conversation_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoveryConversationHistory {
    pub agent_id: String,
    pub conversation_id: String,
    pub history_sha256: String,
}

#[derive(Debug, Clone)]
pub struct RuntimeRecoveryInspectionRequest {
    pub runtime: RuntimeHandle,
    pub identity: RuntimeStatusRequest,
    pub boot_id: String,
    pub conversations: Vec<RecoveryConversation>,
}

#[derive(Debug, Clone)]
pub struct RuntimeRecoveryInspection {
    /// Opaque provider evidence; callers persist it without interpreting it.
    pub provider_plan: serde_json::Value,
    pub provider_plan_sha256: String,
    pub histories: Vec<RecoveryConversationHistory>,
    pub runtime_process_absent: bool,
}

#[derive(Debug, Clone)]
pub struct RuntimeRecoveryQuiescenceRequest {
    pub source: RuntimeRecoveryInspectionRequest,
    /// Already admitted durable lifecycle authority, never a caller-supplied hint.
    pub operation_id: String,
    pub provider_plan: serde_json::Value,
    pub provider_plan_sha256: String,
}

#[derive(Debug, Clone)]
pub struct RuntimeRecoveryQuiescence {
    pub histories: Vec<RecoveryConversationHistory>,
}

#[derive(Debug, Clone)]
pub struct NonexecutingRecoveryVerification {
    pub source: RuntimeAdoptionRequest,
    pub provider_plan: serde_json::Value,
    pub provider_plan_sha256: String,
}

#[async_trait::async_trait]
pub trait RuntimeRecovery: Send + Sync {
    /// Confirm a completed native Close on the observed writer without repairing journals.
    /// False means the exact writer inspected the process and could not prove a Close.
    async fn prove_agent_process_closed(
        &self,
        _handle: &crate::RuntimeHandle,
        _proof: &crate::wire::AgentProcessCloseProof,
    ) -> Result<bool> {
        Err(AdapterError::Unsupported(
            "native Agent Close proof is unavailable".into(),
        ))
    }

    /// Fence a delayed numbered Open on the exact boot before it can start.
    async fn fence_agent_restart(
        &self,
        _handle: &RuntimeHandle,
        _request: &crate::wire::AgentRestartFence,
    ) -> Result<()> {
        Err(AdapterError::Unsupported(
            "native Agent restart fencing is unavailable".into(),
        ))
    }

    /// Read-only proof for reopening already restored, idle conversations.
    /// Requires complete settled history and no Agent/unowned process. This
    /// grants no authority to stop a process or suspend the retained runtime.
    async fn inspect_idle_reopen(
        &self,
        _request: &RuntimeRecoveryInspectionRequest,
    ) -> Result<RuntimeRecoveryInspection> {
        Err(AdapterError::Unsupported(
            "idle conversation reopening proof is unavailable".into(),
        ))
    }

    /// Read-only provider proof for the complete, empty-Agent case. Callers must
    /// establish that scope transactionally; this method grants no mutation authority.
    async fn inspect_nonexecuting_recovery(
        &self,
        _request: &RuntimeAdoptionRequest,
    ) -> Result<Option<RuntimeRecoveryInspection>> {
        Ok(None)
    }

    /// Refresh stopped-container evidence while retaining the admitted immutable
    /// runtime/request/home source, then verify that exact fresh observation.
    async fn verify_nonexecuting_recovery(
        &self,
        _request: &NonexecutingRecoveryVerification,
    ) -> Result<()> {
        Err(AdapterError::Rejected(
            "nonexecuting runtime recovery is unsupported".into(),
        ))
    }

    /// Final provider observation and suspension CAS under the same source proof.
    /// The caller must already have committed the empty-Agent quiescence receipt.
    async fn suspend_nonexecuting_recovery(
        &self,
        _request: &NonexecutingRecoveryVerification,
    ) -> Result<()> {
        Err(AdapterError::Rejected(
            "nonexecuting runtime suspension is unsupported".into(),
        ))
    }

    /// Read only. This evidence does not itself authorize process termination.
    async fn inspect_recovery(
        &self,
        request: &RuntimeRecoveryInspectionRequest,
    ) -> Result<RuntimeRecoveryInspection>;

    /// Execute/replay a durable recovery plan after admission. The provider must
    /// journal before effects and retain all original history and user files.
    async fn quiesce_recovery(
        &self,
        request: &RuntimeRecoveryQuiescenceRequest,
    ) -> Result<RuntimeRecoveryQuiescence>;
}

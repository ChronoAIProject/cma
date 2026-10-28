//! Provider-owned graceful recovery of an unpublished runtime incarnation.
use serde::{Deserialize, Serialize};

use crate::{Result, RuntimeHandle, RuntimeStatusRequest};

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

#[async_trait::async_trait]
pub trait RuntimeRecovery: Send + Sync {
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

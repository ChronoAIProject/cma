use serde::{Deserialize, Serialize};

use crate::sandbox::AgentWorkReceipt;
use crate::{AgentId, Caller, OperationClaim, WorktreeId};
use layer3_adapter::AgentRestartRuntime;

use super::AgentControlTarget;

/// An internal request. The service supplies the observation after verifying
/// the exact native Open and Close against the retained runtime.
#[derive(Clone)]
pub struct AgentRestartRequest {
    pub target: AgentControlTarget,
    pub caller: Caller,
    pub claim: OperationClaim,
    pub observed_process: Option<AgentRestartSource>,
    pub requested_at: String,
}

/// The original accepted result, independent of later lifecycle work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentRestartAcceptance {
    pub operation_id: String,
    pub agent_id: AgentId,
    pub worktree_id: WorktreeId,
    pub revision: u64,
    pub accepted_at: String,
    pub source: AgentRestartSource,
    pub next_incarnation: u64,
}

/// Exact process and durable source observed before admission. The acceptance
/// retains this unchanged across dispatch and reconciliation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentRestartSource {
    pub conversation_id: String,
    pub predecessor_open: AgentWorkReceipt,
    pub predecessor_close: AgentWorkReceipt,
    pub runtime: AgentRestartRuntime,
    pub worktree_revision: u64,
    pub workspace_revision: u64,
    pub sandbox_revision: u64,
}

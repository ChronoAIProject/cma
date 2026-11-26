use serde::{Deserialize, Serialize};

use crate::sandbox::{Agent, AgentWorkReceipt};
use crate::{AgentId, Caller, OperationClaim, WorktreeId};
use layer3_adapter::{AgentOpenRequest, AgentRestartRuntime};

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
    pub predecessor_stop_operation: String,
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

/// A restart accepted in the control transaction and its immutable Open,
/// once the worker has crossed the durable dispatch boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRestartWork {
    pub acceptance: AgentRestartAcceptance,
    pub dispatched_open: Option<AgentOpenRequest>,
}

#[derive(Clone)]
pub struct AgentRestartDispatch {
    pub claimed: Agent,
    pub work: AgentRestartWork,
    pub open: AgentOpenRequest,
    pub dispatched_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRestartTeardown {
    pub operation_id: String,
    pub acceptance: AgentRestartAcceptance,
    pub open: AgentOpenRequest,
}

impl AgentRestartTeardown {
    pub fn fence(
        &self,
        authority_fence: u64,
        worktree_path: String,
        close_successor: bool,
    ) -> layer3_adapter::wire::AgentRestartFence {
        use layer3_adapter::wire::{AgentProcessCloseProof, AgentRestartFence};
        use layer3_adapter::{AgentCloseRequest, AgentSlot};
        let source = &self.acceptance.source;
        AgentRestartFence {
            predecessor: AgentProcessCloseProof {
                runtime: source.runtime.clone(),
                close: AgentCloseRequest::new(
                    source.predecessor_close.operation_id.clone(),
                    AgentSlot::new(self.acceptance.agent_id.as_str()),
                ),
                open_request_sha256: source.predecessor_open.request_sha256.clone(),
                worktree_path,
            },
            successor_operation_id: self.open.operation_id.clone(),
            authority_fence,
            successor_open_request_sha256: close_successor
                .then(|| self.open.request_sha256.clone()),
        }
    }
}

#[derive(Clone)]
pub struct AgentRestartTeardownPublication {
    pub claimed: Agent,
    pub teardown: AgentRestartTeardown,
    pub acknowledgement: layer3_adapter::wire::AgentRestartFence,
    pub parents: super::AgentWorkPublicationParents,
    pub published_at: String,
}

use serde::{Deserialize, Serialize};

use crate::{AgentId, LaunchId, SandboxId, ServiceId, WorkspaceId};

use super::{
    LaunchBlockedReason, LaunchFailure, LaunchState, LaunchStep, RepeatChoice,
    ServiceRequirementState,
};
use crate::sandbox::FirstMessageDelivery;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum LaunchEventKind {
    Created,
    ConfirmationRecorded,
    StateChanged {
        from: LaunchState,
        to: LaunchState,
    },
    StepChanged {
        step: LaunchStep,
    },
    SandboxResolved {
        sandbox_id: SandboxId,
        reused: bool,
    },
    WorkspaceResolved {
        workspace_id: WorkspaceId,
        reused: bool,
    },
    AgentResolved {
        agent_id: AgentId,
        reused: bool,
    },
    RepeatChoiceRecorded {
        choice: RepeatChoice,
    },
    RequirementChanged {
        service: ServiceId,
        state: ServiceRequirementState,
    },
    Blocked {
        reason: LaunchBlockedReason,
    },
    Resumed {
        reason: LaunchBlockedReason,
    },
    FirstMessageChanged {
        state: FirstMessageDelivery,
    },
    Failed {
        failure: LaunchFailure,
    },
    Cancelled,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchEvent {
    pub launch_id: LaunchId,
    pub seq: u64,
    pub appended_at: String,
    pub kind: LaunchEventKind,
}

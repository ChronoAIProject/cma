use serde::{Deserialize, Serialize};

use crate::{AgentId, LaunchId, SandboxId, ServiceId, WorkspaceId};

use super::{
    LaunchBlockedReason, LaunchEffectKind, LaunchFailure, LaunchState, LaunchStep, RepeatChoice,
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
    SandboxReplaced {
        previous_sandbox_id: SandboxId,
        sandbox_id: SandboxId,
    },
    WorkspaceResolved {
        workspace_id: WorkspaceId,
        reused: bool,
    },
    WorkspaceReplaced {
        previous_workspace_id: WorkspaceId,
        workspace_id: WorkspaceId,
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
    Deferred {
        retry_at: String,
    },
    RetryScheduled {
        attempt: u32,
        retry_at: String,
    },
    EffectReconciled {
        kind: LaunchEffectKind,
        result_id: String,
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

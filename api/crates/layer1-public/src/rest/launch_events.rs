//! Public wire projection of durable Launch events.

use layer2_core::sandbox::FirstMessageDelivery;
use layer2_core::trigger_domain::{
    LaunchBlockedReason, LaunchEffectKind, LaunchEvent, LaunchEventKind, LaunchFailure,
    LaunchState, LaunchStep, LaunchStepKey, LaunchStepStatus, RepeatChoice,
    ServiceRequirementState,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

macro_rules! wire_value {
    ($view:ident, $domain:ident, $($variant:ident),+ $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
        #[serde(rename_all = "snake_case")]
        pub enum $view {
            $($variant),+
        }

        impl From<$domain> for $view {
            fn from(value: $domain) -> Self {
                match value {
                    $($domain::$variant => Self::$variant),+
                }
            }
        }

        impl From<$view> for $domain {
            fn from(value: $view) -> Self {
                match value {
                    $($view::$variant => Self::$variant),+
                }
            }
        }
    };
}

wire_value!(
    LaunchStateValue,
    LaunchState,
    Draft,
    Resolving,
    SandboxPending,
    WorkspacePending,
    AgentPending,
    MessagePending,
    Ready,
    Completed,
    FailedRetryable,
    FailedTerminal,
    Cancelled,
);

wire_value!(
    LaunchStepKeyValue,
    LaunchStepKey,
    SigningIn,
    CheckingRequirements,
    Sandbox,
    Workspace,
    Agent,
    FirstMessage,
    Ready,
);

wire_value!(
    LaunchStepStatusValue,
    LaunchStepStatus,
    Pending,
    Running,
    Completed,
    Blocked,
    Retryable,
    Failed,
);

wire_value!(RepeatChoiceValue, RepeatChoice, Resume, New,);

wire_value!(
    ServiceRequirementStateValue,
    ServiceRequirementState,
    Satisfied,
    Inactive,
    Missing,
    Unknown,
);

wire_value!(
    FirstMessageStateValue,
    FirstMessageDelivery,
    Queued,
    DispatchFenced,
    WriteConfirmed,
    ExecutionObserved,
    Completed,
    Failed,
    RecoveryRequired,
);

wire_value!(
    LaunchEffectKindValue,
    LaunchEffectKind,
    CreateSandbox,
    ReviveSandbox,
    RepairSandboxServiceGrant,
    CreateWorkspace,
    CreateAgent,
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LaunchStepView {
    pub key: LaunchStepKeyValue,
    pub status: LaunchStepStatusValue,
    pub safe_detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
}

impl From<LaunchStep> for LaunchStepView {
    fn from(value: LaunchStep) -> Self {
        Self {
            key: value.key.into(),
            status: value.status.into(),
            safe_detail: value.safe_detail,
            started_at: value.started_at,
            completed_at: value.completed_at,
        }
    }
}

impl From<LaunchStepView> for LaunchStep {
    fn from(value: LaunchStepView) -> Self {
        Self {
            key: value.key.into(),
            status: value.status.into(),
            safe_detail: value.safe_detail,
            started_at: value.started_at,
            completed_at: value.completed_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum LaunchBlockedReasonView {
    ConnectionRequired { service: String },
    OrchestrationPaused,
    SandboxPausedReviving,
    RecoveryRequired,
}

impl From<LaunchBlockedReason> for LaunchBlockedReasonView {
    fn from(value: LaunchBlockedReason) -> Self {
        match value {
            LaunchBlockedReason::ConnectionRequired { service } => Self::ConnectionRequired {
                service: service.to_string(),
            },
            LaunchBlockedReason::OrchestrationPaused => Self::OrchestrationPaused,
            LaunchBlockedReason::SandboxPausedReviving => Self::SandboxPausedReviving,
            LaunchBlockedReason::RecoveryRequired => Self::RecoveryRequired,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LaunchFailureView {
    pub code: String,
    pub safe_detail: String,
    pub retryable: bool,
    pub occurred_at: String,
}

impl From<LaunchFailure> for LaunchFailureView {
    fn from(value: LaunchFailure) -> Self {
        Self {
            code: value.code,
            safe_detail: value.safe_detail,
            retryable: value.retryable,
            occurred_at: value.occurred_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum LaunchEventKindView {
    Created,
    ConfirmationRecorded,
    StateChanged {
        from: LaunchStateValue,
        to: LaunchStateValue,
    },
    StepChanged {
        step: LaunchStepView,
    },
    SandboxResolved {
        sandbox_id: String,
        reused: bool,
    },
    SandboxReplaced {
        previous_sandbox_id: String,
        sandbox_id: String,
    },
    WorkspaceResolved {
        workspace_id: String,
        reused: bool,
    },
    WorkspaceReplaced {
        previous_workspace_id: String,
        workspace_id: String,
    },
    AgentResolved {
        agent_id: String,
        reused: bool,
    },
    RepeatChoiceRecorded {
        choice: RepeatChoiceValue,
    },
    RequirementChanged {
        service: String,
        state: ServiceRequirementStateValue,
    },
    Blocked {
        reason: LaunchBlockedReasonView,
    },
    Resumed {
        reason: LaunchBlockedReasonView,
    },
    Deferred {
        retry_at: String,
    },
    RetryScheduled {
        attempt: u32,
        retry_at: String,
    },
    EffectReconciled {
        kind: LaunchEffectKindValue,
        result_id: String,
    },
    FirstMessageChanged {
        state: FirstMessageStateValue,
    },
    Failed {
        failure: LaunchFailureView,
    },
    Cancelled,
    Completed,
}

impl From<LaunchEventKind> for LaunchEventKindView {
    fn from(value: LaunchEventKind) -> Self {
        match value {
            LaunchEventKind::Created => Self::Created,
            LaunchEventKind::ConfirmationRecorded => Self::ConfirmationRecorded,
            LaunchEventKind::StateChanged { from, to } => Self::StateChanged {
                from: from.into(),
                to: to.into(),
            },
            LaunchEventKind::StepChanged { step } => Self::StepChanged { step: step.into() },
            LaunchEventKind::SandboxResolved { sandbox_id, reused } => Self::SandboxResolved {
                sandbox_id: sandbox_id.to_string(),
                reused,
            },
            LaunchEventKind::SandboxReplaced {
                previous_sandbox_id,
                sandbox_id,
            } => Self::SandboxReplaced {
                previous_sandbox_id: previous_sandbox_id.to_string(),
                sandbox_id: sandbox_id.to_string(),
            },
            LaunchEventKind::WorkspaceResolved {
                workspace_id,
                reused,
            } => Self::WorkspaceResolved {
                workspace_id: workspace_id.to_string(),
                reused,
            },
            LaunchEventKind::WorkspaceReplaced {
                previous_workspace_id,
                workspace_id,
            } => Self::WorkspaceReplaced {
                previous_workspace_id: previous_workspace_id.to_string(),
                workspace_id: workspace_id.to_string(),
            },
            LaunchEventKind::AgentResolved { agent_id, reused } => Self::AgentResolved {
                agent_id: agent_id.to_string(),
                reused,
            },
            LaunchEventKind::RepeatChoiceRecorded { choice } => Self::RepeatChoiceRecorded {
                choice: choice.into(),
            },
            LaunchEventKind::RequirementChanged { service, state } => Self::RequirementChanged {
                service: service.to_string(),
                state: state.into(),
            },
            LaunchEventKind::Blocked { reason } => Self::Blocked {
                reason: reason.into(),
            },
            LaunchEventKind::Resumed { reason } => Self::Resumed {
                reason: reason.into(),
            },
            LaunchEventKind::Deferred { retry_at } => Self::Deferred { retry_at },
            LaunchEventKind::RetryScheduled { attempt, retry_at } => {
                Self::RetryScheduled { attempt, retry_at }
            }
            LaunchEventKind::EffectReconciled { kind, result_id } => Self::EffectReconciled {
                kind: kind.into(),
                result_id,
            },
            LaunchEventKind::FirstMessageChanged { state } => Self::FirstMessageChanged {
                state: state.into(),
            },
            LaunchEventKind::Failed { failure } => Self::Failed {
                failure: failure.into(),
            },
            LaunchEventKind::Cancelled => Self::Cancelled,
            LaunchEventKind::Completed => Self::Completed,
        }
    }
}

/// One immutable event in an owned Launch's durable event log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LaunchEventView {
    pub launch_id: String,
    pub seq: u64,
    pub appended_at: String,
    pub kind: LaunchEventKindView,
}

impl From<LaunchEvent> for LaunchEventView {
    fn from(value: LaunchEvent) -> Self {
        Self {
            launch_id: value.launch_id.to_string(),
            seq: value.seq,
            appended_at: value.appended_at,
            kind: value.kind.into(),
        }
    }
}

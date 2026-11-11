use crate::sandbox::{AgentStatus, FirstMessageDelivery, SandboxCredentialPrerequisiteError};
use crate::RuntimeCapability;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AgentOperationError {
    #[error(transparent)]
    Budget(crate::budget::BudgetError),
    #[error(transparent)]
    Connection(#[from] SandboxCredentialPrerequisiteError),
    #[error("forbidden")]
    Forbidden,
    #[error("agent not found")]
    NotFound,
    #[error("agent status does not permit this operation: {status:?}")]
    StatusConflict { status: AgentStatus },
    #[error("agent has no active turn")]
    NoActiveTurn,
    #[error("agent approval is not pending: {approval_id}")]
    ApprovalNotPending { approval_id: String },
    #[error("agent first message still owns input delivery: {state:?}")]
    FirstMessageReserved { state: FirstMessageDelivery },
    #[error("agent operation is still dispatching: {operation_sequence}")]
    DispatchInProgress { operation_sequence: u64 },
    #[error("agent operation requires explicit recovery: {operation_sequence}")]
    DispatchRecoveryRequired { operation_sequence: u64 },
    #[error("agent runtime does not support {capability:?}")]
    CapabilityUnsupported { capability: RuntimeCapability },
    #[error("agent input content is not supported")]
    ContentUnsupported,
    #[error("agent queued-message capacity reached: {limit}")]
    QueueCapacityExceeded { limit: usize },
    #[error("agent lifecycle authority is stale")]
    StaleWorkAuthority,
    #[error("agent request is invalid: {0}")]
    InvalidRequest(String),
    #[error(
        "skill {id:?} is installed at version {installed:?}; changing it to version {requested:?} is unsupported"
    )]
    SkillVersionUpgradeUnsupported {
        id: String,
        installed: String,
        requested: String,
    },
    #[error("agent changed while the operation was being fenced")]
    Conflict,
    #[error("sandbox checkpoint is in progress; input was not admitted")]
    CheckpointInProgress,
    #[error("the previous turn does not match the conversation's latest turn")]
    PreviousTurnConflict,
    #[error("agent log capacity exceeded")]
    LogCapacityExceeded,
    #[error("agent data is not usable: {0}")]
    Corrupt(String),
    #[error("agent runtime is unavailable: {0}")]
    RuntimeUnavailable(String),
    #[error("agent operation delivery is ambiguous at fence {operation_sequence}: {detail}")]
    DeliveryAmbiguous {
        operation_sequence: u64,
        detail: String,
    },
    #[error("agent operation outcome could not be made durable: {0}")]
    OutcomeUnavailable(String),
    #[error("agent operation store is unavailable: {0}")]
    Unavailable(String),
}

impl AgentOperationError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Budget(reason) => reason.code(),
            Self::Connection(error) => error.code(),
            Self::Forbidden => "forbidden",
            Self::NotFound => "agent_not_found",
            Self::StatusConflict { .. } => "agent_status_conflict",
            Self::NoActiveTurn => "agent_turn_not_active",
            Self::ApprovalNotPending { .. } => "agent_approval_not_pending",
            Self::FirstMessageReserved { .. } => "agent_first_message_reserved",
            Self::DispatchInProgress { .. } => "agent_dispatch_in_progress",
            Self::DispatchRecoveryRequired { .. } => "agent_dispatch_recovery_required",
            Self::CapabilityUnsupported { .. } => "agent_capability_unsupported",
            Self::ContentUnsupported => "agent_content_unsupported",
            Self::QueueCapacityExceeded { .. } => "agent_message_queue_full",
            Self::StaleWorkAuthority => "agent_work_authority_stale",
            Self::InvalidRequest(_) => "invalid_agent_operation",
            Self::SkillVersionUpgradeUnsupported { .. } => {
                "agent_skill_version_upgrade_unsupported"
            }
            Self::Conflict => "agent_operation_conflict",
            Self::CheckpointInProgress => "sandbox_checkpoint_in_progress",
            Self::PreviousTurnConflict => "agent_previous_turn_conflict",
            Self::LogCapacityExceeded => "agent_log_capacity_exceeded",
            Self::Corrupt(_) => "stored_agent_corrupt",
            Self::RuntimeUnavailable(_) => "agent_runtime_unavailable",
            Self::DeliveryAmbiguous { .. } => "agent_delivery_ambiguous",
            Self::OutcomeUnavailable(_) => "agent_operation_outcome_unavailable",
            Self::Unavailable(_) => "agent_operation_unavailable",
        }
    }
}

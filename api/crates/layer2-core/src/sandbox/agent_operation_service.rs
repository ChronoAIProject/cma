use std::sync::Arc;

use crate::{AgentId, Caller, Capability, RuntimeCapability};

use super::store::{AgentConversationStore, AgentStore, SandboxStoreError};
use super::{
    Agent, AgentApprovalDecision, AgentOperationDelivery, AgentRuntimeRegistry, AgentStatus,
    FirstMessageDelivery,
};

pub(super) const MAX_OPERATION_COMMIT_RETRIES: usize = 8;

pub trait AgentOperationStore: AgentStore + AgentConversationStore {}

impl<T> AgentOperationStore for T where T: AgentStore + AgentConversationStore {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentOperationReceipt {
    pub agent: Agent,
    pub operation_sequence: u64,
    pub turn_sequence: u64,
    pub delivery: AgentOperationDelivery,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentMessageOutcome {
    Dispatched(AgentOperationReceipt),
    Queued { agent: Agent, sequence: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AgentOperationError {
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
    #[error("agent queued-message capacity reached: {limit}")]
    QueueCapacityExceeded { limit: usize },
    #[error("agent lifecycle authority is stale")]
    StaleWorkAuthority,
    #[error("agent request is invalid: {0}")]
    InvalidRequest(String),
    #[error("agent changed while the operation was being fenced")]
    Conflict,
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
            Self::Forbidden => "forbidden",
            Self::NotFound => "agent_not_found",
            Self::StatusConflict { .. } => "agent_status_conflict",
            Self::NoActiveTurn => "agent_turn_not_active",
            Self::ApprovalNotPending { .. } => "agent_approval_not_pending",
            Self::FirstMessageReserved { .. } => "agent_first_message_reserved",
            Self::DispatchInProgress { .. } => "agent_dispatch_in_progress",
            Self::DispatchRecoveryRequired { .. } => "agent_dispatch_recovery_required",
            Self::CapabilityUnsupported { .. } => "agent_capability_unsupported",
            Self::QueueCapacityExceeded { .. } => "agent_message_queue_full",
            Self::StaleWorkAuthority => "agent_work_authority_stale",
            Self::InvalidRequest(_) => "invalid_agent_operation",
            Self::Conflict => "agent_operation_conflict",
            Self::LogCapacityExceeded => "agent_log_capacity_exceeded",
            Self::Corrupt(_) => "stored_agent_corrupt",
            Self::RuntimeUnavailable(_) => "agent_runtime_unavailable",
            Self::DeliveryAmbiguous { .. } => "agent_delivery_ambiguous",
            Self::OutcomeUnavailable(_) => "agent_operation_outcome_unavailable",
            Self::Unavailable(_) => "agent_operation_unavailable",
        }
    }
}

pub struct AgentOperationService {
    pub(super) store: Arc<dyn AgentOperationStore>,
    pub(super) runtimes: Arc<dyn AgentRuntimeRegistry>,
}

impl AgentOperationService {
    pub fn new<S, R>(store: Arc<S>, runtimes: Arc<R>) -> Arc<Self>
    where
        S: AgentOperationStore + 'static,
        R: AgentRuntimeRegistry + 'static,
    {
        Arc::new(Self { store, runtimes })
    }

    pub async fn send_message(
        &self,
        caller: &Caller,
        id: &AgentId,
        text: String,
    ) -> Result<AgentMessageOutcome, AgentOperationError> {
        self.require_owner_authority(caller)?;
        self.validate_message(&text)?;
        for _ in 0..MAX_OPERATION_COMMIT_RETRIES {
            let current = self.owned_agent(caller, id).await?;
            self.require_first_message_settled(&current)?;
            self.require_dispatch_available(&current)?;
            match current.status {
                AgentStatus::Ready => match self.fence_message(current, text.clone()).await {
                    Ok(prepared) => {
                        return self
                            .dispatch_prepared(prepared)
                            .await
                            .map(AgentMessageOutcome::Dispatched)
                    }
                    Err(AgentOperationError::Conflict) => continue,
                    Err(error) => return Err(error),
                },
                AgentStatus::Paused => match self.queue_paused_message(current, text.clone()).await
                {
                    Ok(outcome) => return Ok(outcome),
                    Err(AgentOperationError::Conflict) => continue,
                    Err(error) => return Err(error),
                },
                status => return Err(AgentOperationError::StatusConflict { status }),
            }
        }
        Err(AgentOperationError::Conflict)
    }

    pub async fn steer(
        &self,
        caller: &Caller,
        id: &AgentId,
        text: String,
    ) -> Result<AgentOperationReceipt, AgentOperationError> {
        self.require_owner_authority(caller)?;
        self.validate_message(&text)?;
        for _ in 0..MAX_OPERATION_COMMIT_RETRIES {
            let current = self.owned_agent(caller, id).await?;
            self.require_active_status(&current, &[AgentStatus::Working])?;
            self.require_first_message_dispatch_settled(&current)?;
            self.require_dispatch_available(&current)?;
            match self.fence_steer(current, text.clone()).await {
                Ok(prepared) => return self.dispatch_prepared(prepared).await,
                Err(AgentOperationError::Conflict) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(AgentOperationError::Conflict)
    }

    pub async fn interrupt(
        &self,
        caller: &Caller,
        id: &AgentId,
    ) -> Result<AgentOperationReceipt, AgentOperationError> {
        self.require_owner_authority(caller)?;
        for _ in 0..MAX_OPERATION_COMMIT_RETRIES {
            let current = self.owned_agent(caller, id).await?;
            self.require_active_status(
                &current,
                &[AgentStatus::Working, AgentStatus::WaitingApproval],
            )?;
            self.require_first_message_dispatch_settled(&current)?;
            self.require_dispatch_available(&current)?;
            match self.fence_interrupt(current).await {
                Ok(prepared) => return self.dispatch_prepared(prepared).await,
                Err(AgentOperationError::Conflict) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(AgentOperationError::Conflict)
    }

    pub async fn answer_approval(
        &self,
        caller: &Caller,
        id: &AgentId,
        approval_id: String,
        decision: AgentApprovalDecision,
    ) -> Result<AgentOperationReceipt, AgentOperationError> {
        self.require_owner_authority(caller)?;
        self.validate_approval_id(&approval_id)?;
        for _ in 0..MAX_OPERATION_COMMIT_RETRIES {
            let current = self.owned_agent(caller, id).await?;
            self.require_active_status(&current, &[AgentStatus::WaitingApproval])?;
            self.require_first_message_dispatch_settled(&current)?;
            self.require_dispatch_available(&current)?;
            match self
                .fence_approval(current, approval_id.clone(), decision)
                .await
            {
                Ok(prepared) => return self.dispatch_prepared(prepared).await,
                Err(AgentOperationError::Conflict) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(AgentOperationError::Conflict)
    }

    fn require_owner_authority(&self, caller: &Caller) -> Result<(), AgentOperationError> {
        caller
            .authorization
            .allows(Capability::UseOwnSandboxes)
            .then_some(())
            .ok_or(AgentOperationError::Forbidden)
    }

    async fn owned_agent(
        &self,
        caller: &Caller,
        id: &AgentId,
    ) -> Result<Agent, AgentOperationError> {
        AgentStore::get_agent(self.store.as_ref(), id)
            .await
            .map_err(operation_store_error)?
            .filter(|agent| agent.id == *id && agent.owner == caller.subject)
            .ok_or(AgentOperationError::NotFound)
    }

    fn require_active_status(
        &self,
        agent: &Agent,
        statuses: &[AgentStatus],
    ) -> Result<u64, AgentOperationError> {
        if !statuses.contains(&agent.status) {
            return Err(AgentOperationError::StatusConflict {
                status: agent.status,
            });
        }
        agent
            .conversation
            .active_turn
            .filter(|turn| *turn > 0)
            .ok_or(AgentOperationError::NoActiveTurn)
    }

    fn require_first_message_settled(&self, agent: &Agent) -> Result<(), AgentOperationError> {
        if let Some(record) = agent
            .conversation
            .first_message
            .as_ref()
            .filter(|record| !record.is_terminal())
        {
            return Err(AgentOperationError::FirstMessageReserved {
                state: record.state,
            });
        }
        Ok(())
    }

    fn require_first_message_dispatch_settled(
        &self,
        agent: &Agent,
    ) -> Result<(), AgentOperationError> {
        if let Some(record) = agent.conversation.first_message.as_ref().filter(|record| {
            matches!(
                record.state,
                FirstMessageDelivery::Queued
                    | FirstMessageDelivery::DispatchFenced
                    | FirstMessageDelivery::RecoveryRequired
            )
        }) {
            return Err(AgentOperationError::FirstMessageReserved {
                state: record.state,
            });
        }
        Ok(())
    }

    fn require_dispatch_available(&self, agent: &Agent) -> Result<(), AgentOperationError> {
        match agent.conversation.operation.as_ref() {
            Some(record) if record.state == AgentOperationDelivery::DispatchFenced => {
                Err(AgentOperationError::DispatchInProgress {
                    operation_sequence: record.sequence,
                })
            }
            Some(record) if record.state == AgentOperationDelivery::RecoveryRequired => {
                Err(AgentOperationError::DispatchRecoveryRequired {
                    operation_sequence: record.sequence,
                })
            }
            _ => Ok(()),
        }
    }
}

pub(super) fn operation_store_error(error: SandboxStoreError) -> AgentOperationError {
    match error {
        SandboxStoreError::NotFound => AgentOperationError::NotFound,
        SandboxStoreError::Conflict => AgentOperationError::Conflict,
        SandboxStoreError::SequenceConflict => {
            AgentOperationError::Corrupt("agent log sequence is inconsistent".into())
        }
        SandboxStoreError::AgentLogCapacityExceeded => AgentOperationError::LogCapacityExceeded,
        SandboxStoreError::Corrupt(detail) => AgentOperationError::Corrupt(detail),
        other => AgentOperationError::Unavailable(other.to_string()),
    }
}

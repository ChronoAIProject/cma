use std::sync::Arc;

use crate::{AgentId, Caller, RuntimeCapability};

use super::store::{
    AgentConversationStore, AgentLogStore, AgentStore, SandboxLifecycleExecutionStore,
    SandboxMutationGuard, SandboxStore, SandboxStoreError,
};
use super::{
    Agent, AgentApprovalDecision, AgentOperationDelivery, AgentRuntimeRegistry, AgentStatus,
    FirstMessageDelivery, SandboxImageRotationPhase, SandboxLifecycleOperation, SandboxRevivePhase,
    SandboxStatus,
};

pub(super) const MAX_OPERATION_COMMIT_RETRIES: usize = 8;

#[path = "agent_linear_input.rs"]
mod linear_input;

#[path = "agent_operation_authority.rs"]
mod execution_authority;

pub trait AgentOperationStore:
    AgentStore + AgentLogStore + AgentConversationStore + SandboxStore + SandboxLifecycleExecutionStore
{
}

impl<T> AgentOperationStore for T where
    T: AgentStore
        + AgentLogStore
        + AgentConversationStore
        + SandboxStore
        + SandboxLifecycleExecutionStore
{
}

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
    #[error(transparent)]
    Connection(#[from] super::SandboxCredentialPrerequisiteError),
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

pub struct AgentOperationService {
    pub(super) instruction_policy: Arc<dyn super::AgentInstructionPolicy>,
    pub(super) settings_authority: Option<super::AgentSettingsAuthorization>,
    pub(super) command_claim: Option<super::agent_io::CommandClaim>,
    pub(super) execution_authority: Option<super::agent_io::CommandExecutionAuthorization>,
    pub(super) trigger_authority: Option<super::agent_io::TriggerExecutionAuthorization>,
    pub(super) connections: Arc<dyn super::SandboxCredentialPrerequisite>,
    pub(super) store: Arc<dyn AgentOperationStore>,
    pub(super) runtimes: Arc<dyn AgentRuntimeRegistry>,
}

impl AgentOperationService {
    pub fn new<S, R>(store: Arc<S>, runtimes: Arc<R>) -> Arc<Self>
    where
        S: AgentOperationStore + 'static,
        R: AgentRuntimeRegistry + 'static,
    {
        Arc::new(Self {
            instruction_policy: Arc::new(super::agent_preferences::UnmanagedInstructions),
            settings_authority: None,
            command_claim: None,
            execution_authority: None,
            trigger_authority: None,
            store,
            runtimes,
            connections: Arc::new(super::FixedSandboxCredentialPrerequisite::Ready),
        })
    }

    pub fn with_connections(
        &self,
        connections: Arc<dyn super::SandboxCredentialPrerequisite>,
    ) -> Arc<Self> {
        Arc::new(Self {
            instruction_policy: self.instruction_policy.clone(),
            settings_authority: self.settings_authority.clone(),
            command_claim: self.command_claim.clone(),
            execution_authority: self.execution_authority.clone(),
            trigger_authority: self.trigger_authority.clone(),
            connections,
            store: self.store.clone(),
            runtimes: self.runtimes.clone(),
        })
    }

    pub(super) async fn require_agent_connections(
        &self,
        agent: &Agent,
    ) -> Result<(), AgentOperationError> {
        let sandbox = SandboxStore::get_sandbox(self.store.as_ref(), &agent.sandbox_id)
            .await
            .map_err(operation_store_error)?
            .filter(|sandbox| sandbox.owner == agent.owner)
            .ok_or_else(|| {
                AgentOperationError::Corrupt("agent sandbox ownership is inconsistent".into())
            })?;
        self.connections.check_sandbox(&sandbox).await?;
        Ok(())
    }

    pub(super) async fn execute_message(
        &self,
        caller: &Caller,
        id: &AgentId,
        text: String,
        metadata: std::collections::BTreeMap<String, String>,
    ) -> Result<AgentMessageOutcome, AgentOperationError> {
        self.require_owner_authority(caller)?;
        self.validate_message(&text)?;
        for _ in 0..MAX_OPERATION_COMMIT_RETRIES {
            let current = self.owned_agent(caller, id).await?;
            require_granted(&current, [RuntimeCapability::Conversation])?;
            self.require_first_message_settled(&current)?;
            self.require_dispatch_available(&current)?;
            if !matches!(current.status, AgentStatus::Ready | AgentStatus::Paused) {
                return Err(AgentOperationError::StatusConflict {
                    status: current.status,
                });
            }
            self.require_agent_connections(&current).await?;
            let (parent, queues) = self.parent_admission(&current).await?;
            // Existing queued records have no metadata field. Do not accept
            // metadata that a retained reader or lifecycle worker would lose.
            if (queues || current.status == AgentStatus::Paused) && !metadata.is_empty() {
                return Err(AgentOperationError::InvalidRequest(
                    "queued message metadata is not supported".into(),
                ));
            }
            match current.status {
                AgentStatus::Ready if queues => {
                    match self.queue_message(current, text.clone(), parent).await {
                        Ok(outcome) => return Ok(outcome),
                        Err(AgentOperationError::Conflict) => continue,
                        Err(error) => return Err(error),
                    }
                }
                AgentStatus::Ready => match self
                    .fence_message(current, text.clone(), parent, metadata.clone())
                    .await
                {
                    Ok(prepared) => {
                        return self
                            .dispatch_prepared(prepared)
                            .await
                            .map(AgentMessageOutcome::Dispatched);
                    }
                    Err(AgentOperationError::Conflict) => continue,
                    Err(error) => return Err(error),
                },
                AgentStatus::Paused => {
                    match self.queue_message(current, text.clone(), parent).await {
                        Ok(outcome) => return Ok(outcome),
                        Err(AgentOperationError::Conflict) => continue,
                        Err(error) => return Err(error),
                    }
                }
                status => return Err(AgentOperationError::StatusConflict { status }),
            }
        }
        Err(AgentOperationError::Conflict)
    }

    pub(super) async fn parent_admission(
        &self,
        agent: &Agent,
    ) -> Result<(SandboxMutationGuard, bool), AgentOperationError> {
        let sandbox = SandboxStore::get_sandbox(self.store.as_ref(), &agent.sandbox_id)
            .await
            .map_err(operation_store_error)?
            .ok_or_else(|| AgentOperationError::Corrupt("agent parent sandbox is absent".into()))?;
        if sandbox.id != agent.sandbox_id || sandbox.owner != agent.owner {
            return Err(AgentOperationError::Corrupt(
                "agent parent sandbox ownership is inconsistent".into(),
            ));
        }
        if sandbox.lifecycle.checkpoint_capture.is_some() {
            return Err(AgentOperationError::CheckpointInProgress);
        }
        // A common command remains in its durable queue until the parent has
        // published Active. PublishActive is still a lifecycle phase; fencing
        // here would make the ordered writer refuse the same pending command.
        if self.command_claim.is_some()
            && !matches!(
                sandbox.status,
                SandboxStatus::Active | SandboxStatus::CredentialRotating
            )
        {
            return Err(AgentOperationError::StatusConflict {
                status: agent.status,
            });
        }
        let queues = match sandbox.status {
            SandboxStatus::Pausing | SandboxStatus::Paused => Ok(true),
            SandboxStatus::Reviving => match sandbox.lifecycle.operation.as_ref() {
                Some(SandboxLifecycleOperation::Revive { phase, .. }) => {
                    Ok(*phase != SandboxRevivePhase::PublishActive)
                }
                _ => Err(AgentOperationError::Corrupt(
                    "reviving sandbox lifecycle operation is inconsistent".into(),
                )),
            },
            SandboxStatus::Rotating => match sandbox.lifecycle.operation.as_ref() {
                Some(SandboxLifecycleOperation::ImageRotation { phase, .. }) => {
                    Ok(*phase != SandboxImageRotationPhase::PublishActive)
                }
                _ => Err(AgentOperationError::Corrupt(
                    "rotating sandbox lifecycle operation is inconsistent".into(),
                )),
            },
            SandboxStatus::Active | SandboxStatus::CredentialRotating => Ok(false),
            _ => Err(AgentOperationError::StatusConflict {
                status: agent.status,
            }),
        }?;
        Ok((SandboxMutationGuard::capture(&sandbox), queues))
    }

    pub(super) async fn execute_steer(
        &self,
        caller: &Caller,
        id: &AgentId,
        text: String,
    ) -> Result<AgentOperationReceipt, AgentOperationError> {
        self.require_owner_authority(caller)?;
        self.validate_message(&text)?;
        for _ in 0..MAX_OPERATION_COMMIT_RETRIES {
            let current = self.owned_agent(caller, id).await?;
            require_granted(
                &current,
                [RuntimeCapability::Conversation, RuntimeCapability::Steer],
            )?;
            self.require_active_status(&current, &[AgentStatus::Working])?;
            self.require_first_message_dispatch_settled(&current)?;
            self.require_dispatch_available(&current)?;
            let (parent, _) = self.parent_admission(&current).await?;
            match self.fence_steer(current, text.clone(), parent).await {
                Ok(prepared) => return self.dispatch_prepared(prepared).await,
                Err(AgentOperationError::Conflict) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(AgentOperationError::Conflict)
    }

    pub(super) async fn execute_interrupt(
        &self,
        caller: &Caller,
        id: &AgentId,
    ) -> Result<AgentOperationReceipt, AgentOperationError> {
        self.require_owner_authority(caller)?;
        for _ in 0..MAX_OPERATION_COMMIT_RETRIES {
            let current = self.owned_agent(caller, id).await?;
            require_granted(
                &current,
                [
                    RuntimeCapability::Conversation,
                    RuntimeCapability::Interrupt,
                ],
            )?;
            self.require_active_status(
                &current,
                &[AgentStatus::Working, AgentStatus::WaitingApproval],
            )?;
            self.require_first_message_dispatch_settled(&current)?;
            self.require_dispatch_available(&current)?;
            let (parent, _) = self.parent_admission(&current).await?;
            match self.fence_interrupt(current, parent).await {
                Ok(prepared) => return self.dispatch_prepared(prepared).await,
                Err(AgentOperationError::Conflict) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(AgentOperationError::Conflict)
    }

    pub(super) async fn execute_approval(
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
            require_granted(&current, [RuntimeCapability::Conversation])?;
            self.require_active_status(&current, &[AgentStatus::WaitingApproval])?;
            self.require_first_message_dispatch_settled(&current)?;
            self.require_dispatch_available(&current)?;
            let (parent, _) = self.parent_admission(&current).await?;
            match self
                .fence_approval(current, approval_id.clone(), decision, parent)
                .await
            {
                Ok(prepared) => return self.dispatch_prepared(prepared).await,
                Err(AgentOperationError::Conflict) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(AgentOperationError::Conflict)
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

    pub(super) fn require_first_message_settled(
        &self,
        agent: &Agent,
    ) -> Result<(), AgentOperationError> {
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

    pub(super) fn require_first_message_dispatch_settled(
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

    pub(super) fn require_dispatch_available(
        &self,
        agent: &Agent,
    ) -> Result<(), AgentOperationError> {
        if agent.work.step == super::AgentWorkStep::InstallSkill {
            return Err(AgentOperationError::StatusConflict {
                status: agent.status,
            });
        }
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

fn require_granted(
    agent: &Agent,
    required: impl IntoIterator<Item = RuntimeCapability>,
) -> Result<(), AgentOperationError> {
    agent
        .capabilities
        .require(required)
        .map_err(|capability| AgentOperationError::CapabilityUnsupported { capability })
}

pub(super) fn operation_store_error(error: SandboxStoreError) -> AgentOperationError {
    match error {
        SandboxStoreError::NotFound => AgentOperationError::NotFound,
        SandboxStoreError::Conflict => AgentOperationError::Conflict,
        SandboxStoreError::CheckpointInProgress => AgentOperationError::CheckpointInProgress,
        SandboxStoreError::SequenceConflict => {
            AgentOperationError::Corrupt("agent log sequence is inconsistent".into())
        }
        SandboxStoreError::AgentLogCapacityExceeded => AgentOperationError::LogCapacityExceeded,
        SandboxStoreError::Corrupt(detail) => AgentOperationError::Corrupt(detail),
        other => AgentOperationError::Unavailable(other.to_string()),
    }
}

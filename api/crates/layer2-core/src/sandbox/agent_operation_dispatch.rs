use layer3_adapter::{ContentBlock, ConversationInput, PermissionDecision, PermissionLocator};

use crate::{now_rfc3339, RuntimeCapability};

use super::agent_operation_service::operation_store_error;
use super::store::{AgentConversationStore, SandboxMutationGuard};
use super::{
    Agent, AgentLogKind, AgentMessageOutcome, AgentOperationError, AgentOperationIntent,
    AgentOperationService, AgentRuntimeTarget, AgentStatus, QueuedAgentMessage,
    MAX_AGENT_MESSAGE_BYTES, MAX_QUEUED_AGENT_MESSAGES,
};

#[derive(Clone)]
pub(super) struct ApprovalSnapshot {
    pub(super) approval_id: String,
    pub(super) action: Option<String>,
    pub(super) detail: Option<String>,
    pub(super) runtime_locator: Option<String>,
}

#[derive(Clone)]
pub(super) enum PreWriteRollback {
    Settings(Option<super::AgentSettings>),
    NewTurn,
    Approval(ApprovalSnapshot),
    WaitingApproval,
    None,
}

pub(super) struct PreparedAgentOperation {
    pub(super) target: AgentRuntimeTarget,
    pub(super) operation_sequence: u64,
    pub(super) turn_sequence: u64,
    pub(super) required: Vec<RuntimeCapability>,
    pub(super) input: ConversationInput,
    pub(super) rollback: PreWriteRollback,
    pub(super) queued_message_sequence: Option<u64>,
}

impl AgentOperationService {
    pub(super) fn validate_message(&self, text: &str) -> Result<(), AgentOperationError> {
        if text.trim().is_empty() || text.len() > MAX_AGENT_MESSAGE_BYTES {
            return Err(AgentOperationError::InvalidRequest(format!(
                "message must contain between 1 and {MAX_AGENT_MESSAGE_BYTES} bytes"
            )));
        }
        Ok(())
    }

    pub(super) fn validate_approval_id(
        &self,
        approval_id: &str,
    ) -> Result<(), AgentOperationError> {
        super::AgentOperationIntent::answer_approval(
            approval_id.to_owned(),
            super::AgentApprovalDecision::Deny,
        )
        .map(|_| ())
        .map_err(|error| AgentOperationError::InvalidRequest(error.to_string()))
    }

    pub(super) async fn queue_message(
        &self,
        current: Agent,
        text: String,
        parent: SandboxMutationGuard,
    ) -> Result<AgentMessageOutcome, AgentOperationError> {
        if self.command_claim.is_some() {
            return Err(AgentOperationError::StatusConflict {
                status: current.status,
            });
        }
        if current.conversation.queued_messages.len() >= MAX_QUEUED_AGENT_MESSAGES {
            return Err(AgentOperationError::QueueCapacityExceeded {
                limit: MAX_QUEUED_AGENT_MESSAGES,
            });
        }
        let at = now_rfc3339();
        let sequence = next_sequence(current.conversation.last_seq)?;
        let message = QueuedAgentMessage::new(sequence, text.clone(), at.clone())
            .map_err(|error| AgentOperationError::InvalidRequest(error.to_string()))?;
        let mut next = current.clone();
        next.conversation.queued_messages.push(message);
        let mut logs = vec![AgentLogKind::OwnerMessage {
            content: text.clone(),
        }];
        logs.extend(
            next.conversation
                .observations
                .metadata
                .record_task_preview(&text),
        );
        let agent = self
            .commit_admission(&current, next, logs, at, parent)
            .await?
            .0;
        Ok(AgentMessageOutcome::Queued { agent, sequence })
    }

    pub(super) async fn fence_message(
        &self,
        current: Agent,
        text: String,
        parent: SandboxMutationGuard,
        metadata: std::collections::BTreeMap<String, String>,
        task_acceptance: Option<&super::task_acceptance::TaskAcceptanceIntent>,
    ) -> Result<PreparedAgentOperation, AgentOperationError> {
        self.check_model_funding(&current).await?;
        if current.conversation.active_turn.is_some() {
            return Err(AgentOperationError::Corrupt(
                "ready agent already has an active turn".into(),
            ));
        }
        let target = runtime_target(&current)?;
        let required = vec![RuntimeCapability::Conversation];
        self.require_runtime(&target, &required).await?;
        let at = now_rfc3339();
        let operation_sequence = next_sequence(current.conversation.last_seq)?;
        let turn_sequence = operation_sequence
            .checked_add(1)
            .ok_or_else(|| AgentOperationError::Corrupt("agent turn sequence overflow".into()))?;
        let intent = AgentOperationIntent::send_message(&text);
        let mut next = current.clone();
        next.conversation.operation = Some(new_operation(
            operation_sequence,
            &target,
            turn_sequence,
            intent,
            &at,
        )?);
        next.conversation.active_turn = Some(turn_sequence);
        if let Some(intent) = task_acceptance {
            let claim = self
                .command_claim
                .as_ref()
                .ok_or(AgentOperationError::Conflict)?;
            let acceptance = &mut next.conversation.observations.task_acceptance;
            *acceptance = Some(super::task_acceptance::TaskAcceptance::bind(
                acceptance.as_ref(),
                intent,
                &claim.command_id,
                turn_sequence,
            )?);
        }
        next.transition(AgentStatus::Working, at.clone(), None)
            .map_err(|error| AgentOperationError::Corrupt(error.to_string()))?;
        let mut logs = vec![
            operation_log(&next, operation_sequence)?,
            AgentLogKind::TurnStarted {
                model: Some(current.settings().model),
                turn_sequence,
                origin_key: self
                    .command_claim
                    .as_ref()
                    .map(|claim| format!("command:{}", claim.command_id.as_str())),
            },
            AgentLogKind::OwnerMessage {
                content: text.clone(),
            },
            AgentLogKind::StatusChanged {
                from: AgentStatus::Ready,
                to: AgentStatus::Working,
            },
        ];
        if !metadata.is_empty() {
            logs.push(AgentLogKind::TurnMetadataSet {
                turn_sequence,
                metadata,
            });
        }
        logs.extend(
            next.conversation
                .observations
                .metadata
                .record_task_preview(&text),
        );
        if let Some(authority) = &self.trigger_authority {
            super::agent_io::trigger::fence_first(
                authority,
                &current,
                &mut next,
                self.command_claim
                    .as_ref()
                    .ok_or(AgentOperationError::Conflict)?
                    .fence,
                &at,
                &mut logs,
            )?;
        }
        self.commit_admission(&current, next, logs, at, parent)
            .await?;
        let prompt_id = layer3_adapter::PromptIdentity::new(
            &target.conversation_id,
            target.runtime_generation,
            turn_sequence,
        );
        Ok(PreparedAgentOperation {
            target,
            operation_sequence,
            turn_sequence,
            required,
            input: ConversationInput::RecoverablePrompt {
                content: vec![ContentBlock::from(text)],
                prompt_id,
            },
            rollback: PreWriteRollback::NewTurn,
            queued_message_sequence: None,
        })
    }

    pub(super) async fn fence_steer(
        &self,
        current: Agent,
        text: String,
        parent: SandboxMutationGuard,
    ) -> Result<PreparedAgentOperation, AgentOperationError> {
        self.require_model_credit()?;
        let turn_sequence = active_turn(&current)?;
        let target = runtime_target(&current)?;
        let required = vec![RuntimeCapability::Conversation, RuntimeCapability::Steer];
        self.require_runtime(&target, &required).await?;
        let at = now_rfc3339();
        let operation_sequence = next_sequence(current.conversation.last_seq)?;
        let mut next = current.clone();
        next.conversation.operation = Some(new_operation(
            operation_sequence,
            &target,
            turn_sequence,
            AgentOperationIntent::steer(&text),
            &at,
        )?);
        let logs = vec![
            operation_log(&next, operation_sequence)?,
            AgentLogKind::OwnerMessage {
                content: text.clone(),
            },
        ];
        self.commit_admission(&current, next, logs, at, parent)
            .await?;
        Ok(PreparedAgentOperation {
            target,
            operation_sequence,
            turn_sequence,
            required,
            input: ConversationInput::Steer(vec![ContentBlock::from(text)]),
            rollback: PreWriteRollback::None,
            queued_message_sequence: None,
        })
    }

    pub(super) async fn fence_interrupt(
        &self,
        current: Agent,
        parent: SandboxMutationGuard,
    ) -> Result<PreparedAgentOperation, AgentOperationError> {
        let turn_sequence = active_turn(&current)?;
        let target = runtime_target(&current)?;
        let required = vec![
            RuntimeCapability::Conversation,
            RuntimeCapability::Interrupt,
        ];
        self.require_runtime(&target, &required).await?;
        let at = now_rfc3339();
        let operation_sequence = next_sequence(current.conversation.last_seq)?;
        let was_waiting = current.status == AgentStatus::WaitingApproval;
        let mut next = current.clone();
        next.conversation.operation = Some(new_operation(
            operation_sequence,
            &target,
            turn_sequence,
            AgentOperationIntent::Interrupt,
            &at,
        )?);
        let mut logs = vec![operation_log(&next, operation_sequence)?];
        if was_waiting {
            next.transition(AgentStatus::Working, at.clone(), None)
                .map_err(|error| AgentOperationError::Corrupt(error.to_string()))?;
            logs.push(AgentLogKind::StatusChanged {
                from: AgentStatus::WaitingApproval,
                to: AgentStatus::Working,
            });
        }
        self.commit_admission(&current, next, logs, at, parent)
            .await?;
        Ok(PreparedAgentOperation {
            target,
            operation_sequence,
            turn_sequence,
            required,
            input: ConversationInput::Cancel,
            rollback: if was_waiting {
                PreWriteRollback::WaitingApproval
            } else {
                PreWriteRollback::None
            },
            queued_message_sequence: None,
        })
    }

    pub(super) async fn fence_approval(
        &self,
        current: Agent,
        approval_id: String,
        decision: super::AgentApprovalDecision,
        parent: SandboxMutationGuard,
    ) -> Result<PreparedAgentOperation, AgentOperationError> {
        if decision != super::AgentApprovalDecision::Abort {
            self.require_model_credit()?;
        }
        let turn_sequence = active_turn(&current)?;
        let pending =
            AgentConversationStore::pending_agent_approvals(self.store.as_ref(), &current.id)
                .await
                .map_err(operation_store_error)?;
        let approvals_remain = pending.len() > 1;
        let snapshot = pending
            .into_iter()
            .find_map(|entry| match entry.kind {
                AgentLogKind::ApprovalRequested {
                    approval_id: found,
                    action,
                    detail,
                    runtime_locator,
                } if found == approval_id => Some(ApprovalSnapshot {
                    approval_id: found,
                    action,
                    detail,
                    runtime_locator,
                }),
                _ => None,
            })
            .ok_or_else(|| AgentOperationError::ApprovalNotPending {
                approval_id: approval_id.clone(),
            })?;
        let target = runtime_target(&current)?;
        let required = vec![RuntimeCapability::Conversation];
        self.require_runtime(&target, &required).await?;
        let at = now_rfc3339();
        let operation_sequence = next_sequence(current.conversation.last_seq)?;
        let intent = AgentOperationIntent::answer_approval(approval_id.clone(), decision)
            .map_err(|error| AgentOperationError::InvalidRequest(error.to_string()))?;
        let mut next = current.clone();
        next.conversation.operation = Some(new_operation(
            operation_sequence,
            &target,
            turn_sequence,
            intent,
            &at,
        )?);
        let mut logs = vec![
            operation_log(&next, operation_sequence)?,
            AgentLogKind::ApprovalResolved {
                approval_id: approval_id.clone(),
                decision: decision.as_str().into(),
            },
        ];
        if !approvals_remain {
            next.transition(AgentStatus::Working, at.clone(), None)
                .map_err(|error| AgentOperationError::Corrupt(error.to_string()))?;
            logs.push(AgentLogKind::StatusChanged {
                from: AgentStatus::WaitingApproval,
                to: AgentStatus::Working,
            });
        }
        self.commit_admission(&current, next, logs, at, parent)
            .await?;
        let locator = snapshot
            .runtime_locator
            .as_ref()
            .map(|value| PermissionLocator::new(value.clone()));
        Ok(PreparedAgentOperation {
            target,
            operation_sequence,
            turn_sequence,
            required,
            input: ConversationInput::PermissionAnswer {
                id: approval_id,
                locator,
                decision: runtime_decision(decision),
            },
            rollback: PreWriteRollback::Approval(snapshot),
            queued_message_sequence: None,
        })
    }

    pub(super) async fn require_runtime(
        &self,
        target: &AgentRuntimeTarget,
        required: &[RuntimeCapability],
    ) -> Result<(), AgentOperationError> {
        let connection = match self.runtimes.resolve(target).await {
            Ok(connection) => connection,
            Err(error) => {
                if self.command_claim.is_some()
                    && matches!(
                        error,
                        super::AgentRuntimeError::NotFound
                            | super::AgentRuntimeError::StaleGeneration
                    )
                {
                    self.runtimes.recover_missing_attachment(target).await;
                }
                return Err(runtime_error(error));
            }
        };
        connection
            .capabilities
            .require(required.iter().copied())
            .map_err(|capability| AgentOperationError::CapabilityUnsupported { capability })
    }
}

pub(super) fn runtime_target(agent: &Agent) -> Result<AgentRuntimeTarget, AgentOperationError> {
    let conversation_id = agent
        .conversation
        .conversation_id
        .clone()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| AgentOperationError::Corrupt("conversation identity is absent".into()))?;
    let runtime_generation = agent
        .conversation
        .runtime_generation
        .filter(|generation| *generation > 0)
        .ok_or_else(|| AgentOperationError::Corrupt("runtime generation is absent".into()))?;
    Ok(AgentRuntimeTarget {
        agent_id: agent.id.clone(),
        conversation_id,
        runtime_generation,
    })
}

pub(super) fn new_operation(
    sequence: u64,
    target: &AgentRuntimeTarget,
    turn_sequence: u64,
    intent: AgentOperationIntent,
    at: &str,
) -> Result<super::AgentOperationRecord, AgentOperationError> {
    super::AgentOperationRecord::new(
        sequence,
        target.runtime_generation,
        turn_sequence,
        intent,
        at.to_owned(),
    )
    .map_err(|error| AgentOperationError::Corrupt(error.to_string()))
}

pub(super) fn operation_log(
    agent: &Agent,
    sequence: u64,
) -> Result<AgentLogKind, AgentOperationError> {
    let operation = agent
        .conversation
        .operation
        .as_ref()
        .filter(|operation| operation.sequence == sequence)
        .ok_or_else(|| AgentOperationError::Corrupt("operation fence is absent".into()))?;
    Ok(AgentLogKind::OperationDeliveryChanged {
        operation_sequence: sequence,
        operation: operation.intent.kind(),
        state: operation.state,
    })
}

fn active_turn(agent: &Agent) -> Result<u64, AgentOperationError> {
    agent
        .conversation
        .active_turn
        .filter(|turn| *turn > 0)
        .ok_or(AgentOperationError::NoActiveTurn)
}

pub(super) fn next_sequence(current: u64) -> Result<u64, AgentOperationError> {
    current
        .checked_add(1)
        .ok_or_else(|| AgentOperationError::Corrupt("agent sequence overflow".into()))
}

fn runtime_decision(decision: super::AgentApprovalDecision) -> PermissionDecision {
    match decision {
        super::AgentApprovalDecision::Allow => PermissionDecision::Allow,
        super::AgentApprovalDecision::Deny => PermissionDecision::Deny,
        super::AgentApprovalDecision::Abort => PermissionDecision::Abort,
    }
}

fn runtime_error(error: super::AgentRuntimeError) -> AgentOperationError {
    AgentOperationError::RuntimeUnavailable(error.to_string())
}

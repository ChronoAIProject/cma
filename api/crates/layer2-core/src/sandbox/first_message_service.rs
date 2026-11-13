use crate::{now_rfc3339, AgentId, Caller, Capability, RuntimeCapability};

use super::agent_operation_service::{operation_store_error, MAX_OPERATION_COMMIT_RETRIES};
use super::first_message_dispatch::PreparedFirstMessage;
use super::store::AgentStore;
use super::{
    Agent, AgentLogKind, AgentOperationDelivery, AgentOperationError, AgentOperationService,
    AgentRuntimeTarget, AgentStatus, AgentWorkStep, FirstMessageDelivery, FirstMessageFailure,
    FirstMessageRecord,
};

#[path = "first_message_budget.rs"]
mod budget;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentWorkAuthority {
    pub holder: String,
    pub fence: u64,
}

impl AgentWorkAuthority {
    pub fn new(holder: String, fence: u64) -> Result<Self, AgentOperationError> {
        if holder.trim().is_empty() || fence == 0 {
            return Err(AgentOperationError::InvalidRequest(
                "agent work authority is invalid".into(),
            ));
        }
        Ok(Self { holder, fence })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FirstMessageDispatchOutcome {
    NotConfigured,
    CommonCommand(FirstMessageRecord),
    Accepted(FirstMessageRecord),
    Retryable(FirstMessageRecord),
    InProgress(FirstMessageRecord),
    RecoveryRequired(FirstMessageRecord),
}

impl AgentOperationService {
    pub async fn dispatch_queued_first_message(
        &self,
        id: &AgentId,
        authority: AgentWorkAuthority,
    ) -> Result<FirstMessageDispatchOutcome, AgentOperationError> {
        for _ in 0..MAX_OPERATION_COMMIT_RETRIES {
            let current = AgentStore::get_agent(self.store.as_ref(), id)
                .await
                .map_err(operation_store_error)?
                .ok_or(AgentOperationError::NotFound)?;
            require_work_authority(&current, &authority)?;
            current
                .capabilities
                .require([RuntimeCapability::Conversation])
                .map_err(|capability| AgentOperationError::CapabilityUnsupported { capability })?;
            let Some(record) = current.conversation.first_message.as_ref() else {
                return Ok(FirstMessageDispatchOutcome::NotConfigured);
            };
            if let Some(binding) = &record.common_binding {
                binding
                    .require_agent(&current)
                    .map_err(|_| AgentOperationError::Conflict)?;
                return Ok(FirstMessageDispatchOutcome::CommonCommand(record.clone()));
            }
            self.store
                .require_legacy_first_message(&current)
                .await
                .map_err(operation_store_error)?;
            match record.state {
                FirstMessageDelivery::DispatchFenced
                    if record.dispatch_generation == authority.fence =>
                {
                    return Ok(FirstMessageDispatchOutcome::InProgress(record.clone()));
                }
                FirstMessageDelivery::DispatchFenced => {
                    match self
                        .fence_abandoned_first_message(&current, &authority)
                        .await
                    {
                        Ok(record) => {
                            return Ok(FirstMessageDispatchOutcome::RecoveryRequired(record));
                        }
                        Err(AgentOperationError::Conflict) => continue,
                        Err(error) => return Err(error),
                    }
                }
                FirstMessageDelivery::RecoveryRequired => {
                    return Ok(FirstMessageDispatchOutcome::RecoveryRequired(
                        record.clone(),
                    ));
                }
                FirstMessageDelivery::WriteConfirmed
                | FirstMessageDelivery::ExecutionObserved
                | FirstMessageDelivery::Completed
                | FirstMessageDelivery::Failed => {
                    return Ok(FirstMessageDispatchOutcome::Accepted(record.clone()));
                }
                FirstMessageDelivery::Queued => {}
            }
            match self
                .fence_first_message(current.clone(), authority.clone())
                .await
            {
                Ok(prepared) => return self.send_first_message(prepared).await,
                Err(AgentOperationError::Conflict) => continue,
                Err(AgentOperationError::Budget(reason)) => {
                    match self
                        .refuse_first_message_budget(&current, &authority, reason)
                        .await
                    {
                        Err(AgentOperationError::Conflict) => continue,
                        result => return result,
                    }
                }
                Err(error) => return Err(error),
            }
        }
        Err(AgentOperationError::Conflict)
    }

    pub async fn authorize_first_message_recovery(
        &self,
        caller: &Caller,
        id: &AgentId,
        payload_sha256: &str,
        duplicate_execution_acknowledged: bool,
    ) -> Result<FirstMessageRecord, AgentOperationError> {
        if !caller.authorization.allows(Capability::UseOwnSandboxes) {
            return Err(AgentOperationError::Forbidden);
        }
        for _ in 0..MAX_OPERATION_COMMIT_RETRIES {
            let current = AgentStore::get_agent(self.store.as_ref(), id)
                .await
                .map_err(operation_store_error)?
                .filter(|agent| agent.id == *id && agent.owner == caller.subject)
                .ok_or(AgentOperationError::NotFound)?;
            self.store
                .require_legacy_first_message(&current)
                .await
                .map_err(operation_store_error)?;
            let previous_turn = current
                .conversation
                .first_message
                .as_ref()
                .and_then(|record| record.turn_sequence);
            let (parent, _) = self.parent_admission(&current).await?;
            let mut next = current.clone();
            let record = next.conversation.first_message.as_mut().ok_or_else(|| {
                AgentOperationError::InvalidRequest("agent has no first message to recover".into())
            })?;
            record
                .authorize_recovery(
                    payload_sha256,
                    duplicate_execution_acknowledged,
                    now_rfc3339(),
                )
                .map_err(|error| AgentOperationError::InvalidRequest(error.to_string()))?;
            let recovered = record.clone();
            let at = now_rfc3339();
            let mut logs = vec![first_message_log(&recovered)];
            match current.status {
                AgentStatus::Working => {
                    next.conversation.active_turn = None;
                    next.transition(AgentStatus::Ready, at.clone(), None)
                        .map_err(|error| AgentOperationError::Corrupt(error.to_string()))?;
                    if let Some(turn_sequence) = previous_turn {
                        logs.push(AgentLogKind::TurnInterrupted { turn_sequence });
                    }
                    logs.push(AgentLogKind::StatusChanged {
                        from: AgentStatus::Working,
                        to: AgentStatus::Ready,
                    });
                }
                AgentStatus::Ready => next.conversation.active_turn = None,
                status => return Err(AgentOperationError::StatusConflict { status }),
            }
            match self
                .commit_admission(&current, next, logs, at, parent)
                .await
            {
                Ok(_) => return Ok(recovered),
                Err(AgentOperationError::Conflict) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(AgentOperationError::Conflict)
    }

    async fn fence_first_message(
        &self,
        current: Agent,
        authority: AgentWorkAuthority,
    ) -> Result<PreparedFirstMessage, AgentOperationError> {
        if current.status != AgentStatus::Ready {
            return Err(AgentOperationError::StatusConflict {
                status: current.status,
            });
        }
        if current.conversation.active_turn.is_some() {
            return Err(AgentOperationError::Corrupt(
                "ready agent already has an active turn".into(),
            ));
        }
        if current
            .conversation
            .operation
            .as_ref()
            .is_some_and(|record| record.blocks_dispatch())
        {
            let record = current.conversation.operation.as_ref().expect("checked");
            return if record.state == AgentOperationDelivery::DispatchFenced {
                Err(AgentOperationError::DispatchInProgress {
                    operation_sequence: record.sequence,
                })
            } else {
                Err(AgentOperationError::DispatchRecoveryRequired {
                    operation_sequence: record.sequence,
                })
            };
        }
        let target = runtime_target(&current)?;
        let (parent, _) = self.parent_admission(&current).await?;
        let connection = self
            .runtimes
            .resolve(&target)
            .await
            .map_err(|error| AgentOperationError::RuntimeUnavailable(error.to_string()))?;
        connection
            .capabilities
            .require([RuntimeCapability::Conversation])
            .map_err(|capability| AgentOperationError::CapabilityUnsupported { capability })?;

        let at = now_rfc3339();
        let fence_sequence = next_sequence(current.conversation.last_seq)?;
        let existing_turn = current
            .conversation
            .first_message
            .as_ref()
            .and_then(|record| record.turn_sequence);
        let turn_sequence = existing_turn.unwrap_or(
            fence_sequence
                .checked_add(1)
                .ok_or_else(|| AgentOperationError::Corrupt("agent turn overflow".into()))?,
        );
        let mut next = current.clone();
        let first = next
            .conversation
            .first_message
            .as_mut()
            .ok_or_else(|| AgentOperationError::Corrupt("first message disappeared".into()))?;
        first
            .fence_dispatch_for(turn_sequence, fence_sequence, authority.fence, at.clone())
            .map_err(|error| AgentOperationError::Corrupt(error.to_string()))?;
        let fenced = first.clone();
        next.conversation.active_turn = Some(turn_sequence);
        next.transition(AgentStatus::Working, at.clone(), None)
            .map_err(|error| AgentOperationError::Corrupt(error.to_string()))?;
        let mut logs = vec![first_message_log(&fenced)];
        if existing_turn.is_none() {
            logs.push(AgentLogKind::TurnStarted {
                model: Some(current.settings().model),
                turn_sequence,
                origin_key: Some(fenced.origin_key.clone()),
            });
            logs.push(AgentLogKind::OwnerMessage {
                content: fenced.text.clone(),
            });
        }
        logs.push(AgentLogKind::StatusChanged {
            from: AgentStatus::Ready,
            to: AgentStatus::Working,
        });
        if fenced.purpose == Some(super::FirstMessagePurpose::Task) {
            logs.extend(
                next.conversation
                    .observations
                    .metadata
                    .record_task_preview(&fenced.text),
            );
        }
        self.commit_admission(&current, next, logs, at, parent)
            .await?;
        Ok(PreparedFirstMessage {
            target,
            authority,
            record: fenced,
        })
    }

    async fn fence_abandoned_first_message(
        &self,
        current: &Agent,
        authority: &AgentWorkAuthority,
    ) -> Result<FirstMessageRecord, AgentOperationError> {
        require_work_authority(current, authority)?;
        let at = now_rfc3339();
        let mut next = current.clone();
        let record = next
            .conversation
            .first_message
            .as_mut()
            .ok_or_else(|| AgentOperationError::Corrupt("first message disappeared".into()))?;
        record
            .require_recovery(first_message_failure(
                "first_message_dispatch_abandoned",
                "a prior dispatch ended without durable write evidence",
                false,
                &at,
            )?)
            .map_err(|error| AgentOperationError::Corrupt(error.to_string()))?;
        let recovered = record.clone();
        let logs = vec![first_message_log(&recovered)];
        self.commit_change(current, next, logs, at).await?;
        Ok(recovered)
    }
}

pub(super) fn require_work_authority(
    agent: &Agent,
    authority: &AgentWorkAuthority,
) -> Result<(), AgentOperationError> {
    if agent.work.step != AgentWorkStep::DispatchFirstMessage
        || !agent.work.owns(&authority.holder, authority.fence)
    {
        return Err(AgentOperationError::StaleWorkAuthority);
    }
    Ok(())
}

fn runtime_target(agent: &Agent) -> Result<AgentRuntimeTarget, AgentOperationError> {
    Ok(AgentRuntimeTarget {
        agent_id: agent.id.clone(),
        conversation_id: agent
            .conversation
            .conversation_id
            .clone()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                AgentOperationError::Corrupt("conversation identity is absent".into())
            })?,
        runtime_generation: agent
            .conversation
            .runtime_generation
            .filter(|generation| *generation > 0)
            .ok_or_else(|| AgentOperationError::Corrupt("runtime generation is absent".into()))?,
    })
}

pub(super) fn first_message_log(record: &FirstMessageRecord) -> AgentLogKind {
    AgentLogKind::FirstMessageChanged {
        state: record.state,
        recovery_generation: record.recovery_generation,
    }
}

pub(super) fn first_message_failure(
    code: &str,
    detail: &str,
    retryable: bool,
    at: &str,
) -> Result<FirstMessageFailure, AgentOperationError> {
    FirstMessageFailure::new(code, bounded_detail(detail), retryable, at)
        .map_err(|error| AgentOperationError::Corrupt(error.to_string()))
}

fn bounded_detail(value: &str) -> String {
    let mut value = value.to_owned();
    if value.len() <= 4 * 1024 {
        return value;
    }
    let mut end = 4 * 1024;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value.truncate(end);
    value
}

fn next_sequence(current: u64) -> Result<u64, AgentOperationError> {
    current
        .checked_add(1)
        .ok_or_else(|| AgentOperationError::Corrupt("agent sequence overflow".into()))
}

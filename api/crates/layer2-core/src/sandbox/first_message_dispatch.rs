use layer3_adapter::{ContentBlock, ConversationInput, ConversationSendOutcome};

use crate::{now_rfc3339, RuntimeCapability};

use super::agent_operation_service::{operation_store_error, MAX_OPERATION_COMMIT_RETRIES};
use super::first_message_service::{
    first_message_failure, first_message_log, require_work_authority, AgentWorkAuthority,
    FirstMessageDispatchOutcome,
};
use super::store::AgentStore;
use super::{
    Agent, AgentLogKind, AgentOperationError, AgentOperationService, AgentRuntimeTarget,
    AgentStatus, FirstMessageDelivery, FirstMessageRecord,
};

pub(super) struct PreparedFirstMessage {
    pub(super) target: AgentRuntimeTarget,
    pub(super) authority: AgentWorkAuthority,
    pub(super) record: FirstMessageRecord,
}

enum FirstMessageSend {
    PreWrite,
    Confirmed,
    Ambiguous(String),
}

impl AgentOperationService {
    pub(super) async fn send_first_message(
        &self,
        prepared: PreparedFirstMessage,
    ) -> Result<FirstMessageDispatchOutcome, AgentOperationError> {
        let current = AgentStore::get_agent(self.store.as_ref(), &prepared.target.agent_id)
            .await
            .map_err(operation_store_error)?
            .ok_or(AgentOperationError::NotFound)?;
        require_work_authority(&current, &prepared.authority)?;
        require_exact_first_message_fence(&current, &prepared.record)?;
        let classification = match self.runtimes.resolve(&prepared.target).await {
            Ok(connection) => match connection
                .capabilities
                .require([RuntimeCapability::Conversation])
            {
                Ok(()) => match connection
                    .input
                    .send_classified(ConversationInput::Prompt(vec![ContentBlock::from(
                        prepared.record.text.clone(),
                    )]))
                    .await
                {
                    ConversationSendOutcome::PreWriteFailure(_) => FirstMessageSend::PreWrite,
                    ConversationSendOutcome::WriteConfirmed => FirstMessageSend::Confirmed,
                    ConversationSendOutcome::Ambiguous(error) => {
                        FirstMessageSend::Ambiguous(error.to_string())
                    }
                },
                Err(_) => FirstMessageSend::PreWrite,
            },
            Err(_) => FirstMessageSend::PreWrite,
        };
        self.settle_first_message(prepared, classification).await
    }

    async fn settle_first_message(
        &self,
        prepared: PreparedFirstMessage,
        classification: FirstMessageSend,
    ) -> Result<FirstMessageDispatchOutcome, AgentOperationError> {
        for _ in 0..MAX_OPERATION_COMMIT_RETRIES {
            let current = AgentStore::get_agent(self.store.as_ref(), &prepared.target.agent_id)
                .await
                .map_err(outcome_error)?
                .ok_or(AgentOperationError::NotFound)?;
            let record =
                current.conversation.first_message.as_ref().ok_or_else(|| {
                    AgentOperationError::Corrupt("first message disappeared".into())
                })?;
            if record.payload_sha256 != prepared.record.payload_sha256
                || record.recovery_generation != prepared.record.recovery_generation
            {
                return Err(AgentOperationError::OutcomeUnavailable(
                    "first-message dispatch identity changed".into(),
                ));
            }
            match record.state {
                FirstMessageDelivery::WriteConfirmed
                | FirstMessageDelivery::ExecutionObserved
                | FirstMessageDelivery::Completed
                | FirstMessageDelivery::Failed => {
                    return Ok(FirstMessageDispatchOutcome::Accepted(record.clone()));
                }
                FirstMessageDelivery::RecoveryRequired => {
                    return Ok(FirstMessageDispatchOutcome::RecoveryRequired(
                        record.clone(),
                    ));
                }
                FirstMessageDelivery::Queued => {
                    return Ok(FirstMessageDispatchOutcome::Retryable(record.clone()));
                }
                FirstMessageDelivery::DispatchFenced => {}
            }
            let at = now_rfc3339();
            let mut next = current.clone();
            let next_record = next
                .conversation
                .first_message
                .as_mut()
                .expect("the record was checked");
            match &classification {
                FirstMessageSend::Confirmed => next_record
                    .mark_write_confirmed(at.clone())
                    .map_err(|error| AgentOperationError::Corrupt(error.to_string()))?,
                FirstMessageSend::PreWrite => next_record
                    .retry_after_pre_write_failure()
                    .map_err(|error| AgentOperationError::Corrupt(error.to_string()))?,
                FirstMessageSend::Ambiguous(detail) => next_record
                    .require_recovery(first_message_failure(
                        "first_message_write_ambiguous",
                        detail,
                        false,
                        &at,
                    )?)
                    .map_err(|error| AgentOperationError::Corrupt(error.to_string()))?,
            }
            let settled = next_record.clone();
            let mut logs = vec![first_message_log(&settled)];
            if matches!(&classification, FirstMessageSend::PreWrite)
                && current.status == AgentStatus::Working
                && current.conversation.active_turn == settled.turn_sequence
            {
                next.conversation.active_turn = None;
                next.transition(AgentStatus::Ready, at.clone(), None)
                    .map_err(|error| AgentOperationError::Corrupt(error.to_string()))?;
                logs.push(AgentLogKind::StatusChanged {
                    from: AgentStatus::Working,
                    to: AgentStatus::Ready,
                });
            }
            match self.commit_change(&current, next, logs, at).await {
                Ok(_) => {
                    return Ok(match classification {
                        FirstMessageSend::Confirmed => {
                            FirstMessageDispatchOutcome::Accepted(settled)
                        }
                        FirstMessageSend::PreWrite => {
                            FirstMessageDispatchOutcome::Retryable(settled)
                        }
                        FirstMessageSend::Ambiguous(_) => {
                            FirstMessageDispatchOutcome::RecoveryRequired(settled)
                        }
                    });
                }
                Err(AgentOperationError::Conflict) => continue,
                Err(error) => return Err(outcome_operation_error(error)),
            }
        }
        Err(AgentOperationError::OutcomeUnavailable(
            "agent changed during every first-message outcome commit".into(),
        ))
    }
}

fn require_exact_first_message_fence(
    agent: &Agent,
    expected: &FirstMessageRecord,
) -> Result<(), AgentOperationError> {
    let matches = agent
        .conversation
        .first_message
        .as_ref()
        .is_some_and(|record| {
            record.state == FirstMessageDelivery::DispatchFenced
                && record.payload_sha256 == expected.payload_sha256
                && record.recovery_generation == expected.recovery_generation
                && record.attempt == expected.attempt
                && record.fence_sequence == expected.fence_sequence
                && record.dispatch_generation == expected.dispatch_generation
        });
    matches
        .then_some(())
        .ok_or_else(|| AgentOperationError::OutcomeUnavailable("dispatch fence changed".into()))
}

fn outcome_error(error: super::store::SandboxStoreError) -> AgentOperationError {
    outcome_operation_error(operation_store_error(error))
}

fn outcome_operation_error(error: AgentOperationError) -> AgentOperationError {
    match error {
        AgentOperationError::NotFound => AgentOperationError::NotFound,
        AgentOperationError::Corrupt(detail) => AgentOperationError::Corrupt(detail),
        other => AgentOperationError::OutcomeUnavailable(other.to_string()),
    }
}

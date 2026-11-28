use layer3_adapter::ConversationSendOutcome;
use std::sync::Arc;

use crate::now_rfc3339;

use super::agent_operation_dispatch::{operation_log, PreWriteRollback, PreparedAgentOperation};
use super::agent_operation_service::{operation_store_error, MAX_OPERATION_COMMIT_RETRIES};
use super::agent_settings_validation::SettingsInputGuard;
use super::store::{AgentConversationMutation, AgentStore, SandboxMutationGuard};
use super::{
    Agent, AgentLogEntry, AgentLogKind, AgentOperationDelivery, AgentOperationError,
    AgentOperationFailure, AgentOperationReceipt, AgentOperationService, AgentStatus,
    QueuedMessageState, TypedFailure,
};

#[path = "agent_operation_prewrite.rs"]
mod prewrite;
use prewrite::append_pre_write_rollback;

enum SendClassification {
    SourceDeferred(String),
    Rejected(String),
    PreWrite(String),
    Confirmed,
    Ambiguous(String),
}

impl AgentOperationService {
    pub(super) async fn dispatch_prepared(
        &self,
        prepared: PreparedAgentOperation,
    ) -> Result<AgentOperationReceipt, AgentOperationError> {
        let trace_configuration = |stage: &'static str| {
            if matches!(
                prepared.input,
                layer3_adapter::ConversationInput::Configure { .. }
            ) {
                use sha2::{Digest, Sha256};
                tracing::debug!(
                    target: "layer2_core::sandbox::settings_dispatch",
                    parent: None,
                    agent_sha256 = %format_args!("{:x}", Sha256::digest(prepared.target.agent_id.as_str())),
                    sequence = prepared.operation_sequence,
                    stage,
                    "agent configuration dispatch"
                );
            }
        };
        trace_configuration("started");
        let classification = match self.runtimes.resolve(&prepared.target).await {
            Ok(connection) => match connection
                .capabilities
                .require(prepared.required.iter().copied())
            {
                Ok(()) => {
                    let outcome = if self.funded_message.is_some() {
                        self.send_funded(&prepared).await
                    } else if matches!(
                        prepared.input,
                        layer3_adapter::ConversationInput::RefreshContext { .. }
                    ) {
                        connection
                            .input
                            .send_guarded(
                                prepared.input.clone(),
                                self.managed_context_input_guard(&prepared),
                            )
                            .await
                    } else if matches!(
                        prepared.input,
                        layer3_adapter::ConversationInput::Configure { .. }
                    ) {
                        connection
                            .input
                            .send_guarded(
                                prepared.input.clone(),
                                Arc::new(SettingsInputGuard {
                                    authority: self.settings_authority.clone(),
                                    store: self.store.clone(),
                                    target: prepared.target.clone(),
                                    sequence: prepared.operation_sequence,
                                }),
                            )
                            .await
                    } else if let Some(authority) = &self.execution_authority {
                        connection
                            .input
                            .send_guarded(
                                prepared.input.clone(),
                                std::sync::Arc::new(
                                    super::agent_io::execution_dispatch::CommandDispatchGuard {
                                        authority: authority.clone(),
                                        target: prepared.target.clone(),
                                        operation_sequence: prepared.operation_sequence,
                                        turn_sequence: prepared.turn_sequence,
                                    },
                                ),
                            )
                            .await
                    } else if let Some(authority) = &self.trigger_authority {
                        connection
                            .input
                            .send_guarded(
                                prepared.input.clone(),
                                std::sync::Arc::new(
                                    super::agent_io::execution_dispatch::TriggerDispatchGuard {
                                        authority: authority.clone(),
                                        target: prepared.target.clone(),
                                        operation_sequence: prepared.operation_sequence,
                                        turn_sequence: prepared.turn_sequence,
                                    },
                                ),
                            )
                            .await
                    } else {
                        connection
                            .input
                            .send_classified(prepared.input.clone())
                            .await
                    };
                    match outcome {
                        ConversationSendOutcome::PreWriteFailure(
                            layer3_adapter::AdapterError::Unsupported(detail),
                        ) if matches!(
                            prepared.input,
                            layer3_adapter::ConversationInput::Configure { .. }
                                | layer3_adapter::ConversationInput::RefreshContext { .. }
                        ) =>
                        {
                            SendClassification::Rejected(detail)
                        }
                        ConversationSendOutcome::PreWriteFailure(
                            layer3_adapter::AdapterError::InputAuthorityUnavailable(detail),
                        ) => SendClassification::SourceDeferred(detail),
                        ConversationSendOutcome::PreWriteFailure(error) => {
                            SendClassification::PreWrite(error.to_string())
                        }
                        ConversationSendOutcome::WriteConfirmed => SendClassification::Confirmed,
                        ConversationSendOutcome::Ambiguous(error) => {
                            SendClassification::Ambiguous(error.to_string())
                        }
                    }
                }
                Err(capability) => {
                    self.classify_runtime_refusal(format!(
                        "runtime capability changed before dispatch: {capability:?}"
                    ))
                    .await
                }
            },
            Err(error) => self.classify_runtime_refusal(error.to_string()).await,
        };
        if matches!(classification, SendClassification::Confirmed) {
            trace_configuration("confirmed");
        }
        self.settle_operation(prepared, classification).await
    }

    async fn classify_runtime_refusal(&self, detail: String) -> SendClassification {
        if self.funded_message.is_none() {
            return SendClassification::PreWrite(detail);
        }
        match self.resolve_failed_funded_send(detail).await {
            ConversationSendOutcome::WriteConfirmed => SendClassification::Confirmed,
            ConversationSendOutcome::PreWriteFailure(error) => {
                SendClassification::PreWrite(error.to_string())
            }
            ConversationSendOutcome::Ambiguous(error) => {
                SendClassification::Ambiguous(error.to_string())
            }
        }
    }

    async fn settle_operation(
        &self,
        prepared: PreparedAgentOperation,
        classification: SendClassification,
    ) -> Result<AgentOperationReceipt, AgentOperationError> {
        let detail = match &classification {
            SendClassification::Rejected(detail)
            | SendClassification::PreWrite(detail)
            | SendClassification::SourceDeferred(detail)
            | SendClassification::Ambiguous(detail) => Some(bounded_detail(detail)),
            SendClassification::Confirmed => None,
        };
        for _ in 0..MAX_OPERATION_COMMIT_RETRIES {
            let current = AgentStore::get_agent(self.store.as_ref(), &prepared.target.agent_id)
                .await
                .map_err(outcome_store_error)?
                .ok_or(AgentOperationError::NotFound)?;
            let record = current
                .conversation
                .operation
                .as_ref()
                .filter(|record| record.sequence == prepared.operation_sequence)
                .ok_or_else(|| {
                    AgentOperationError::OutcomeUnavailable(
                        "the durable dispatch fence was replaced".into(),
                    )
                })?;
            if matches!(
                record.state,
                AgentOperationDelivery::WriteConfirmed | AgentOperationDelivery::ExecutionObserved
            ) {
                return Ok(receipt(current, prepared.operation_sequence));
            }
            if record.state == AgentOperationDelivery::RecoveryRequired
                && !matches!(classification, SendClassification::Confirmed)
            {
                return Err(AgentOperationError::DeliveryAmbiguous {
                    operation_sequence: record.sequence,
                    detail: record
                        .failure
                        .as_ref()
                        .map(|failure| failure.detail.clone())
                        .unwrap_or_else(|| "delivery requires recovery".into()),
                });
            }
            if record.state == AgentOperationDelivery::PreWriteFailed {
                return Err(AgentOperationError::RuntimeUnavailable(
                    record
                        .failure
                        .as_ref()
                        .map(|failure| failure.detail.clone())
                        .unwrap_or_else(|| "input was not written".into()),
                ));
            }

            let at = now_rfc3339();
            let mut next = current.clone();
            let next_record = next
                .conversation
                .operation
                .as_mut()
                .expect("the matching operation was checked");
            match &classification {
                SendClassification::Confirmed => {
                    let result = if matches!(
                        next_record.intent,
                        super::AgentOperationIntent::UpdateSettings { .. }
                            | super::AgentOperationIntent::RefreshContext { .. }
                    ) {
                        next_record.mark_execution_observed(at.clone())
                    } else {
                        next_record.mark_write_confirmed(at.clone())
                    };
                    result.map_err(|error| AgentOperationError::Corrupt(error.to_string()))?;
                    if let super::AgentOperationIntent::RefreshContext { revision, .. } =
                        &next_record.intent
                    {
                        next.conversation.managed_guidance_revision = *revision;
                        next.conversation.settings =
                            super::agent_managed_context::confirmed_settings(
                                &current,
                                &next_record.intent,
                            )
                            .map_err(|error| AgentOperationError::Corrupt(error.to_string()))?;
                    }
                }
                SendClassification::Rejected(detail)
                | SendClassification::PreWrite(detail)
                | SendClassification::SourceDeferred(detail) => next_record
                    .mark_pre_write_failed(operation_failure("input_not_written", detail, &at)?)
                    .map_err(|error| AgentOperationError::Corrupt(error.to_string()))?,
                SendClassification::Ambiguous(detail) => next_record
                    .require_recovery(operation_failure("input_write_ambiguous", detail, &at)?)
                    .map_err(|error| AgentOperationError::Corrupt(error.to_string()))?,
            }
            settle_queued_message(&mut next, &prepared, &classification)?;
            let mut logs = vec![operation_log(&next, prepared.operation_sequence)?];
            if matches!(
                &classification,
                SendClassification::Rejected(_)
                    | SendClassification::PreWrite(_)
                    | SendClassification::SourceDeferred(_)
            ) {
                let deferred = if matches!(classification, SendClassification::SourceDeferred(_)) {
                    self.execution_authority
                        .as_ref()
                        .and_then(|authority| authority.continuation_envelope())
                        .map(|envelope| {
                            super::agent_io::continuation_retry::deferred_detail(
                                envelope,
                                prepared.operation_sequence,
                                prepared.turn_sequence,
                            )
                        })
                } else {
                    None
                };
                append_pre_write_rollback(
                    &current, &mut next, &prepared, &at, &mut logs, deferred,
                )?;
            }
            super::agent_io::trigger::project_delivery(
                &mut next,
                prepared.operation_sequence,
                &at,
                &mut logs,
            )?;
            match self.commit_change(&current, next, logs, at).await {
                Ok((agent, _)) => {
                    return match &classification {
                        SendClassification::Confirmed => {
                            Ok(receipt(agent, prepared.operation_sequence))
                        }
                        SendClassification::Rejected(_) => {
                            Err(AgentOperationError::InvalidRequest(
                                detail.clone().expect("failure detail exists"),
                            ))
                        }
                        SendClassification::PreWrite(_) | SendClassification::SourceDeferred(_) => {
                            Err(AgentOperationError::RuntimeUnavailable(
                                detail.clone().expect("failure detail exists"),
                            ))
                        }
                        SendClassification::Ambiguous(_) => {
                            Err(AgentOperationError::DeliveryAmbiguous {
                                operation_sequence: prepared.operation_sequence,
                                detail: detail.clone().expect("failure detail exists"),
                            })
                        }
                    };
                }
                Err(AgentOperationError::Conflict) => continue,
                Err(error) => return Err(outcome_error(error)),
            }
        }
        Err(AgentOperationError::OutcomeUnavailable(
            "agent changed during every outcome commit attempt".into(),
        ))
    }

    pub(super) async fn commit_change(
        &self,
        current: &Agent,
        next: Agent,
        logs: Vec<AgentLogKind>,
        at: String,
    ) -> Result<(Agent, Vec<AgentLogEntry>), AgentOperationError> {
        self.commit_guarded_change(current, next, logs, at, None)
            .await
    }

    pub(super) async fn commit_admission(
        &self,
        current: &Agent,
        next: Agent,
        logs: Vec<AgentLogKind>,
        at: String,
        guard: SandboxMutationGuard,
    ) -> Result<(Agent, Vec<AgentLogEntry>), AgentOperationError> {
        self.commit_guarded_change(current, next, logs, at, Some(guard))
            .await
    }

    async fn commit_guarded_change(
        &self,
        current: &Agent,
        mut next: Agent,
        logs: Vec<AgentLogKind>,
        at: String,
        admission_guard: Option<SandboxMutationGuard>,
    ) -> Result<(Agent, Vec<AgentLogEntry>), AgentOperationError> {
        if next.revision == current.revision {
            next.revision = current
                .revision
                .checked_add(1)
                .ok_or_else(|| AgentOperationError::Corrupt("agent revision overflow".into()))?;
            next.updated_at = at.clone();
        }
        let count = u64::try_from(logs.len())
            .map_err(|_| AgentOperationError::Corrupt("agent log overflow".into()))?;
        next.conversation.last_seq = current
            .conversation
            .last_seq
            .checked_add(count)
            .ok_or_else(|| AgentOperationError::Corrupt("agent log overflow".into()))?;
        next.validate_conversation()
            .map_err(|error| AgentOperationError::Corrupt(error.to_string()))?;
        let mutation = AgentConversationMutation {
            settings_fence: admission_guard
                .as_ref()
                .and(self.settings_authority.as_ref())
                .map(|authority| authority.fence()),
            command_claim: admission_guard.as_ref().and(self.command_claim.clone()),
            execution_fence: admission_guard
                .as_ref()
                .and(self.execution_authority.as_ref())
                .map(|authority| authority.fence()),
            trigger_execution_fence: admission_guard
                .as_ref()
                .and(self.trigger_authority.as_ref())
                .map(|authority| authority.fence()),
            admission_guard,
            expected_agent_revision: current.revision,
            expected_last_seq: current.conversation.last_seq,
            runtime_generation: current.conversation.runtime_generation.ok_or_else(|| {
                AgentOperationError::Corrupt("runtime generation is absent".into())
            })?,
            agent: next.clone(),
            appended_at: at,
            agent_logs: logs,
        };
        let appended = self
            .store
            .commit_agent_conversation(mutation)
            .await
            .map_err(operation_store_error)?;
        Ok((next, appended))
    }
}

fn settle_queued_message(
    next: &mut Agent,
    prepared: &PreparedAgentOperation,
    classification: &SendClassification,
) -> Result<(), AgentOperationError> {
    let Some(sequence) = prepared.queued_message_sequence else {
        return Ok(());
    };
    let message = next
        .conversation
        .queued_messages
        .iter_mut()
        .find(|message| message.sequence == sequence)
        .ok_or_else(|| {
            AgentOperationError::OutcomeUnavailable(
                "the queued message associated with the dispatch fence is absent".into(),
            )
        })?;
    let valid_state = message.state == QueuedMessageState::DispatchFenced
        || (matches!(classification, SendClassification::Confirmed)
            && message.state == QueuedMessageState::RecoveryRequired);
    if !valid_state {
        return Err(AgentOperationError::OutcomeUnavailable(
            "the queued message dispatch fence was replaced".into(),
        ));
    }
    let result = match classification {
        SendClassification::Confirmed => message.mark_delivered(),
        SendClassification::Rejected(_)
        | SendClassification::PreWrite(_)
        | SendClassification::SourceDeferred(_) => message.retry_after_pre_write_failure(),
        SendClassification::Ambiguous(_) => message.require_recovery(),
    };
    result.map_err(|error| AgentOperationError::Corrupt(error.to_string()))
}

fn receipt(agent: Agent, operation_sequence: u64) -> AgentOperationReceipt {
    let (turn_sequence, delivery) = agent
        .conversation
        .operation
        .as_ref()
        .map(|record| (record.turn_sequence, record.state))
        .expect("a settled operation retains its record");
    AgentOperationReceipt {
        agent,
        operation_sequence,
        turn_sequence,
        delivery,
    }
}

fn operation_failure(
    code: &str,
    detail: &str,
    at: &str,
) -> Result<AgentOperationFailure, AgentOperationError> {
    AgentOperationFailure::new(code, bounded_detail(detail), at)
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

fn outcome_store_error(error: super::store::SandboxStoreError) -> AgentOperationError {
    outcome_error(operation_store_error(error))
}

fn outcome_error(error: AgentOperationError) -> AgentOperationError {
    match error {
        AgentOperationError::NotFound => AgentOperationError::NotFound,
        AgentOperationError::Corrupt(detail) => AgentOperationError::Corrupt(detail),
        AgentOperationError::LogCapacityExceeded => AgentOperationError::LogCapacityExceeded,
        other => AgentOperationError::OutcomeUnavailable(other.to_string()),
    }
}

use std::collections::BTreeMap;

use crate::{AgentId, Caller};

use super::super::{
    AgentApprovalDecision, AgentMessageOutcome, AgentOperationError, AgentOperationReceipt,
    AgentOperationService,
};
use super::{AgentCommand, AgentInput, AgentInputOrder};

impl crate::sandbox::AgentReadService {
    /// Replay the common public projection under the same lineage checks and
    /// log-integrity rules used for history. This never reads the live lane.
    pub async fn events(
        &self,
        caller: &Caller,
        id: &AgentId,
        after: u64,
        limit: u16,
    ) -> Result<Vec<super::AgentEvent>, crate::sandbox::AgentReadError> {
        Ok(self
            .log(caller, id, after, limit)
            .await?
            .into_iter()
            .filter_map(|entry| super::AgentEvent::try_from(entry).ok())
            .collect())
    }
}

impl AgentOperationService {
    /// Common typed entry point for interaction adapters. The authenticated
    /// caller is supplied separately from untrusted intent and checked again
    /// against retained ownership and dispatch fences by each operation.
    pub async fn execute_command(
        &self,
        caller: &Caller,
        id: &AgentId,
        command: AgentCommand,
    ) -> Result<AgentMessageOutcome, AgentOperationError> {
        self.require_owner_authority(caller)?;
        if command.external_event().is_some() && self.execution_authority.is_none() {
            return Err(super::input::invalid(
                "external events require durable command execution",
            ));
        }
        match command.canonical()? {
            AgentCommand::Message {
                input,
                order,
                metadata,
            } => {
                let task_acceptance = input.task_acceptance.clone();
                if task_acceptance.is_some() && self.command_claim.is_none() {
                    return Err(super::input::invalid(
                        "task acceptance requires durable admission",
                    ));
                }
                let text = input.into_text()?;
                match order {
                    AgentInputOrder::Append | AgentInputOrder::InterruptAfterTurn { .. } => {
                        self.execute_message(caller, id, text, metadata, task_acceptance.as_ref())
                            .await
                    }
                    AgentInputOrder::AfterTurn { turn_sequence } => self
                        .execute_linear_message(
                            caller,
                            id,
                            text,
                            turn_sequence,
                            metadata,
                            task_acceptance.as_ref(),
                        )
                        .await
                        .map(AgentMessageOutcome::Dispatched),
                }
            }
            AgentCommand::Steer { input } => self
                .execute_steer(caller, id, input.into_text()?)
                .await
                .map(AgentMessageOutcome::Dispatched),
            AgentCommand::Interrupt => self
                .execute_interrupt(caller, id)
                .await
                .map(AgentMessageOutcome::Dispatched),
            AgentCommand::AnswerApproval {
                approval_id,
                decision,
            } => self
                .execute_approval(caller, id, approval_id, decision)
                .await
                .map(AgentMessageOutcome::Dispatched),
        }
    }

    pub async fn send_message(
        &self,
        caller: &Caller,
        id: &AgentId,
        text: String,
    ) -> Result<AgentMessageOutcome, AgentOperationError> {
        self.execute_command(caller, id, AgentCommand::message(text))
            .await
    }

    pub async fn send_linear_message(
        &self,
        caller: &Caller,
        id: &AgentId,
        text: String,
        previous_turn_sequence: Option<u64>,
        metadata: BTreeMap<String, String>,
    ) -> Result<AgentOperationReceipt, AgentOperationError> {
        dispatched(
            self.execute_command(
                caller,
                id,
                AgentCommand::Message {
                    input: AgentInput::text(text),
                    order: AgentInputOrder::AfterTurn {
                        turn_sequence: previous_turn_sequence,
                    },
                    metadata,
                },
            )
            .await?,
        )
    }

    pub async fn steer(
        &self,
        caller: &Caller,
        id: &AgentId,
        text: String,
    ) -> Result<AgentOperationReceipt, AgentOperationError> {
        dispatched(
            self.execute_command(
                caller,
                id,
                AgentCommand::Steer {
                    input: AgentInput::text(text),
                },
            )
            .await?,
        )
    }

    pub async fn interrupt(
        &self,
        caller: &Caller,
        id: &AgentId,
    ) -> Result<AgentOperationReceipt, AgentOperationError> {
        dispatched(
            self.execute_command(caller, id, AgentCommand::Interrupt)
                .await?,
        )
    }

    pub async fn answer_approval(
        &self,
        caller: &Caller,
        id: &AgentId,
        approval_id: String,
        decision: AgentApprovalDecision,
    ) -> Result<AgentOperationReceipt, AgentOperationError> {
        dispatched(
            self.execute_command(
                caller,
                id,
                AgentCommand::AnswerApproval {
                    approval_id,
                    decision,
                },
            )
            .await?,
        )
    }
}

fn dispatched(outcome: AgentMessageOutcome) -> Result<AgentOperationReceipt, AgentOperationError> {
    match outcome {
        AgentMessageOutcome::Dispatched(receipt) => Ok(receipt),
        AgentMessageOutcome::Queued { .. } => Err(AgentOperationError::Corrupt(
            "a control or linear input was incorrectly queued".into(),
        )),
    }
}

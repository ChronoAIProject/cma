//! Durable command/response reservation in an additive ledger. No runtime input
//! is issued here; unknown commits are recovered by the same idempotency key.

use crate::store::transaction_terminal::GuardedTerminal as _;
use layer2_core::ids::AgentCommandId;
use layer2_core::sandbox::agent_io::{
    AdmittedAgentCommand, AgentCommandStore, CommandLedgerError, CommandReservation,
    CommandReservationResult,
};
use mongodb::bson::doc;

use super::sandbox_mongo::{
    abort_with_acknowledgment, commit_transaction, is_confirmed_body_retry, start_transaction,
    CommitOutcome, MongoSandboxStore,
};

pub(super) mod audit;
pub(super) mod binding;
mod interrupting_message;
pub(in crate::store) mod lane_records;
pub(in crate::store) mod queued_cancellation;
mod queued_input;
pub(in crate::store) mod reconcile;
mod reconcile_heads;
pub(in crate::store) mod records;
pub(in crate::store) mod reserve;
mod retry;
mod targeted_control;
pub(in crate::store) mod unclaimed;
pub(in crate::store) mod worker;
pub(in crate::store) mod worker_apply;

pub(super) use lane_records::read as read_command_lane;
#[cfg(test)]
pub(super) use records::document as command_document;
pub(super) use records::ensure_indexes;
pub(super) use records::read as read_command;

#[derive(Debug)]
pub(super) enum ApplyError {
    Domain(CommandLedgerError),
    Driver(mongodb::error::Error),
}

impl From<CommandLedgerError> for ApplyError {
    fn from(value: CommandLedgerError) -> Self {
        Self::Domain(value)
    }
}

impl From<mongodb::error::Error> for ApplyError {
    fn from(value: mongodb::error::Error) -> Self {
        Self::Driver(value)
    }
}

impl From<super::agent_conversation_store::ApplyError> for ApplyError {
    fn from(error: super::agent_conversation_store::ApplyError) -> Self {
        use super::agent_conversation_store::ApplyError as ConversationError;
        use layer2_core::sandbox::store::SandboxStoreError;

        match error {
            ConversationError::Driver(error) => Self::Driver(error),
            ConversationError::Domain(SandboxStoreError::Budget(reason)) => {
                CommandLedgerError::Budget(reason).into()
            }
            ConversationError::Domain(SandboxStoreError::AdmissionPaused) => {
                CommandLedgerError::AdmissionPaused.into()
            }
            ConversationError::Domain(SandboxStoreError::SessionCapacityUnavailable) => {
                CommandLedgerError::SessionCapacityUnavailable.into()
            }
            _ => CommandLedgerError::Unavailable.into(),
        }
    }
}

#[async_trait::async_trait]
impl AgentCommandStore for MongoSandboxStore {
    async fn reserve_interrupting_message(
        &self,
        request: layer2_core::sandbox::agent_io::InterruptingMessageReservation,
    ) -> Result<layer2_core::sandbox::agent_io::AdmittedInterruptingMessage, CommandLedgerError>
    {
        let mut observation =
            crate::telemetry::work::Admission::new("reserve_interrupting_message");
        let result = interrupting_message::run(self, request).await;
        observation.result(&result);
        result
    }

    async fn edit_queued_input(
        &self,
        request: layer2_core::sandbox::agent_io::QueuedInputReservation,
    ) -> Result<layer2_core::sandbox::agent_io::QueuedInputOutcome, CommandLedgerError> {
        let mut observation = crate::telemetry::work::Admission::new("edit_queued_input");
        let result = queued_input::run(self, request).await;
        observation.result(&result);
        result
    }
    async fn list_queued_inputs(
        &self,
        agent_id: &layer2_core::AgentId,
    ) -> Result<Vec<AdmittedAgentCommand>, CommandLedgerError> {
        use futures_util::TryStreamExt;
        let mut cursor = self.agent_commands.find(doc! { "agent_id": agent_id.as_str(), "status": { "$in": ["queued", "waiting_action"] } }).sort(doc! { "agent_order": 1 }).await.map_err(|_| CommandLedgerError::Unavailable)?;
        let mut result = Vec::new();
        while let Some(document) = cursor
            .try_next()
            .await
            .map_err(|_| CommandLedgerError::Unavailable)?
        {
            let record = records::read(&document)?;
            if layer2_core::sandbox::agent_io::queued_cancellation_eligible(&record)
                && layer2_core::sandbox::agent_io::queued_input_editable_actor(&record)
            {
                result.push(record);
            }
            if result.len() > layer2_core::sandbox::agent_io::MAX_COMMAND_QUEUE_CAPACITY as usize {
                return Err(CommandLedgerError::Corrupt);
            }
        }
        Ok(result)
    }

    async fn cancel_queued_response(
        &self,
        request: layer2_core::sandbox::agent_io::QueuedCancellationReservation,
    ) -> Result<layer2_core::sandbox::agent_io::QueuedCancellationOutcome, CommandLedgerError> {
        let mut observation = crate::telemetry::work::Admission::new("cancel_queued_response");
        let result = queued_cancellation::run(self, request).await;
        observation.result(&result);
        result
    }

    async fn reserve_targeted_control(
        &self,
        request: layer2_core::sandbox::agent_io::TargetedControlReservation,
    ) -> Result<layer2_core::sandbox::agent_io::AdmittedTargetedControl, CommandLedgerError> {
        let mut observation = crate::telemetry::work::Admission::new("reserve_targeted_control");
        let result = async {
            targeted_control::run(self, request, true)
                .await?
                .ok_or(CommandLedgerError::Corrupt)
        }
        .await;
        observation.result(&result);
        result
    }

    async fn lookup_targeted_control(
        &self,
        request: layer2_core::sandbox::agent_io::TargetedControlReservation,
    ) -> Result<Option<layer2_core::sandbox::agent_io::AdmittedTargetedControl>, CommandLedgerError>
    {
        targeted_control::run(self, request, false).await
    }

    async fn inspect_targeted_control(
        &self,
        request: layer2_core::sandbox::agent_io::TargetedControlReservation,
    ) -> Result<layer2_core::sandbox::agent_io::TargetedControlInspection, CommandLedgerError> {
        targeted_control::inspect(self, request).await
    }

    async fn get_admitted_command(
        &self,
        scope: &str,
        key_sha256: &str,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        self.agent_commands
            .find_one(doc! { "scope": scope, "key_sha256": key_sha256 })
            .await
            .map_err(|_| CommandLedgerError::Unavailable)?
            .as_ref()
            .map(records::read)
            .transpose()
    }

    async fn get_external_event_command(
        &self,
        key: &str,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        self.agent_commands
            .find_one(doc! { "external_event_key": key })
            .await
            .map_err(|_| CommandLedgerError::Unavailable)?
            .as_ref()
            .map(records::read)
            .transpose()
    }

    async fn get_operation_command(
        &self,
        agent_id: &layer2_core::AgentId,
        operation_sequence: u64,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        use futures_util::TryStreamExt;
        let sequence =
            i64::try_from(operation_sequence).map_err(|_| CommandLedgerError::Invalid)?;
        let mut cursor = self
            .agent_commands
            .find(doc! {
                "agent_id": agent_id.as_str(), "record.work.dispatch.operation_sequence": sequence,
            })
            .limit(2)
            .await
            .map_err(|_| CommandLedgerError::Unavailable)?;
        let first = cursor
            .try_next()
            .await
            .map_err(|_| CommandLedgerError::Unavailable)?;
        if cursor
            .try_next()
            .await
            .map_err(|_| CommandLedgerError::Unavailable)?
            .is_some()
        {
            return Err(CommandLedgerError::Corrupt);
        }
        first.as_ref().map(records::read).transpose()
    }

    async fn get_response_command(
        &self,
        id: &layer2_core::ids::AgentResponseId,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        self.agent_commands
            .find_one(doc! { "response_id": id.as_str() })
            .await
            .map_err(|_| CommandLedgerError::Unavailable)?
            .as_ref()
            .map(records::read)
            .transpose()
    }

    async fn reserve_command(
        &self,
        request: CommandReservation,
    ) -> Result<CommandReservationResult, CommandLedgerError> {
        let mut observation = crate::telemetry::work::Admission::new("reserve_command");
        let result = async {
            request.require_non_channel()?;
            request.validate()?;
            if self
                .fleet_log_activation
                .selected(&request.envelope.agent_id)
                && matches!(
                    request.envelope.command,
                    layer2_core::sandbox::agent_io::AgentCommand::Message { .. }
                )
            {
                request.require_current_authority()?;
                self.prepare_fleet_log(&request.envelope.agent_id).await;
            }
            // Local period-row writers are scheduled for the whole retry loop
            // (concurrency rule 5); the guard drops with this block, before
            // the admission telemetry below is recorded.
            let _platform = match request.envelope.command {
                layer2_core::sandbox::agent_io::AgentCommand::Message { .. } => {
                    self.platform_write_guard().await
                }
                _ => None,
            };
            let mut retries = retry::CommandAdmissionRetries::new(request.authority_deadline);
            while retries.next().await {
                request.require_current_authority()?;
                let mut session = self
                    .client
                    .start_session()
                    .await
                    .map_err(|_| CommandLedgerError::Unavailable)?;
                start_transaction(&mut session)
                    .await
                    .map_err(|_| CommandLedgerError::Unavailable)?;
                let result = match reserve::apply(self, &request, &mut session).await {
                    Ok(result) => result,
                    Err(ApplyError::Domain(error)) => {
                        if error == CommandLedgerError::Conflict
                            && request.binding.is_none()
                            && request.envelope.continuation.is_none()
                        {
                            abort_with_acknowledgment(&mut session)
                                .await
                                .map_err(|_| CommandLedgerError::Unavailable)?;
                            return Err(CommandLedgerError::KnownAborted);
                        }
                        let _ = session.abort_guarded().await;
                        return Err(error);
                    }
                    Err(ApplyError::Driver(error)) => {
                        let _ = session.abort_guarded().await;
                        if is_confirmed_body_retry(&error) {
                            continue;
                        }
                        return Err(CommandLedgerError::Unavailable);
                    }
                };
                if let Err(error) = request.require_current_authority() {
                    let _ = session.abort_guarded().await;
                    return Err(error);
                }
                match commit_transaction(&mut session).await {
                    CommitOutcome::Committed => return Ok(result),
                    CommitOutcome::Retry(_) => continue,
                    // Nothing has executed. Never acknowledge an unconfirmed
                    // reservation or discard its permanent idempotency identity.
                    CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => {
                        return Err(CommandLedgerError::Unavailable);
                    }
                }
            }
            Err(CommandLedgerError::Conflict)
        }
        .await;
        observation.reservation(&result);
        result
    }

    async fn get_command(
        &self,
        id: &AgentCommandId,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        self.agent_commands
            .find_one(doc! { "_id": id.as_str() })
            .await
            .map_err(|_| CommandLedgerError::Unavailable)?
            .as_ref()
            .map(records::read)
            .transpose()
    }
}

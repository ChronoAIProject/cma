//! Durable command/response reservation in an additive ledger. No runtime input
//! is issued here; unknown commits are recovered by the same idempotency key.

use layer2_core::ids::AgentCommandId;
use layer2_core::sandbox::agent_io::{
    AdmittedAgentCommand, AgentCommandStore, CommandLedgerError, CommandReservation,
    CommandReservationResult,
};
use mongodb::bson::doc;

use super::sandbox_mongo::{
    commit_transaction, is_duplicate_key, is_transient, start_transaction, CommitOutcome,
    MongoSandboxStore, TRANSACTION_ATTEMPTS,
};

pub(super) mod binding;
mod lane_records;
mod reconcile;
mod records;
mod reserve;
mod worker;
mod worker_apply;

pub(super) use records::ensure_indexes;
pub(super) use records::read as read_command;

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

#[async_trait::async_trait]
impl AgentCommandStore for MongoSandboxStore {
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
        request.validate()?;
        for _ in 0..TRANSACTION_ATTEMPTS {
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
                    let _ = session.abort_transaction().await;
                    return Err(error);
                }
                Err(ApplyError::Driver(error)) => {
                    let _ = session.abort_transaction().await;
                    if is_transient(&error) || is_duplicate_key(&error) {
                        tokio::task::yield_now().await;
                        continue;
                    }
                    return Err(CommandLedgerError::Unavailable);
                }
            };
            if let Err(error) = request.require_current_authority() {
                let _ = session.abort_transaction().await;
                return Err(error);
            }
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(result),
                CommitOutcome::Retry(_) => continue,
                // Nothing has executed. Never acknowledge an unconfirmed
                // reservation or discard its permanent idempotency identity.
                CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => {
                    return Err(CommandLedgerError::Unavailable)
                }
            }
        }
        Err(CommandLedgerError::Conflict)
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

use layer2_core::sandbox::agent_io::{
    AdmittedAgentCommand, AgentCommandWorkerStore, CommandClaim, CommandClaimRequest,
    CommandFailureCode, CommandLedgerError,
};

use super::super::sandbox_mongo::{
    commit_transaction, is_transient, start_transaction, CommitOutcome, MongoSandboxStore,
    TRANSACTION_ATTEMPTS,
};
use super::ApplyError;

pub(super) enum Change {
    Claim(CommandClaimRequest),
    Renew {
        claim: CommandClaim,
        now: String,
        expires_at: String,
    },
    Release {
        claim: CommandClaim,
        now: String,
    },
    Reject {
        claim: CommandClaim,
        now: String,
        code: CommandFailureCode,
    },
}

#[async_trait::async_trait]
impl AgentCommandWorkerStore for MongoSandboxStore {
    async fn claim_next_command(
        &self,
        request: CommandClaimRequest,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        request.validate()?;
        transaction(self, Change::Claim(request)).await
    }

    async fn renew_command_claim(
        &self,
        claim: &CommandClaim,
        now: String,
        expires_at: String,
    ) -> Result<(), CommandLedgerError> {
        CommandClaimRequest {
            agent_id: claim.agent_id.clone(),
            holder: claim.holder.clone(),
            now: now.clone(),
            expires_at: expires_at.clone(),
        }
        .validate()?;
        transaction(
            self,
            Change::Renew {
                claim: claim.clone(),
                now,
                expires_at,
            },
        )
        .await
        .map(|_| ())
    }

    async fn release_command_claim(
        &self,
        claim: &CommandClaim,
        now: String,
    ) -> Result<(), CommandLedgerError> {
        transaction(
            self,
            Change::Release {
                claim: claim.clone(),
                now,
            },
        )
        .await
        .map(|_| ())
    }

    async fn reject_command(
        &self,
        claim: &CommandClaim,
        code: CommandFailureCode,
        now: String,
    ) -> Result<AdmittedAgentCommand, CommandLedgerError> {
        transaction(
            self,
            Change::Reject {
                claim: claim.clone(),
                code,
                now,
            },
        )
        .await?
        .ok_or(CommandLedgerError::StaleClaim)
    }
}

async fn transaction(
    store: &MongoSandboxStore,
    change: Change,
) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
    for _ in 0..TRANSACTION_ATTEMPTS {
        let mut session = store
            .client
            .start_session()
            .await
            .map_err(|_| CommandLedgerError::Unavailable)?;
        start_transaction(&mut session)
            .await
            .map_err(|_| CommandLedgerError::Unavailable)?;
        let result = match super::worker_apply::apply(store, &change, &mut session).await {
            Ok(result) => result,
            Err(ApplyError::Domain(error)) => {
                let _ = session.abort_transaction().await;
                return Err(error);
            }
            Err(ApplyError::Driver(error)) => {
                let _ = session.abort_transaction().await;
                if is_transient(&error) {
                    tokio::task::yield_now().await;
                    continue;
                }
                return Err(CommandLedgerError::Unavailable);
            }
        };
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => return Ok(result),
            CommitOutcome::Retry(_) => continue,
            CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => {
                return Err(CommandLedgerError::Unavailable)
            }
        }
    }
    Err(CommandLedgerError::Conflict)
}

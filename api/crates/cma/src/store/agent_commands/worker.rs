use crate::store::transaction_terminal::GuardedTerminal as _;
use layer2_core::sandbox::agent_io::{
    AdmittedAgentCommand, AgentCommandWorkerStore, CommandClaim, CommandClaimRequest,
    CommandFailureCode, CommandLedgerError,
};

use super::super::sandbox_mongo::{
    commit_transaction, is_transient, CommitOutcome, MongoSandboxStore, TRANSACTION_ATTEMPTS,
};
use super::ApplyError;
use crate::store::agent_log_segments::{
    command_worker, terminal, transaction_scope::TransactionScope,
};

pub(in crate::store) enum Change {
    Reconcile(layer2_core::ids::AgentCommandId),
    Claim {
        request: CommandClaimRequest,
        observed: Option<(layer2_core::ids::AgentCommandId, u64)>,
    },
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
    async fn command_reconcile_heads(
        &self,
        agent_ids: &[layer2_core::AgentId],
    ) -> Result<Vec<layer2_core::sandbox::agent_io::CommandReconcileHead>, CommandLedgerError> {
        super::reconcile_heads::read(self, agent_ids).await
    }

    async fn reject_unclaimed_command(
        &self,
        id: &layer2_core::ids::AgentCommandId,
        expected_revision: u64,
        code: CommandFailureCode,
        now: String,
    ) -> Result<(), CommandLedgerError> {
        use layer2_core::sandbox::agent_io::AgentCommandStore;
        let command = self
            .get_command(id)
            .await?
            .ok_or(CommandLedgerError::NotFound)?;
        super::unclaimed::run(self, &command, expected_revision, code, now).await
    }

    async fn command_work_page(
        &self,
        page: &layer2_core::sandbox::store::PageRequest,
    ) -> Result<layer2_core::sandbox::store::Page<AdmittedAgentCommand>, CommandLedgerError> {
        use futures_util::TryStreamExt;
        use mongodb::bson::doc;
        layer2_core::sandbox::store::PageRequest::new(page.limit, page.cursor.clone())
            .map_err(|_| CommandLedgerError::Invalid)?;
        let mut filter = doc! { "status": { "$in": ["queued", "in_progress", "waiting_action"] } };
        if self
            .empty_discovery
            .empty(
                super::super::empty_discovery::Kind::Commands,
                &self.agent_commands,
                filter.clone(),
            )
            .await
        {
            return Ok(layer2_core::sandbox::store::Page {
                items: Vec::new(),
                next: None,
            });
        }
        if let Some(cursor) = &page.cursor {
            filter.insert("_id", doc! { "$gt": cursor });
        }
        let mut cursor = self
            .agent_commands
            .find(filter)
            .sort(doc! { "_id": 1 })
            .hint(mongodb::options::Hint::Name(
                "agent_command_work_scan".into(),
            ))
            .limit(i64::from(page.limit) + 1)
            .max_time(std::time::Duration::from_secs(10))
            .await
            .map_err(|_| CommandLedgerError::Unavailable)?;
        let mut items = Vec::new();
        while let Some(document) = cursor
            .try_next()
            .await
            .map_err(|_| CommandLedgerError::Unavailable)?
        {
            items.push(super::records::read(&document)?);
        }
        self.empty_discovery.observed(
            super::super::empty_discovery::Kind::Commands,
            items.is_empty(),
        );
        layer2_core::sandbox::agent_io::finish_command_work_page(items, page)
    }

    async fn reconcile_command(
        &self,
        id: &layer2_core::ids::AgentCommandId,
    ) -> Result<AdmittedAgentCommand, CommandLedgerError> {
        let (command, outcome) = transaction_observed(self, Change::Reconcile(id.clone())).await?;
        if let Some(outcome) = outcome {
            outcome.record();
        }
        command.ok_or(CommandLedgerError::NotFound)
    }

    async fn claim_next_command(
        &self,
        request: CommandClaimRequest,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        request.validate()?;
        transaction(
            self,
            Change::Claim {
                request,
                observed: None,
            },
        )
        .await
    }

    async fn claim_observed_command(
        &self,
        request: CommandClaimRequest,
        command_id: &layer2_core::ids::AgentCommandId,
        expected_revision: u64,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        request.validate()?;
        transaction(
            self,
            Change::Claim {
                request,
                observed: Some((command_id.clone(), expected_revision)),
            },
        )
        .await
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
    transaction_observed(store, change)
        .await
        .map(|(command, _)| command)
}

pub(in crate::store) type TransactionResult = (
    Option<AdmittedAgentCommand>,
    Option<crate::telemetry::command_reconcile::Outcome>,
);

async fn transaction_observed(
    store: &MongoSandboxStore,
    change: Change,
) -> Result<TransactionResult, CommandLedgerError> {
    for _ in 0..TRANSACTION_ATTEMPTS {
        let mut session = store
            .client
            .start_session()
            .await
            .map_err(|_| CommandLedgerError::Unavailable)?;
        let mut scope = TransactionScope::start(&mut session)
            .await
            .map_err(|_| CommandLedgerError::Unavailable)?;
        let applied = Box::pin(command_worker::worker(store, &change, &mut scope)).await;
        let session = scope.release();
        let (result, receipt) = match applied {
            Ok(result) => result,
            Err(ApplyError::Domain(error)) => {
                let _ = session.abort_guarded().await;
                return Err(error);
            }
            Err(ApplyError::Driver(error)) => {
                let _ = session.abort_guarded().await;
                if is_transient(&error) {
                    tokio::task::yield_now().await;
                    continue;
                }
                return Err(CommandLedgerError::Unavailable);
            }
        };
        match commit_transaction(session).await {
            CommitOutcome::Committed => return Ok(result),
            CommitOutcome::Retry(_) => continue,
            CommitOutcome::Unknown(_) => {
                if terminal::reconcile(store, receipt.as_ref(), session.operation_time()).await? {
                    return Ok(result);
                }
                return Err(CommandLedgerError::Unavailable);
            }
            CommitOutcome::Failed(_) => return Err(CommandLedgerError::Unavailable),
        }
    }
    Err(CommandLedgerError::Conflict)
}

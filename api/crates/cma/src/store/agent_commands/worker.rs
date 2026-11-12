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
        use mongodb::bson::doc;
        let mut command = self
            .get_command(id)
            .await?
            .ok_or(CommandLedgerError::NotFound)?;
        if matches!(
            command.receipt.envelope.actor,
            layer2_core::sandbox::agent_io::AgentActor::Trigger { .. }
        ) {
            return super::super::trigger_first_messages::reject_unclaimed(
                self,
                &command,
                expected_revision,
                code,
                now,
            )
            .await;
        }
        command.reject_unclaimed(expected_revision, code, now)?;
        let mut fields = super::records::document(&command)?;
        fields.remove("_id");
        let revision = super::super::sandbox_mongo::signed(expected_revision, "command revision")
            .map_err(|_| CommandLedgerError::Invalid)?;
        let result = self
            .agent_commands
            .update_one(
                doc! { "_id": id.as_str(), "revision": revision },
                doc! { "$set": fields },
            )
            .await
            .map_err(|_| CommandLedgerError::Unavailable)?;
        if result.matched_count != 1 {
            return Err(CommandLedgerError::StaleClaim);
        }
        Ok(())
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
        layer2_core::sandbox::agent_io::finish_command_work_page(items, page)
    }

    async fn reconcile_command(
        &self,
        id: &layer2_core::ids::AgentCommandId,
    ) -> Result<AdmittedAgentCommand, CommandLedgerError> {
        transaction(self, Change::Reconcile(id.clone()))
            .await?
            .ok_or(CommandLedgerError::NotFound)
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

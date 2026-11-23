use std::time::Duration;

use layer2_core::{
    agent_ownership::execution_authority,
    sandbox::{
        store::{SandboxStoreError, StaleTurnSettlement},
        AgentLogEntry, AgentLogKind, PendingAgentApprovals,
    },
};
use mongodb::{bson::doc, options::ReadConcern};

use super::{evidence::MAX_TURN_ROWS, StaleTurnStore};
use crate::store::{
    agent_commands::records,
    agent_conversation_store::ApplyError,
    agent_log_codec::{log_id, read_entry},
    agent_log_segments::{
        conversation::checkpoint::apply_stale_turn, transaction_scope::TransactionScope,
    },
    budget,
    sandbox_mongo::{
        commit_transaction, is_transient, signed, CommitOutcome, DOCUMENT_SCHEMA_VERSION,
    },
    sandboxes::read_sandbox,
};

const RECEIPT: &str = "stale_turn_publication";
const READ_DEADLINE: Duration = Duration::from_secs(1);
const CONFLICT: ApplyError = ApplyError::Domain(SandboxStoreError::Conflict);

#[derive(Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    version: u32,
    proof_sha256: String,
    terminal_seq: i64,
    terminal_at: String,
    budget_key: Option<String>,
}

impl StaleTurnStore {
    pub(crate) async fn settle(
        &self,
        proof: &StaleTurnSettlement,
    ) -> Result<(), SandboxStoreError> {
        let token = proof.ownership();
        if self.store.agent_ownership.is_none()
            || execution_authority()
                .is_none_or(|owner| owner.token != *token || !owner.is_current())
            || self.store.agent_manager_instance() != Some(token.instance_id.as_str())
        {
            return Err(SandboxStoreError::Conflict);
        }
        if self.confirm(proof).await? {
            return Ok(());
        }
        let _writer = self.store.agent_writes.lock(&token.agent_id).await;
        let _platform = self
            .store
            .conversation_platform_guard(proof.mutation())
            .await;
        let mut session = self
            .store
            .client
            .start_session()
            .await
            .map_err(unavailable)?;
        let mut scope = TransactionScope::start(&mut session)
            .await
            .map_err(unavailable)?;
        let applied = Box::pin(self.apply(proof, &mut scope)).await;
        let session = scope.release();
        if let Err(error) = applied {
            if let Err(error) = session.abort_transaction().await {
                tracing::warn!(%error, "stale-turn transaction abort unconfirmed");
            }
            return Err(store_error(error));
        }
        match commit_transaction(session).await {
            CommitOutcome::Committed => {
                crate::telemetry::work::published(
                    &proof.checkpoint().outcome.appended,
                    "committed",
                );
                Ok(())
            }
            CommitOutcome::Unknown(_) => {
                // An operation time from the failed commit may still refer
                // to the precommit state. Only positive evidence confirms it.
                if self.confirm(proof).await? {
                    crate::telemetry::work::published(
                        &proof.checkpoint().outcome.appended,
                        "reconciled",
                    );
                    Ok(())
                } else {
                    Err(unavailable("stale-turn commit outcome unconfirmed"))
                }
            }
            CommitOutcome::Retry(_) => Err(SandboxStoreError::Conflict),
            CommitOutcome::Failed(error) => Err(unavailable(error)),
        }
    }

    async fn apply(
        &self,
        proof: &StaleTurnSettlement,
        scope: &mut TransactionScope<'_>,
    ) -> Result<(), ApplyError> {
        let parent = proof.parent();
        let agent = &proof.snapshot().agent;
        let command = proof.command();
        let id = &command.receipt.envelope.command_id;
        let row = scope
            .find_one(&self.store.agent_commands, doc! {"_id": id.as_str()})
            .await
            .map_err(ApplyError::Driver)?
            .ok_or(CONFLICT)?;
        if records::read(&row).map_err(|_| CONFLICT)? != *command
            || row.contains_key(RECEIPT)
            || row.contains_key("channel_history_seal")
        {
            return Err(CONFLICT);
        }
        let row = scope
            .find_one(&self.store.sandboxes, doc! {"_id": parent.id.as_str()})
            .await
            .map_err(ApplyError::Driver)?
            .ok_or(CONFLICT)?;
        if read_sandbox(&row).map_err(ApplyError::Domain)? != *parent {
            return Err(CONFLICT);
        }
        // This transaction writes all three rows shared with targeted control
        // admission: the Agent, the Sandbox and the original response command.
        let locked = scope.update_one(
            &self.store.sandboxes,
            doc! {
                "_id": parent.id.as_str(),
                "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                "revision": signed(parent.revision, "stale-turn parent revision").map_err(|_| CONFLICT)?,
            },
            doc! {"$inc": {"stale_turn_parent_fence": 1_i64}},
        ).await.map_err(ApplyError::Driver)?;
        if locked.matched_count != 1 {
            return Err(CONFLICT);
        }
        let pending = scope
            .find_one(
                &self.store.agent_commands,
                doc! {
                    "agent_id": agent.id.as_str(),
                    "_id": {"$ne": id.as_str()},
                    "status": {"$in": ["queued", "in_progress", "waiting_action"]},
                    "record.receipt.envelope.command.type": {"$ne": "message"},
                },
            )
            .await
            .map_err(ApplyError::Driver)?;
        if pending.is_some() {
            return Err(CONFLICT);
        }
        let (turn, event) = terminal_event(proof)?;
        self.require_no_approvals(proof, turn, scope).await?;
        let key = budget::turn_key(agent, turn);
        let reservation = scope
            .find_one(&self.store.model_budget, doc! {"_id": &key})
            .await
            .map_err(ApplyError::Driver)?;
        if let Some(row) = &reservation {
            // A pending budget stop already has its own failure disposition.
            budget::validate_stale_turn_reservation(row, agent, turn, false)?;
        }
        Box::pin(apply_stale_turn(&self.store, proof, scope)).await?;
        let budget_key = if reservation.is_some() {
            let row = scope
                .find_one(&self.store.model_budget, doc! {"_id": &key})
                .await
                .map_err(ApplyError::Driver)?
                .ok_or(CONFLICT)?;
            budget::validate_stale_turn_reservation(&row, agent, turn, true)?;
            Some(key)
        } else {
            None
        };
        let receipt = Receipt {
            version: 1,
            proof_sha256: proof.proof_sha256().to_owned(),
            terminal_seq: signed(event.seq, "stale-turn terminal sequence")
                .map_err(|_| CONFLICT)?,
            terminal_at: event.appended_at.clone(),
            budget_key,
        };
        let receipt = mongodb::bson::serialize_to_document(&receipt).map_err(|_| CONFLICT)?;
        // Ledger transitions $set their own fields and preserve this sidecar.
        let updated = scope.update_one(&self.store.agent_commands, doc! {
            "_id": id.as_str(),
            "revision": signed(command.revision, "stale-turn command revision").map_err(|_| CONFLICT)?,
            RECEIPT: {"$exists": false},
            "channel_history_seal": {"$exists": false},
        }, doc! {"$set": {RECEIPT: receipt}}).await.map_err(ApplyError::Driver)?;
        if updated.matched_count != 1 {
            return Err(CONFLICT);
        }
        Ok(())
    }

    async fn require_no_approvals(
        &self,
        proof: &StaleTurnSettlement,
        turn: u64,
        scope: &mut TransactionScope<'_>,
    ) -> Result<(), ApplyError> {
        let agent = &proof.snapshot().agent;
        let head = agent.conversation.last_seq;
        let length = head
            .checked_sub(turn)
            .filter(|count| *count < MAX_TURN_ROWS)
            .ok_or(CONFLICT)?
            + 1;
        let history = scope
            .find_page(
                &self.store.agent_log,
                doc! {
                    "agent_id": agent.id.as_str(),
                    "seq": {
                        "$gte": signed(turn, "stale-turn start").map_err(|_| CONFLICT)?,
                        "$lte": signed(head, "stale-turn head").map_err(|_| CONFLICT)?,
                    },
                },
                doc! {"seq": 1},
                length as u32,
            )
            .await
            .map_err(ApplyError::Driver)?;
        if history.len() != length as usize {
            return Err(CONFLICT);
        }
        let mut approvals = PendingAgentApprovals::default();
        for (offset, row) in history.iter().enumerate() {
            let entry = read_entry(row).map_err(ApplyError::Domain)?;
            if entry.agent_id != agent.id || entry.seq != turn + offset as u64 {
                return Err(CONFLICT);
            }
            approvals.observe(&entry);
        }
        if !approvals.into_entries().is_empty() {
            return Err(CONFLICT);
        }
        Ok(())
    }

    // Both log formats retain the exact event row. The receipt and the budget
    // identity survive an ordinary command projection or later checkpoint.
    async fn confirm(&self, proof: &StaleTurnSettlement) -> Result<bool, SandboxStoreError> {
        let (turn, event) = terminal_event(proof).map_err(store_error)?;
        let agent = &proof.snapshot().agent;
        let id = &proof.command().receipt.envelope.command_id;
        let mut session = self
            .store
            .client
            .start_session()
            .snapshot(true)
            .await
            .map_err(unavailable)?;
        let row = self
            .store
            .agent_commands
            .find_one(doc! {"_id": id.as_str()})
            .projection(doc! {RECEIPT: 1})
            .read_concern(ReadConcern::snapshot())
            .max_time(READ_DEADLINE)
            .session(&mut session)
            .await
            .map_err(unavailable)?;
        let Some(value) = row.and_then(|row| row.get(RECEIPT).cloned()) else {
            return Ok(false);
        };
        let receipt: Receipt = mongodb::bson::deserialize_from_bson(value).map_err(unavailable)?;
        let key = budget::turn_key(agent, turn);
        if receipt.version != 1
            || receipt.proof_sha256 != proof.proof_sha256()
            || u64::try_from(receipt.terminal_seq).ok() != Some(event.seq)
            || receipt
                .budget_key
                .as_ref()
                .is_some_and(|recorded| *recorded != key)
        {
            return Err(SandboxStoreError::Conflict);
        }
        let stored = self
            .store
            .agent_log
            .find_one(doc! {"_id": log_id(&agent.id, event.seq)})
            .read_concern(ReadConcern::snapshot())
            .max_time(READ_DEADLINE)
            .session(&mut session)
            .await
            .map_err(unavailable)?;
        let entry = stored.as_ref().map(read_entry).transpose()?;
        if entry.as_ref().is_none_or(|entry| {
            entry.agent_id != agent.id
                || entry.seq != event.seq
                || entry.kind != event.kind
                || entry.appended_at != receipt.terminal_at
        }) {
            return Err(unavailable(
                "stale-turn receipt has no matching terminal event",
            ));
        }
        let reservation = self
            .store
            .model_budget
            .find_one(doc! {"_id": &key})
            .read_concern(ReadConcern::snapshot())
            .max_time(READ_DEADLINE)
            .session(&mut session)
            .await
            .map_err(unavailable)?;
        match (receipt.budget_key, reservation) {
            (Some(_), Some(row)) => {
                budget::validate_stale_turn_reservation(&row, agent, turn, true)
                    .map_err(store_error)?
            }
            (None, None) => {}
            _ => {
                return Err(unavailable(
                    "stale-turn receipt has no matching budget state",
                ))
            }
        }
        Ok(true)
    }
}

fn terminal_event(proof: &StaleTurnSettlement) -> Result<(u64, &AgentLogEntry), ApplyError> {
    let turn = proof
        .snapshot()
        .agent
        .conversation
        .active_turn
        .ok_or(CONFLICT)?;
    proof
        .checkpoint()
        .outcome
        .appended
        .iter()
        .find(|event| {
            matches!(
                &event.kind, AgentLogKind::TurnFailed {turn_sequence, ..} if *turn_sequence == turn
            )
        })
        .map(|event| (turn, event))
        .ok_or(CONFLICT)
}

fn store_error(error: ApplyError) -> SandboxStoreError {
    match error {
        ApplyError::Domain(error) => error,
        ApplyError::Driver(error) if is_transient(&error) => SandboxStoreError::Conflict,
        ApplyError::Driver(error) => unavailable(error),
    }
}

fn unavailable(error: impl std::fmt::Display) -> SandboxStoreError {
    SandboxStoreError::Unavailable(error.to_string())
}

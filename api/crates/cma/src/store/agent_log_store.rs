//! Public log port backed by bounded snapshot readers and atomic append.
use super::agent_log::{append_in_transaction, reconcile_append_after_unknown};
use super::agent_log_integrity::TransactionLogError;
use super::agent_log_reads::scan_read_range;
use super::sandbox_mongo::{
    commit_transaction, is_transient, start_transaction, wait_for_reconciliation, CommitOutcome,
    MongoSandboxStore, TRANSACTION_ATTEMPTS,
};
use layer2_core::ids::AgentId;
use layer2_core::sandbox::store::{AgentLogStore, SandboxStoreError, MAX_PAGE_SIZE};
use layer2_core::sandbox::{AgentLogEntry, AgentLogKind};

#[async_trait::async_trait]
impl AgentLogStore for MongoSandboxStore {
    async fn agent_log_current_state(
        &self,
        agent: &layer2_core::sandbox::Agent,
        after_approval: u64,
        limit: u16,
    ) -> Result<Option<layer2_core::sandbox::store::AgentLogCurrentState>, SandboxStoreError> {
        super::agent_log_segments::current_state::read(self, agent, after_approval, limit).await
    }
    async fn agent_log_turn_at(
        &self,
        agent: &layer2_core::sandbox::Agent,
        through: u64,
    ) -> Result<layer2_core::sandbox::AgentLogTurnLookup, SandboxStoreError> {
        super::agent_log_segments::history_context::turn_at(self, agent, through).await
    }

    async fn agent_log_projection(
        &self,
        agent: &layer2_core::sandbox::Agent,
    ) -> Result<Option<layer2_core::sandbox::AgentLogProjectionState>, SandboxStoreError> {
        super::agent_log_segments::read_projection(self, agent).await
    }

    async fn agent_log_final_answer(
        &self,
        agent: &layer2_core::sandbox::Agent,
        sequence: u64,
    ) -> Result<layer2_core::sandbox::AgentLogAnswerLookup, SandboxStoreError> {
        super::agent_log_segments::final_answer(self, agent, sequence).await
    }

    async fn append_agent_log(
        &self,
        agent_id: &AgentId,
        appended_at: String,
        kind: AgentLogKind,
    ) -> Result<AgentLogEntry, SandboxStoreError> {
        let mut last_transient = None;
        for attempt in 0..TRANSACTION_ATTEMPTS {
            if attempt > 0 {
                tokio::task::yield_now().await;
            }
            let mut session = match self.client.start_session().await {
                Ok(session) => session,
                Err(error) if is_transient(&error) => {
                    last_transient = Some(error.to_string());
                    continue;
                }
                Err(error) => return Err(SandboxStoreError::Unavailable(error.to_string())),
            };
            if let Err(error) = start_transaction(&mut session).await {
                if is_transient(&error) {
                    last_transient = Some(error.to_string());
                    continue;
                }
                return Err(SandboxStoreError::Unavailable(error.to_string()));
            }
            let receipt = match append_in_transaction(
                self,
                &mut session,
                agent_id,
                appended_at.clone(),
                kind.clone(),
            )
            .await
            {
                Ok(entry) => entry,
                Err(TransactionLogError::Domain(error)) => {
                    let _ = session.abort_transaction().await;
                    return Err(error);
                }
                Err(TransactionLogError::Driver(error)) if is_transient(&error) => {
                    let _ = session.abort_transaction().await;
                    last_transient = Some(error.to_string());
                    continue;
                }
                Err(TransactionLogError::Driver(error)) => {
                    let _ = session.abort_transaction().await;
                    return Err(SandboxStoreError::Unavailable(error.to_string()));
                }
            };
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(receipt.entry),
                CommitOutcome::Retry(error) => last_transient = Some(error.to_string()),
                CommitOutcome::Unknown(error) => {
                    let snapshot_time = session.operation_time().ok_or_else(|| {
                        SandboxStoreError::Unavailable(format!(
                            "agent log commit remained unknown without an operation time: {error}"
                        ))
                    })?;
                    let commit_error = error.to_string();
                    let unresolved_error = commit_error.clone();
                    let store = self.clone();
                    return wait_for_reconciliation("agent log commit", async move {
                        reconcile_append_after_unknown(&store, &receipt, snapshot_time)
                            .await?
                            .ok_or_else(|| {
                                SandboxStoreError::Unavailable(format!(
                                    "agent log commit remained unknown: {unresolved_error}"
                                ))
                            })
                    })
                    .await
                    .map_err(|deadline| {
                        SandboxStoreError::Unavailable(format!(
                            "{deadline}; ambiguous commit: {commit_error}"
                        ))
                    })?;
                }
                CommitOutcome::Failed(error) => {
                    return Err(SandboxStoreError::Unavailable(error.to_string()));
                }
            }
        }
        Err(SandboxStoreError::Unavailable(format!(
            "agent log transaction exhausted its retry budget: {}",
            last_transient.unwrap_or_else(|| "no driver result".into())
        )))
    }

    async fn agent_log(
        &self,
        agent_id: &AgentId,
        after: u64,
        limit: u16,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
        if limit == 0 || limit > MAX_PAGE_SIZE {
            return Err(SandboxStoreError::InvalidPage);
        }
        let mut page = Vec::with_capacity(usize::from(limit));
        scan_read_range(self, agent_id, after, None, limit, |entry| {
            page.push(entry);
            Ok(())
        })
        .await?;
        Ok(page)
    }

    async fn agent_log_range(
        &self,
        agent_id: &AgentId,
        after: u64,
        through: u64,
        limit: u16,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
        if limit == 0 || limit > MAX_PAGE_SIZE || after > through {
            return Err(SandboxStoreError::InvalidPage);
        }
        let mut page = Vec::new();
        scan_read_range(self, agent_id, after, Some(through), limit, |entry| {
            page.push(entry);
            Ok(())
        })
        .await?;
        Ok(page)
    }

    async fn visit_agent_log_range(
        &self,
        agent_id: &AgentId,
        after: u64,
        through: u64,
        limit: u16,
        visit: &mut (dyn FnMut(AgentLogEntry) -> Result<(), SandboxStoreError> + Send),
    ) -> Result<(), SandboxStoreError> {
        if limit == 0 || limit > MAX_PAGE_SIZE || after > through {
            return Err(SandboxStoreError::InvalidPage);
        }
        scan_read_range(self, agent_id, after, Some(through), limit, visit).await
    }

    async fn agent_log_entry(
        &self,
        agent_id: &AgentId,
        sequence: u64,
    ) -> Result<Option<AgentLogEntry>, SandboxStoreError> {
        super::agent_log_reads::read_point(self, agent_id, sequence).await
    }

    async fn visit_agent_log_page_with_predecessor(
        &self,
        agent_id: &AgentId,
        after: u64,
        through: u64,
        limit: u16,
        visit: &mut (dyn FnMut(AgentLogEntry) -> Result<(), SandboxStoreError> + Send),
    ) -> Result<(), SandboxStoreError> {
        if limit == 0 || limit > MAX_PAGE_SIZE || after > through {
            return Err(SandboxStoreError::InvalidPage);
        }
        let predecessor = u16::from(after > 0);
        scan_read_range(
            self,
            agent_id,
            after.saturating_sub(1),
            Some(through),
            limit + predecessor,
            visit,
        )
        .await
    }

    async fn agent_log_prefix(
        &self,
        agent_id: &AgentId,
        through_seq: u64,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
        if through_seq > 10_128 {
            return Err(SandboxStoreError::InvalidPage);
        }
        let mut entries = Vec::new();
        // The legacy reader checks its entire bounded snapshot; segmented
        // compatibility validates the requested prefix against immutable seals.
        self.visit_agent_log_prefix(agent_id, through_seq, &mut |entry| {
            entries.push(entry);
            Ok(())
        })
        .await?;
        if u64::try_from(entries.len()).ok() != Some(through_seq) {
            return Err(SandboxStoreError::SequenceConflict);
        }
        Ok(entries)
    }

    async fn visit_agent_log_prefix(
        &self,
        agent_id: &AgentId,
        through_seq: u64,
        visit: &mut (dyn FnMut(AgentLogEntry) -> Result<(), SandboxStoreError> + Send),
    ) -> Result<(), SandboxStoreError> {
        if through_seq > 10_128 {
            return Err(SandboxStoreError::InvalidPage);
        }
        scan_read_range(
            self,
            agent_id,
            0,
            Some(through_seq),
            (through_seq as u16).max(1),
            visit,
        )
        .await
    }
}

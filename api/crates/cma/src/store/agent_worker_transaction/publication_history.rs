//! Narrow trusted-history selection for ordinary failed CloseProcess retries.
use super::*;
use crate::store::agent_log_integrity::{
    trusted_summary, AppendCapacity, LogSummary, NORMAL_AGENT_LOG_ENTRIES,
};
use layer2_core::sandbox::AgentLogKind;

pub(super) struct History {
    pub(super) summary: LogSummary,
    pub(super) entries: Vec<AgentLogEntry>,
    pub(super) last: Option<AgentLogEntry>,
}

impl MongoSandboxStore {
    pub(crate) fn with_bounded_close_retries(mut self, enabled: bool) -> Self {
        self.bounded_close_retries = enabled;
        self
    }
}

pub(super) async fn read(
    store: &MongoSandboxStore,
    current: &Agent,
    row: &Document,
    mutation: &AgentWorkMutation,
    ordinary: bool,
    session: &mut mongodb::ClientSession,
) -> Result<History, ApplyError> {
    let needs_marker = agent_cleanup_capacity_retry(current, mutation);
    let caller = if needs_marker
        || crate::store::agent_worker_capacity::capacity_failure_evidence_matches(current, mutation)
    {
        ScanCaller::CapacitySettlement(current.work.step)
    } else {
        ScanCaller::WorkerPublication(current.work.step)
    };
    if ordinary
        && store.bounded_close_retries
        && store.bounded_log_enabled(&current.id)
        && eligible(current, mutation)
    {
        if let Some(summary) = trusted_summary(store, current, row, caller, session, false).await? {
            return Ok(History {
                summary,
                entries: vec![],
                last: None,
            });
        }
    }
    // Capacity/cleanup markers, successful effects and all other publications
    // keep their complete audit. A missing/untrusted proof also follows it.
    let mut entries = Vec::new();
    let mut last = None;
    let summary = scan_complete_log_with_session(
        caller,
        store,
        &current.id,
        current.conversation.last_seq,
        session,
        |entry| {
            entries.push(entry.clone());
            if needs_marker && entry.seq == current.conversation.last_seq {
                last = Some(entry);
            }
            Ok(())
        },
    )
    .await?;
    Ok(History {
        summary,
        entries,
        last,
    })
}

fn eligible(current: &Agent, mutation: &AgentWorkMutation) -> bool {
    // This is selection, never mutation authority. The unchanged publication
    // validator and Agent CAS still fence lease, revision, work and identity.
    current.work.step == AgentWorkStep::CloseProcess
        && mutation.agent.work.step == AgentWorkStep::CloseProcess
        && current.status == mutation.agent.status
        && current.conversation.last_seq < NORMAL_AGENT_LOG_ENTRIES
        && mutation.worktree.is_none()
        && mutation.sandbox_event.is_none()
        && publication_capacity(current, mutation) == AppendCapacity::Normal
        && matches!(
            mutation.agent_logs.as_slice(),
            [AgentLogKind::WorkStepFailed {
                step: AgentWorkStep::CloseProcess,
                ..
            }]
        )
}

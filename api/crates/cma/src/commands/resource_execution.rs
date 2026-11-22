//! Drive retained revocations through exact participant evidence. Production
//! composition requires qualified receiver and transport audience producers.

mod participants;
mod reconciliation;
mod revocations;
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "R3 sweep awaits qualified production composition")
)]
mod sweep;

use std::sync::Arc;

use layer2_core::resource_access::execution::{
    ExecutionError, ExecutionResult, ResourceExecutionStore, RevocationPhaseV1, MAX_PAGE_MEMBERS,
};
use layer3_adapter::resource_execution_protocol as wire;
use participants::ParticipantDirectory;

const PARTICIPANT_CONCURRENCY: usize = 16;

pub(crate) struct RevocationWorker {
    store: Arc<dyn ResourceExecutionStore>,
    participants: Arc<dyn ParticipantDirectory>,
    participant_slots: tokio::sync::Semaphore,
}

pub(crate) struct PendingPage {
    pub operations: Vec<wire::Digest>,
    pub next: Option<wire::Digest>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Progress {
    Pending {
        completed: u32,
        total: u32,
        deadline_exceeded: bool,
        failures: Vec<ExecutionError>,
    },
    Effective,
}

impl RevocationWorker {
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "R3 production audience providers are not yet qualified"
        )
    )]
    pub(crate) fn new(
        store: Arc<dyn ResourceExecutionStore>,
        participants: Arc<dyn ParticipantDirectory>,
    ) -> Self {
        Self {
            store,
            participants,
            participant_slots: tokio::sync::Semaphore::new(PARTICIPANT_CONCURRENCY),
        }
    }

    pub(crate) async fn discover(
        &self,
        after: Option<&wire::Digest>,
    ) -> ExecutionResult<PendingPage> {
        let rows = self
            .store
            .pending_revocations(after, MAX_PAGE_MEMBERS)
            .await?;
        if rows.len() > MAX_PAGE_MEMBERS as usize {
            return Err(ExecutionError::Corrupt);
        }
        let mut previous = after;
        for row in &rows {
            row.validate()?;
            if !matches!(row.phase, RevocationPhaseV1::Pending)
                || previous.is_some_and(|id| id.as_str() >= row.key.operation_id.as_str())
            {
                return Err(ExecutionError::Corrupt);
            }
            previous = Some(&row.key.operation_id);
        }
        let next = (rows.len() == MAX_PAGE_MEMBERS as usize)
            .then(|| rows.last().unwrap().key.operation_id.clone());
        Ok(PendingPage {
            operations: rows.into_iter().map(|row| row.key.operation_id).collect(),
            next,
        })
    }
}

#[cfg(test)]
mod tests;

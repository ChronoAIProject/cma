//! Bounded semantic-row readers for the format-2 Agent log.
//!
//! The segment reader validates every source row and seal in the caller's
//! snapshot. Consumers retain only rows that can affect their fold, so a long
//! history never becomes a second complete-log allocation. Legacy callers keep
//! their existing proof and fallback path.

use layer2_core::sandbox::{store::SandboxStoreError, Agent, AgentLogEntry, AgentLogKind};
use mongodb::ClientSession;

use super::{
    agent_log_reads::integrity::{LogReadIntegrity, PreparedIntegrity},
    agent_log_segments::{self as segments, LogRange},
    sandbox_mongo::MongoSandboxStore,
};

const PAGE_SIZE: u64 = 200;

/// Read selected rows from a trusted segmented history in one snapshot.
///
/// `None` is returned for legacy histories so the caller can use its existing
/// compatibility reader. Every row in each requested page is still validated
/// by the segment reader; filtering only changes the retained output.
pub(super) async fn selected<F>(
    store: &MongoSandboxStore,
    expected: &Agent,
    session: &mut ClientSession,
    mut keep: F,
) -> Result<Option<Vec<AgentLogEntry>>, SandboxStoreError>
where
    F: FnMut(&AgentLogKind) -> bool + Send,
{
    selected_until(store, expected, session, keep, |_| false).await
}

/// Variant for consumers whose exact fold has a terminal prefix (for example,
/// first-turn title discovery). Source rows after the terminal result are
/// still validated, but are not retained.
pub(super) async fn selected_until<F, D>(
    store: &MongoSandboxStore,
    expected: &Agent,
    session: &mut ClientSession,
    mut keep: F,
    mut done: D,
) -> Result<Option<Vec<AgentLogEntry>>, SandboxStoreError>
where
    F: FnMut(&AgentLogKind) -> bool + Send,
    D: FnMut(&[AgentLogEntry]) -> bool + Send,
{
    let Some(head) = PreparedIntegrity
        .read_head(store, &expected.id, session)
        .await?
    else {
        return Err(SandboxStoreError::NotFound);
    };
    if head.last_seq != expected.conversation.last_seq {
        return Err(SandboxStoreError::SequenceConflict);
    }
    let Some(segmented) = head.segmented.as_ref() else {
        return Ok(None);
    };

    let mut selected = Vec::new();
    let mut after = 0_u64;
    while after < head.last_seq {
        let through = after
            .checked_add(PAGE_SIZE)
            .unwrap_or(head.last_seq)
            .min(head.last_seq);
        let mut expected_seq = after + 1;
        segments::visit_range(
            store,
            segmented,
            LogRange {
                after,
                through,
                limit: u16::try_from(through - after)
                    .map_err(|_| SandboxStoreError::InvalidPage)?,
            },
            session,
            true,
            |entry| {
                head.validate_lineage(&entry)?;
                if entry.agent_id != expected.id || entry.seq != expected_seq {
                    return Err(SandboxStoreError::SequenceConflict);
                }
                expected_seq = expected_seq
                    .checked_add(1)
                    .ok_or(SandboxStoreError::SequenceConflict)?;
                if keep(&entry.kind) && !done(&selected) {
                    selected.push(entry);
                }
                Ok(())
            },
        )
        .await
        .map_err(segment_error)?;
        if expected_seq != through + 1 {
            return Err(SandboxStoreError::SequenceConflict);
        }
        after = through;
    }
    Ok(Some(selected))
}

fn segment_error(error: super::agent_log_integrity::TransactionLogError) -> SandboxStoreError {
    match error {
        super::agent_log_integrity::TransactionLogError::Domain(error) => error,
        super::agent_log_integrity::TransactionLogError::Driver(error) => {
            SandboxStoreError::Unavailable(error.to_string())
        }
    }
}

//! Bounded semantic-row readers for the format-2 Agent log.
//!
//! The segment reader validates every source row and seal in the caller's
//! snapshot. Consumers retain only rows that can affect their fold, so a long
//! history never becomes a second complete-log allocation. Legacy callers keep
//! their existing proof and fallback path.

use layer2_core::sandbox::{
    store::SandboxStoreError, Agent, AgentLogEntry, AgentLogKind, AgentTranscript,
    AgentUsageReading,
};
use mongodb::{bson::doc, options::ReadConcern, ClientSession};

use super::{
    agent_log_reads::integrity::{LogReadIntegrity, PreparedIntegrity},
    agent_log_segments::{self as segments, LogRange},
    sandbox_mongo::MongoSandboxStore,
};

pub(super) const PAGE_SIZE: u64 = 200;

pub(super) async fn is_segmented(
    store: &MongoSandboxStore,
    expected: &Agent,
    session: &mut ClientSession,
) -> Result<bool, SandboxStoreError> {
    let Some(row) = store
        .agents
        .find_one(doc! {"_id": expected.id.as_str()})
        .read_concern(ReadConcern::snapshot())
        .session(&mut *session)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
    else {
        return Err(SandboxStoreError::NotFound);
    };
    let agent = crate::store::agents::read_agent(&row)?;
    if agent != *expected {
        return Err(SandboxStoreError::SequenceConflict);
    }
    Ok(crate::store::agent_log_segments::read_format(&row)?.is_some())
}

pub(super) async fn projected_usage(
    store: &MongoSandboxStore,
    expected: &Agent,
    session: &mut ClientSession,
) -> Result<Option<AgentUsageReading>, SandboxStoreError> {
    // Capture below validates the head and source in this same snapshot.
    if !is_segmented(store, expected, session).await? {
        return super::agent_opening_reads::usage(store, expected, session).await;
    }
    let source = segments::capture::capture(
        store,
        expected,
        session,
        segments::capture::CaptureMode::Snapshot,
    )
    .await
    .map_err(segment_error)?;
    Ok(Some(AgentUsageReading {
        agent_id: expected.id.clone(),
        through_seq: expected.conversation.last_seq,
        usage: source.projection().usage(),
    }))
}

pub(super) async fn projected_transcript(
    store: &MongoSandboxStore,
    expected: &Agent,
    session: &mut ClientSession,
) -> Result<Option<AgentTranscript>, SandboxStoreError> {
    transcript::complete(store, expected, session).await
}

pub(super) mod answer;
pub(super) mod page;
pub(super) mod reply_preview;
mod transcript;

pub(super) async fn transcript_page(
    store: &MongoSandboxStore,
    agent: &Agent,
    request: &layer2_core::sandbox::AgentTranscriptPageRequest,
    mode: segments::capture::CaptureMode,
) -> Result<Option<layer2_core::sandbox::AgentTranscriptPage>, SandboxStoreError> {
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
    transcript::page_with_capture(store, agent, &mut session, request, mode).await
}

/// Read one bounded source range from a certified format-2 history. A
/// transaction-safe form is selected with `snapshot = false`; a transaction
/// already supplies its
/// captured snapshot, so adding Mongo's snapshot read concern to individual
/// operations would be rejected by the driver.
pub(super) async fn range_with_snapshot(
    store: &MongoSandboxStore,
    expected: &Agent,
    session: &mut ClientSession,
    after: u64,
    through: u64,
    snapshot: bool,
) -> Result<Option<Vec<AgentLogEntry>>, SandboxStoreError> {
    range_transaction(store, expected, session, after, through, snapshot)
        .await
        .map_err(segment_error)
}

pub(super) async fn range_transaction(
    store: &MongoSandboxStore,
    expected: &Agent,
    session: &mut ClientSession,
    after: u64,
    through: u64,
    snapshot: bool,
) -> Result<Option<Vec<AgentLogEntry>>, super::agent_log_integrity::TransactionLogError> {
    if after > through {
        return Err(SandboxStoreError::InvalidPage.into());
    }
    let find = store.agents.find_one(doc! {"_id": expected.id.as_str()});
    let find = if snapshot {
        find.read_concern(ReadConcern::snapshot())
    } else {
        find
    };
    let Some(row) = find
        .session(&mut *session)
        .await
        .map_err(super::agent_log_integrity::TransactionLogError::Driver)?
    else {
        return Err(SandboxStoreError::NotFound.into());
    };
    let agent = crate::store::agents::read_agent(&row)?;
    if agent.id != expected.id || agent.conversation.last_seq != expected.conversation.last_seq {
        return Err(SandboxStoreError::SequenceConflict.into());
    }
    let Some(()) = crate::store::agent_log_segments::read_format(&row)?.map(|_| ()) else {
        return Ok(None);
    };
    let segmented =
        crate::store::agent_log_segments::read_head(store, &row, session, snapshot).await?;
    let through = through.min(agent.conversation.last_seq);
    if through < after {
        return Err(SandboxStoreError::SequenceConflict.into());
    }
    let count = through - after;
    let mut entries = Vec::new();
    let mut expected_seq = after + 1;
    segments::visit_range(
        store,
        &segmented,
        LogRange {
            after,
            through,
            limit: u16::try_from(count.max(1)).map_err(|_| SandboxStoreError::InvalidPage)?,
        },
        session,
        snapshot,
        |entry| {
            validate_lineage(expected, &entry)?;
            if entry.agent_id != expected.id || entry.seq != expected_seq {
                return Err(SandboxStoreError::SequenceConflict);
            }
            expected_seq = expected_seq
                .checked_add(1)
                .ok_or(SandboxStoreError::SequenceConflict)?;
            entries.push(entry);
            Ok(())
        },
    )
    .await?;
    if expected_seq != through + 1 || entries.len() as u64 != count {
        return Err(SandboxStoreError::SequenceConflict.into());
    }
    Ok(Some(entries))
}

fn validate_lineage(expected: &Agent, entry: &AgentLogEntry) -> Result<(), SandboxStoreError> {
    if let AgentLogKind::Created {
        workspace_id,
        worktree_id,
    } = &entry.kind
    {
        if entry.seq != 1
            || workspace_id != &expected.workspace_id
            || worktree_id != expected.creation_worktree_id()
        {
            return Err(SandboxStoreError::Corrupt(
                "Agent creation log lineage does not match".into(),
            ));
        }
    }
    Ok(())
}

/// Read selected rows from a trusted segmented history in one snapshot.
///
/// `None` is returned for legacy histories so the caller can use its existing
/// compatibility reader. Every row in each requested page is still validated
/// by the segment reader; filtering only changes the retained output.
pub(super) async fn selected<F>(
    store: &MongoSandboxStore,
    expected: &Agent,
    session: &mut ClientSession,
    keep: F,
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

    if head.last_seq > PAGE_SIZE {
        return Err(SandboxStoreError::Unavailable(
            "segmented semantic history requires pagination".into(),
        ));
    }
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

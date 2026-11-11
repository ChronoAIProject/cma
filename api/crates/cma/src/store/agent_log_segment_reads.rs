//! Bounded semantic-row readers for the format-2 Agent log.
//!
//! The segment reader validates every source row and seal in the caller's
//! snapshot. Consumers retain only rows that can affect their fold, so a long
//! history never becomes a second complete-log allocation. Legacy callers keep
//! their existing proof and fallback path.

use layer2_core::sandbox::{
    store::SandboxStoreError, Agent, AgentLogEntry, AgentLogKind, AgentTranscript,
    AgentTranscriptFold, AgentUsageReading,
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

async fn segmented_source(
    store: &MongoSandboxStore,
    expected: &Agent,
    session: &mut ClientSession,
    snapshot: bool,
) -> Result<Option<crate::store::agent_log_segments::Head>, SandboxStoreError> {
    let find = store.agents.find_one(doc! {"_id": expected.id.as_str()});
    let find = if snapshot {
        find.read_concern(ReadConcern::snapshot())
    } else {
        find
    };
    let Some(row) = find
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
    if crate::store::agent_log_segments::read_format(&row)?.is_none() {
        return Ok(None);
    }
    let head = crate::store::agent_log_segments::read_head(store, &row, session, snapshot)
        .await
        .map_err(segment_error)?;
    if u64::try_from(head.last_seq).ok() != Some(expected.conversation.last_seq) {
        return Err(SandboxStoreError::SequenceConflict);
    }
    Ok(Some(head))
}

/// Visit only transcript-affecting rows from a certified segmented source.
/// Source pages are validated one at a time; the visitor owns its projection
/// state and no lifetime source vector is retained by this adapter.
pub(super) async fn visit_selected<F, V>(
    store: &MongoSandboxStore,
    expected: &Agent,
    session: &mut ClientSession,
    through: u64,
    mut keep: F,
    mut visit: V,
) -> Result<Option<()>, SandboxStoreError>
where
    F: FnMut(&AgentLogKind) -> bool + Send,
    V: FnMut(AgentLogEntry) -> Result<(), SandboxStoreError> + Send,
{
    let Some(head) = segmented_source(store, expected, session, true).await? else {
        return Ok(None);
    };
    let through = through.min(expected.conversation.last_seq);
    let mut after = 0_u64;
    while after < through {
        let through = after.checked_add(PAGE_SIZE).unwrap_or(through).min(through);
        let mut expected_seq = after + 1;
        segments::visit_range(
            store,
            &head,
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
                if keep(&entry.kind) {
                    visit(entry)?;
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
    Ok(Some(()))
}

pub(super) async fn title_through(
    store: &MongoSandboxStore,
    expected: &Agent,
    session: &mut ClientSession,
) -> Result<Option<u64>, SandboxStoreError> {
    let Some(head) = segmented_source(store, expected, session, true).await? else {
        return Ok(None);
    };
    crate::store::agent_log_segments::projection::title_through(store, expected, &head, session)
        .await
        .map_err(segment_error)
}

pub(super) async fn projected_usage(
    store: &MongoSandboxStore,
    expected: &Agent,
    session: &mut ClientSession,
) -> Result<Option<AgentUsageReading>, SandboxStoreError> {
    let Some(head) = segmented_source(store, expected, session, true).await? else {
        return Ok(None);
    };
    let projection = segments::projection::read(store, expected, &head, session)
        .await
        .map_err(segment_error)?;
    Ok(Some(AgentUsageReading {
        agent_id: expected.id.clone(),
        through_seq: expected.conversation.last_seq,
        usage: projection.usage(),
    }))
}

pub(super) async fn projected_transcript(
    store: &MongoSandboxStore,
    expected: &Agent,
    session: &mut ClientSession,
) -> Result<Option<AgentTranscript>, SandboxStoreError> {
    let Some(head) = segmented_source(store, expected, session, true).await? else {
        return Ok(None);
    };
    segments::projection::read(store, expected, &head, session)
        .await
        .map_err(segment_error)?;
    let mut fold = AgentTranscriptFold::default();
    let result = visit_selected(
        store,
        expected,
        session,
        expected.conversation.last_seq,
        |kind| {
            matches!(
                kind,
                AgentLogKind::Created { .. }
                    | AgentLogKind::ConversationOpened { .. }
                    | AgentLogKind::RuntimeGenerationRetired { .. }
                    | AgentLogKind::TurnStarted { .. }
                    | AgentLogKind::TurnMetadataSet { .. }
                    | AgentLogKind::OwnerMessage { .. }
                    | AgentLogKind::AgentMessage { .. }
                    | AgentLogKind::TurnCompleted { .. }
                    | AgentLogKind::TurnFailed { .. }
                    | AgentLogKind::TurnInterrupted { .. }
                    | AgentLogKind::ConversationGap { .. }
                    | AgentLogKind::OperationDeliveryChanged {
                        operation: layer2_core::sandbox::AgentOperationKind::Steer,
                        state: layer2_core::sandbox::AgentOperationDelivery::DispatchFenced,
                        ..
                    }
            )
        },
        |entry| {
            fold.push(expected, entry)
                .map_err(SandboxStoreError::Corrupt)
        },
    )
    .await?;
    if result.is_none() {
        return Ok(None);
    }
    fold.finish(expected)
        .map(Some)
        .map_err(SandboxStoreError::Corrupt)
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
    if after > through {
        return Err(SandboxStoreError::InvalidPage);
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
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
    else {
        return Err(SandboxStoreError::NotFound);
    };
    let agent = crate::store::agents::read_agent(&row)?;
    if agent.id != expected.id || agent.conversation.last_seq != expected.conversation.last_seq {
        return Err(SandboxStoreError::SequenceConflict);
    }
    let Some(()) = crate::store::agent_log_segments::read_format(&row)?.map(|_| ()) else {
        return Ok(None);
    };
    let segmented = crate::store::agent_log_segments::read_head(store, &row, session, snapshot)
        .await
        .map_err(segment_error)?;
    let through = through.min(agent.conversation.last_seq);
    if through < after {
        return Err(SandboxStoreError::SequenceConflict);
    }
    let count = through - after;
    if count == 0 {
        return Ok(Some(Vec::new()));
    }
    let mut entries = Vec::new();
    let mut expected_seq = after + 1;
    segments::visit_range(
        store,
        &segmented,
        LogRange {
            after,
            through,
            limit: u16::try_from(count).map_err(|_| SandboxStoreError::InvalidPage)?,
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
    .await
    .map_err(segment_error)?;
    if expected_seq != through + 1 || entries.len() as u64 != count {
        return Err(SandboxStoreError::SequenceConflict);
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

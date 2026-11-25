use layer2_core::ids::AgentId;
use layer2_core::sandbox::{store::SandboxStoreError, AgentLogEntry, AgentLogKind};
use mongodb::{
    bson::{doc, Document, RawDocumentBuf},
    options::ReadConcern,
    ClientSession, Database,
};

use super::{
    corrupt, digest, read_format, Head, Seal, AUDITS, HEADS, MAX_BYTES, MAX_ENTRIES, MAX_SEQUENCE,
    SEGMENTS,
};
use crate::store::{
    agent_log::{log_id, read_entry},
    agent_log_integrity::{TransactionLogError, QUARANTINE_FIELD},
    durability::majority_collection,
    sandbox_mongo::{MongoSandboxStore, AGENT_LOG},
};

type Result<T> = std::result::Result<T, TransactionLogError>;

pub(in crate::store) struct LogRange {
    pub after: u64,
    pub through: u64,
    pub limit: u16,
}

mod metadata;
pub(in crate::store) use metadata::read_head;
pub(super) use metadata::{
    containing, preview_containing, preview_head, preview_seal_at, read_export_head, seal_at,
};
use metadata::{containing_from, read_head_from, seal_at_from, validate_unsealed_creation};

/// Validate all requested rows even when the caller has filled its projected
/// byte budget. Corruption in an undelivered source row must not be truncation.
pub(in crate::store) async fn visit_range<F>(
    store: &MongoSandboxStore,
    head: &Head,
    range: LogRange,
    session: &mut ClientSession,
    snapshot: bool,
    visit: F,
) -> Result<()>
where
    F: FnMut(AgentLogEntry) -> std::result::Result<(), SandboxStoreError> + Send,
{
    visit_range_from(&database(store), head, range, session, snapshot, visit).await
}

async fn visit_range_from<F>(
    db: &Database,
    head: &Head,
    range: LogRange,
    session: &mut ClientSession,
    snapshot: bool,
    mut visit: F,
) -> Result<()>
where
    F: FnMut(AgentLogEntry) -> std::result::Result<(), SandboxStoreError> + Send,
{
    let after = i64::try_from(range.after).map_err(|_| corrupt())?;
    let through = i64::try_from(range.through).map_err(|_| corrupt())?;
    if after > through || through > head.last_seq || through > MAX_SEQUENCE {
        return Err(SandboxStoreError::SequenceConflict.into());
    }
    let end = after + (through - after).min(i64::from(range.limit));
    // A retained head can outlive source writes in the same transaction.
    super::source_keys::prove(db, head, session, snapshot).await?;
    let mut next = after + 1;
    while next <= end && next < head.active_start_seq {
        let seal = containing_from(db, head, next, session, snapshot, None).await?;
        let stop = end.min(seal.end_seq);
        scan_rows(
            db,
            head,
            next..=stop,
            Some(&seal),
            session,
            snapshot,
            &mut visit,
        )
        .await?;
        next = stop + 1;
    }
    if next <= end || after == end {
        if head.active_ordinal > 0 {
            seal_at_from(db, head, head.active_ordinal - 1, session, snapshot, None)
                .await?
                .precedes(head)?;
        }
        let mut selected = |entry: AgentLogEntry| {
            if entry.seq > after as u64 && entry.seq <= end as u64 {
                visit(entry)?;
            }
            Ok(())
        };
        let bytes = if head.active_count == 0 {
            0
        } else {
            scan_rows(
                db,
                head,
                head.active_start_seq..=head.last_seq,
                None,
                session,
                snapshot,
                &mut selected,
            )
            .await?
        };
        if bytes != head.active_bytes {
            return Err(corrupt().into());
        }
    }
    Ok(())
}

async fn scan_rows<F>(
    db: &Database,
    head: &Head,
    range: std::ops::RangeInclusive<i64>,
    seal: Option<&Seal>,
    session: &mut ClientSession,
    snapshot: bool,
    visit: &mut F,
) -> Result<i64>
where
    F: FnMut(AgentLogEntry) -> std::result::Result<(), SandboxStoreError> + Send,
{
    let (start, end) = (*range.start(), *range.end());
    if start > end || end - start + 1 > MAX_ENTRIES {
        return Err(corrupt().into());
    }
    let collection = majority_collection::<RawDocumentBuf>(db, AGENT_LOG);
    let mut read = crate::store::mongo_read_telemetry::Read::find(collection.name());
    let find = collection
        .find(doc! {"agent_id":&head.agent_id,"seq":{"$gte":start,"$lte":end}})
        .sort(doc! {"seq":1})
        .limit(MAX_ENTRIES + 1)
        // Bound the driver batch by 64 rows and Mongo's 16-MiB reply ceiling.
        // One-row batches make a full active tail require 10,128 round trips.
        .batch_size(64);
    let find = if snapshot {
        find.read_concern(ReadConcern::snapshot())
    } else {
        find
    };
    let mut cursor = find
        .session(&mut *session)
        .await
        .map_err(TransactionLogError::Driver)?;
    let mut expected = start;
    let mut bytes = 0_i64;
    while let Some(raw) = read
        .next_in_session(&mut cursor, session)
        .await
        .map_err(TransactionLogError::Driver)?
    {
        let entry = read_entry(&Document::try_from(&raw).map_err(|_| corrupt())?)?;
        if entry.agent_id != AgentId(head.agent_id.clone())
            || i64::try_from(entry.seq).ok() != Some(expected)
            || expected > end
        {
            return Err(SandboxStoreError::SequenceConflict.into());
        }
        if (expected == 1) != matches!(entry.kind, AgentLogKind::Created { .. }) {
            return Err(corrupt().into());
        }
        let size = i64::try_from(raw.as_bytes().len()).map_err(|_| corrupt())?;
        if size > 16 * 1024 * 1024 {
            return Err(corrupt().into());
        }
        bytes = bytes
            .checked_add(size)
            .filter(|n| *n <= MAX_BYTES)
            .ok_or_else(corrupt)?;
        if let Some(seal) = seal {
            let actual = digest::leaf(&head.agent_id, &head.incarnation, expected, raw.as_bytes());
            digest::check_leaf(&seal.leaf_digests, expected - seal.start_seq, &actual)?;
        }
        visit(entry)?;
        expected += 1;
    }
    if expected != end + 1 {
        return Err(SandboxStoreError::SequenceConflict.into());
    }
    read.complete();
    Ok(bytes)
}

fn database(store: &MongoSandboxStore) -> Database {
    store.client.database(&store.agent_log.namespace().db)
}

/// Verify just the durable turn's original start in the caller's snapshot.
/// No projection output, observation lease, or execution store is opened.
pub(in crate::store) async fn retained_turn(
    db: &Database,
    aggregate: &Document,
    turn: u64,
    session: &mut ClientSession,
) -> Result<bool> {
    let agent = crate::store::agents::read_agent(aggregate)?;
    if aggregate.contains_key(QUARANTINE_FIELD) {
        return Err(corrupt().into());
    }
    if turn == 0 || turn > agent.conversation.last_seq {
        return Ok(false);
    }
    let entry = if read_format(aggregate)?.is_some() {
        let head = read_head_from(db, aggregate, session, true, None, None).await?;
        let mut entry = None;
        visit_range_from(
            db,
            &head,
            LogRange {
                after: turn - 1,
                through: turn,
                limit: 1,
            },
            session,
            true,
            |row| {
                entry = Some(row);
                Ok(())
            },
        )
        .await?;
        entry
    } else {
        validate_unsealed_creation(db, &agent, session, true, None).await?;
        majority_collection(db, AGENT_LOG)
            .find_one(doc! {"_id": log_id(&agent.id, turn)})
            .read_concern(ReadConcern::snapshot())
            .session(session)
            .await
            .map_err(TransactionLogError::Driver)?
            .as_ref()
            .map(read_entry)
            .transpose()?
    };
    Ok(entry.is_some_and(|entry| {
        entry.agent_id == agent.id
            && entry.seq == turn
            && matches!(entry.kind, AgentLogKind::TurnStarted { turn_sequence, .. } if turn_sequence == turn)
    }))
}

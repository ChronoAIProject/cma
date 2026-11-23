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

#[path = "reads_ingress.rs"]
mod ingress;

type Result<T> = std::result::Result<T, TransactionLogError>;

pub(in crate::store) struct LogRange {
    pub after: u64,
    pub through: u64,
    pub limit: u16,
}

pub(in crate::store) async fn read_head(
    store: &MongoSandboxStore,
    aggregate: &Document,
    session: &mut ClientSession,
    snapshot: bool,
) -> Result<Head> {
    read_head_from(&database(store), aggregate, session, snapshot, None).await
}

pub(super) async fn read_export_head(
    store: &MongoSandboxStore,
    aggregate: &Document,
    session: &mut ClientSession,
    max_entries: i64,
    max_bytes: i64,
) -> Result<Head> {
    read_head_from(
        &database(store),
        aggregate,
        session,
        true,
        Some((max_entries, max_bytes)),
    )
    .await
}

async fn read_head_from(
    db: &Database,
    aggregate: &Document,
    session: &mut ClientSession,
    snapshot: bool,
    tail_budget: Option<(i64, i64)>,
) -> Result<Head> {
    let agent = crate::store::agents::read_agent(aggregate)?;
    if aggregate.contains_key(QUARANTINE_FIELD) {
        return Err(corrupt().into());
    }
    let (incarnation, revision) = read_format(aggregate)?.ok_or_else(corrupt)?;
    let audits = majority_collection::<Document>(db, AUDITS);
    let find = audits
        .find_one(doc! {"agent_id":agent.id.as_str(),"incarnation":incarnation,"status":"corrupt"})
        .projection(doc! {"_id":1});
    let find = if snapshot {
        find.read_concern(ReadConcern::snapshot())
    } else {
        find
    };
    if find
        .session(&mut *session)
        .await
        .map_err(TransactionLogError::Driver)?
        .is_some()
    {
        return Err(SandboxStoreError::Corrupt(
            "Agent log is quarantined by an integrity audit".into(),
        )
        .into());
    }

    let heads = majority_collection(db, HEADS);
    let find = heads.find_one(doc! {"_id":agent.id.as_str()});
    let find = if snapshot {
        find.read_concern(ReadConcern::snapshot())
    } else {
        find
    };
    let head = Head::read(
        find.session(&mut *session)
            .await
            .map_err(TransactionLogError::Driver)?
            .ok_or_else(corrupt)?,
    )?;
    if head.agent_id != agent.id.as_str()
        || head.incarnation != incarnation
        || head.revision != revision
        || u64::try_from(head.last_seq).ok() != Some(agent.conversation.last_seq)
    {
        return Err(corrupt().into());
    }
    if tail_budget
        .is_some_and(|(entries, bytes)| head.active_count > entries || head.active_bytes > bytes)
    {
        return Err(SandboxStoreError::AgentLogCapacityExceeded.into());
    }
    let seals = majority_collection::<Document>(db, SEGMENTS);
    let find = seals.find_one(doc! {
        "agent_id":&head.agent_id,"incarnation":&head.incarnation,"ordinal":{"$gte":head.active_ordinal},
    }).projection(doc! {"_id":1});
    let find = if snapshot {
        find.read_concern(ReadConcern::snapshot())
    } else {
        find
    };
    if find
        .session(&mut *session)
        .await
        .map_err(TransactionLogError::Driver)?
        .is_some()
    {
        return Err(corrupt().into());
    }
    if head.active_ordinal == 0 && head.last_seq > 0 {
        validate_unsealed_creation(db, &agent, session, snapshot).await?;
    }
    super::source_keys::prove(db, &head, session, snapshot).await?;
    Ok(head)
}

/// Until creation belongs to an immutable seal, even a page that does not
/// deliver that row must bind its lineage to the captured Agent.
async fn validate_unsealed_creation(
    db: &Database,
    agent: &layer2_core::sandbox::Agent,
    session: &mut ClientSession,
    snapshot: bool,
) -> Result<()> {
    let source = majority_collection::<RawDocumentBuf>(db, AGENT_LOG);
    let mut read = crate::store::mongo_read_telemetry::Read::find(source.name());
    let query = source.find_one(doc! {"agent_id":agent.id.as_str(),"seq":1_i64});
    let query = if snapshot {
        query.read_concern(ReadConcern::snapshot())
    } else {
        query
    };
    let raw = query
        .session(session)
        .await
        .map_err(TransactionLogError::Driver)?
        .ok_or_else(corrupt)?;
    read.document(raw.as_bytes().len());
    let entry = read_entry(&Document::try_from(&raw).map_err(|_| corrupt())?)?;
    if entry.agent_id != agent.id
        || entry.seq != 1
        || !matches!(&entry.kind,
        AgentLogKind::Created { workspace_id, worktree_id }
        if workspace_id == &agent.workspace_id && worktree_id == agent.creation_worktree_id())
    {
        return Err(corrupt().into());
    }
    read.complete();
    Ok(())
}

pub(super) async fn seal_at(
    store: &MongoSandboxStore,
    head: &Head,
    ordinal: i64,
    session: &mut ClientSession,
    snapshot: bool,
) -> Result<Seal> {
    seal_at_from(&database(store), head, ordinal, session, snapshot).await
}

async fn seal_at_from(
    db: &Database,
    head: &Head,
    ordinal: i64,
    session: &mut ClientSession,
    snapshot: bool,
) -> Result<Seal> {
    let seals = majority_collection(db, SEGMENTS);
    let find = seals.find_one(doc! {
        "agent_id": &head.agent_id, "incarnation": &head.incarnation, "ordinal":ordinal,
    });
    let find = if snapshot {
        find.read_concern(ReadConcern::snapshot())
    } else {
        find
    };
    let seal = Seal::read(
        find.session(session)
            .await
            .map_err(TransactionLogError::Driver)?
            .ok_or_else(corrupt)?,
    )?;
    if seal.agent_id != head.agent_id
        || seal.incarnation != head.incarnation
        || seal.ordinal != ordinal
    {
        return Err(corrupt().into());
    }
    Ok(seal)
}

pub(super) async fn containing(
    store: &MongoSandboxStore,
    head: &Head,
    seq: i64,
    session: &mut ClientSession,
    snapshot: bool,
) -> Result<Seal> {
    containing_from(&database(store), head, seq, session, snapshot).await
}

async fn containing_from(
    db: &Database,
    head: &Head,
    seq: i64,
    session: &mut ClientSession,
    snapshot: bool,
) -> Result<Seal> {
    let seals = majority_collection(db, SEGMENTS);
    let find = seals
        .find_one(doc! {
            "agent_id": &head.agent_id, "incarnation": &head.incarnation, "start_seq":{"$lte":seq},
        })
        .sort(doc! {"start_seq":-1});
    let find = if snapshot {
        find.read_concern(ReadConcern::snapshot())
    } else {
        find
    };
    let seal = Seal::read(
        find.session(&mut *session)
            .await
            .map_err(TransactionLogError::Driver)?
            .ok_or_else(corrupt)?,
    )?;
    validate_containing(&seal, head, seq)?;
    if seal.ordinal > 0 {
        let previous = seal_at_from(db, head, seal.ordinal - 1, session, snapshot).await?;
        seal.follows(&previous)?;
    }
    if seal.ordinal + 1 == head.active_ordinal {
        seal.precedes(head)?;
    } else {
        let next = seal_at_from(db, head, seal.ordinal + 1, session, snapshot).await?;
        next.follows(&seal)?;
    }
    Ok(seal)
}

fn validate_containing(
    seal: &Seal,
    head: &Head,
    seq: i64,
) -> std::result::Result<(), SandboxStoreError> {
    if seal.agent_id != head.agent_id
        || seal.incarnation != head.incarnation
        || seal.start_seq > seq
        || seal.end_seq < seq
        || seal.ordinal >= head.active_ordinal
    {
        return Err(corrupt());
    }
    Ok(())
}

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
        let seal = containing_from(db, head, next, session, snapshot).await?;
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
            seal_at_from(db, head, head.active_ordinal - 1, session, snapshot)
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
        let head = read_head_from(db, aggregate, session, true, None).await?;
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
        validate_unsealed_creation(db, &agent, session, true).await?;
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

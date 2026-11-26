use layer2_core::sandbox::{store::SandboxStoreError, AgentLogEntry, AgentLogKind};
use mongodb::{
    bson::{doc, Binary, Document, RawDocumentBuf},
    options::ReadConcern,
    ClientSession, Database,
};

use super::{corrupt, read_format, Head, Seal, AUDITS, HEADS, MAX_SEQUENCE, SEGMENTS};
use crate::store::{
    agent_log::{log_id, read_entry},
    agent_log_integrity::{TransactionLogError, QUARANTINE_FIELD},
    durability::majority_collection,
    sandbox_mongo::{MongoSandboxStore, AGENT_LOG},
};

type Result<T> = std::result::Result<T, TransactionLogError>;

pub(in crate::store) async fn verify_terminal_seal(
    store: &MongoSandboxStore,
    head: &Head,
    session: &mut ClientSession,
) -> Result<()> {
    if head.active_ordinal > 0 {
        metadata::seal_at(store, head, head.active_ordinal - 1, session, true)
            .await?
            .precedes(head)?;
    }
    Ok(())
}

pub(in crate::store) struct LogRange {
    pub after: u64,
    pub through: u64,
    pub limit: u16,
}

pub(super) mod metadata;
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
    let end = range
        .through
        .min(range.after.saturating_add(u64::from(range.limit)));
    let leaves =
        if store.active_log_proofs && (end >= head.active_start_seq as u64 || range.after == end) {
            let row = crate::store::stale_turns::find_with_proof_budget(
                &store.agents,
                doc! {"_id":&head.agent_id},
                session,
                snapshot,
            )
            .await
            .map_err(TransactionLogError::Driver)?
            .ok_or_else(corrupt)?;
            let agent = crate::store::agents::read_agent(&row)?;
            if read_format(&row)? != Some((head.incarnation.as_str(), head.revision))
                || agent.conversation.last_seq != head.last_seq as u64
            {
                return Err(corrupt().into());
            }
            super::active_proof::read(store, head, agent.revision, session, snapshot).await?
        } else {
            None
        };
    visit_range_from(
        &database(store),
        head,
        range,
        session,
        snapshot,
        leaves.as_ref(),
        visit,
    )
    .await
}

async fn visit_range_from<F>(
    db: &Database,
    head: &Head,
    range: LogRange,
    session: &mut ClientSession,
    snapshot: bool,
    active_leaves: Option<&Binary>,
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
            Some((&seal.leaf_digests, seal.start_seq)),
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
        if let Some(leaves) = active_leaves {
            if head.active_count > 0 {
                // An empty page still witnesses the current original tail row.
                let selected_range = if next <= end {
                    next..=end
                } else {
                    head.last_seq..=head.last_seq
                };
                scan_rows(
                    db,
                    head,
                    selected_range,
                    Some((leaves, head.active_start_seq)),
                    session,
                    snapshot,
                    &mut selected,
                )
                .await?;
            }
            return Ok(());
        }
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
    proof: Option<(&Binary, i64)>,
    session: &mut ClientSession,
    snapshot: bool,
    visit: &mut F,
) -> Result<i64>
where
    F: FnMut(AgentLogEntry) -> std::result::Result<(), SandboxStoreError> + Send,
{
    let rows = cma_am_agent_log::retained::RetainedRows::new(head, range, proof)?;
    let collection = majority_collection::<RawDocumentBuf>(db, AGENT_LOG);
    let mut read = crate::store::mongo_read_telemetry::Read::find(collection.name());
    let result = rows
        .scan(
            &collection,
            session,
            snapshot,
            crate::store::sandbox_mongo::DOCUMENT_SCHEMA_VERSION,
            &mut |bytes| read.document(bytes),
            visit,
        )
        .await;
    if result.is_ok() {
        read.complete();
    }
    result
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
            None,
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

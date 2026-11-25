use super::super::{corrupt, digest, projection, reads, MAX_BYTES, MAX_ENTRIES};
use super::*;
use crate::store::agent_log::read_entry;
use layer2_core::sandbox::AgentLogKind;
use mongodb::bson::RawDocumentBuf;

// Source verification has its own ceiling before the tail query. The download
// is separately limited after projecting the verified page.
const EXPORT_TAIL_ENTRIES: i64 = 1_024;
const EXPORT_TAIL_BYTES: i64 = 8 * 1024 * 1024;

pub(super) enum ReadBudget<'a> {
    Standard,
    Export,
    Preview(&'a mut preview_budget::PreviewBudget),
}

pub(super) async fn read(
    store: &MongoSandboxStore,
    agent: &Agent,
    row: &Document,
    session: &mut ClientSession,
    context: SourceContext,
    snapshot: bool,
    limits: ReadBudget<'_>,
) -> Result<CapturedLog, TransactionLogError> {
    let (export, mut budget) = match limits {
        ReadBudget::Standard => (false, None),
        ReadBudget::Export => (true, None),
        ReadBudget::Preview(budget) => (false, Some(budget)),
    };
    let (head, entry_limit, byte_limit) = if let Some(budget) = budget.as_deref_mut() {
        let head = reads::preview_head(store, row, session, budget).await?;
        budget.require_bytes(head.active_bytes)?;
        let count = head.active_count;
        (head, count, preview_budget::MAX_SOURCE_BYTES as i64)
    } else if export {
        let head =
            reads::read_export_head(store, row, session, EXPORT_TAIL_ENTRIES, EXPORT_TAIL_BYTES)
                .await?;
        let count = head.active_count;
        (head, count, EXPORT_TAIL_BYTES)
    } else {
        (
            super::super::read_head(store, row, session, snapshot).await?,
            MAX_ENTRIES,
            MAX_BYTES,
        )
    };
    let projection =
        projection::read_budgeted(store, agent, &head, session, budget.as_deref_mut()).await?;
    if head.active_ordinal > 0 {
        let seal = if let Some(budget) = budget.as_deref_mut() {
            reads::preview_seal_at(store, &head, head.active_ordinal - 1, session, budget).await?
        } else {
            reads::seal_at(store, &head, head.active_ordinal - 1, session, snapshot).await?
        };
        seal.precedes(&head)?;
    }
    if !export && budget.is_none() && store.active_log_proofs {
        if let Some(leaves) =
            super::super::active_proof::read(store, &head, agent.revision, session, snapshot)
                .await?
        {
            let mut captured = CapturedLog {
                agent: agent.clone(),
                format: CapturedFormat::Segmented,
                incarnation: head.incarnation.clone(),
                latest_entry: None,
                max_opened_generation: projection.max_opened_generation,
                max_retired_generation: projection.max_retired_generation,
                projection,
                active_head: head,
                active_leaf_digests: leaves,
                context,
            };
            if captured.through() > 0 {
                captured.latest_entry =
                    Some(latest(store, &captured, session, snapshot, None).await?);
            }
            return Ok(captured);
        }
    }
    let source = store.agent_log.clone_with_type::<RawDocumentBuf>();
    let mut read = budget
        .is_none()
        .then(|| crate::store::mongo_read_telemetry::Read::find(source.name()));
    let query=source.find(doc! {"agent_id":agent.id.as_str(),"seq":{"$gte":head.active_start_seq,"$lte":head.last_seq}})
        .sort(doc! {"seq":1}).limit(entry_limit+1).batch_size(64);
    let query = if snapshot {
        query.read_concern(ReadConcern::snapshot())
    } else {
        query
    };
    let mut cursor = if budget.is_none() {
        Some(
            query
                .session(&mut *session)
                .await
                .map_err(TransactionLogError::Driver)?,
        )
    } else {
        None
    };
    let mut preview_rows = preview_budget::PreviewRows::new(
        store.agent_log.clone(),
        doc! {"agent_id":agent.id.as_str(),"seq":{"$gte":head.active_start_seq,"$lte":head.last_seq}},
        entry_limit as usize + 1,
    );
    let mut next = head.active_start_seq;
    let mut bytes = 0_i64;
    let mut leaves = Vec::new();
    let mut latest_entry = None;
    while let Some(raw) = if let Some(budget) = budget.as_deref_mut() {
        preview_rows.next(session, budget).await?
    } else {
        read.as_mut()
            .ok_or_else(corrupt)?
            .next_in_session(cursor.as_mut().ok_or_else(corrupt)?, session)
            .await
            .map_err(TransactionLogError::Driver)?
    } {
        let length = i64::try_from(raw.as_bytes().len()).map_err(|_| corrupt())?;
        bytes = bytes
            .checked_add(length)
            .filter(|n| *n <= byte_limit && length <= 16 * 1024 * 1024)
            .ok_or_else(corrupt)?;
        let entry = read_entry(&Document::try_from(&raw).map_err(|_| corrupt())?)?;
        if entry.agent_id != agent.id
            || entry.seq != next as u64
            || next > head.last_seq
            || (next == 1) != matches!(entry.kind, AgentLogKind::Created { .. })
        {
            return Err(corrupt().into());
        }
        if let AgentLogKind::Created {
            workspace_id,
            worktree_id,
        } = &entry.kind
        {
            if workspace_id != &agent.workspace_id || worktree_id != agent.creation_worktree_id() {
                return Err(corrupt().into());
            }
        }
        leaves.extend_from_slice(
            &digest::leaf(agent.id.as_str(), &head.incarnation, next, raw.as_bytes()).bytes,
        );
        latest_entry = Some(entry);
        next += 1;
    }
    if next != head.last_seq + 1 || bytes != head.active_bytes {
        return Err(corrupt().into());
    }
    let mut captured = CapturedLog {
        agent: agent.clone(),
        format: CapturedFormat::Segmented,
        incarnation: head.incarnation.clone(),
        latest_entry,
        max_opened_generation: projection.max_opened_generation,
        max_retired_generation: projection.max_retired_generation,
        projection,
        active_head: head,
        active_leaf_digests: digest::binary(leaves),
        context,
    };
    if captured.latest_entry.is_none() && captured.through() > 0 {
        captured.latest_entry =
            Some(latest(store, &captured, session, snapshot, budget.as_deref_mut()).await?);
    }
    if let Some(budget) = budget {
        budget.check()?;
    }
    if let Some(read) = read.as_mut() {
        read.complete();
    }
    Ok(captured)
}

async fn latest(
    store: &MongoSandboxStore,
    captured: &CapturedLog,
    session: &mut ClientSession,
    snapshot: bool,
    mut budget: Option<&mut preview_budget::PreviewBudget>,
) -> Result<AgentLogEntry, TransactionLogError> {
    let seq = captured.active_head.last_seq;
    let seal = if seq >= captured.active_head.active_start_seq {
        None
    } else {
        Some(if let Some(budget) = budget.as_deref_mut() {
            reads::preview_containing(store, &captured.active_head, seq, session, budget).await?
        } else {
            reads::containing(store, &captured.active_head, seq, session, snapshot).await?
        })
    };
    let source = store.agent_log.clone_with_type::<RawDocumentBuf>();
    let mut read = budget
        .is_none()
        .then(|| crate::store::mongo_read_telemetry::Read::find(source.name()));
    let raw = if let Some(budget) = budget {
        budget
            .raw(
                &store.agent_log,
                doc! {"agent_id":captured.agent.id.as_str(),"seq":seq},
                None,
                session,
            )
            .await?
            .ok_or_else(corrupt)?
    } else {
        let query = source.find_one(doc! {"agent_id":captured.agent.id.as_str(),"seq":seq});
        let query = if snapshot {
            query.read_concern(ReadConcern::snapshot())
        } else {
            query
        };
        query
            .session(session)
            .await
            .map_err(TransactionLogError::Driver)?
            .ok_or_else(corrupt)?
    };
    if let Some(read) = read.as_mut() {
        read.document(raw.as_bytes().len());
    }
    if raw.as_bytes().len() > 16 * 1024 * 1024 {
        return Err(corrupt().into());
    }
    let entry = read_entry(&Document::try_from(&raw).map_err(|_| corrupt())?)?;
    if entry.seq != captured.through() || entry.agent_id != captured.agent.id {
        return Err(corrupt().into());
    }
    let (leaves, start) = seal.as_ref().map_or(
        (
            &captured.active_leaf_digests,
            captured.active_head.active_start_seq,
        ),
        |seal| (&seal.leaf_digests, seal.start_seq),
    );
    digest::check_leaf(
        leaves,
        seq - start,
        &digest::leaf(
            captured.agent.id.as_str(),
            &captured.incarnation,
            seq,
            raw.as_bytes(),
        ),
    )?;
    if let Some(read) = read.as_mut() {
        read.complete();
    }
    Ok(entry)
}

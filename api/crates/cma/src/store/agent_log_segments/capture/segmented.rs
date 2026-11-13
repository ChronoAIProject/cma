use super::super::{corrupt, digest, projection, reads, MAX_BYTES, MAX_ENTRIES};
use super::*;
use crate::store::agent_log::read_entry;
use layer2_core::sandbox::AgentLogKind;
use mongodb::bson::RawDocumentBuf;

pub(super) async fn read(
    store: &MongoSandboxStore,
    agent: &Agent,
    row: &Document,
    session: &mut ClientSession,
    context: SourceContext,
    snapshot: bool,
) -> Result<CapturedLog, TransactionLogError> {
    let head = super::super::read_head(store, row, session, snapshot).await?;
    let projection = projection::read(store, agent, &head, session).await?;
    if head.active_ordinal > 0 {
        reads::seal_at(store, &head, head.active_ordinal - 1, session, snapshot)
            .await?
            .precedes(&head)?;
    }
    let query=store.agent_log.find_one(doc! {"agent_id":agent.id.as_str(),"$or":[{"seq":{"$gt":head.last_seq}},{"seq":{"$lt":1_i64}}]})
        .projection(doc! {"_id":1});
    let query = if snapshot {
        query.read_concern(ReadConcern::snapshot())
    } else {
        query
    };
    if query
        .session(&mut *session)
        .await
        .map_err(TransactionLogError::Driver)?
        .is_some()
    {
        return Err(SandboxStoreError::SequenceConflict.into());
    }
    let source = store.agent_log.clone_with_type::<RawDocumentBuf>();
    let mut read = crate::store::mongo_read_telemetry::Read::find(source.name());
    let query=source.find(doc! {"agent_id":agent.id.as_str(),"seq":{"$gte":head.active_start_seq,"$lte":head.last_seq}})
        .sort(doc! {"seq":1}).limit(MAX_ENTRIES+1).batch_size(64);
    let query = if snapshot {
        query.read_concern(ReadConcern::snapshot())
    } else {
        query
    };
    let mut cursor = query
        .session(&mut *session)
        .await
        .map_err(TransactionLogError::Driver)?;
    let mut next = head.active_start_seq;
    let mut bytes = 0_i64;
    let mut leaves = Vec::new();
    let mut latest_entry = None;
    while let Some(raw) = read
        .next_in_session(&mut cursor, session)
        .await
        .map_err(TransactionLogError::Driver)?
    {
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
        let length = i64::try_from(raw.as_bytes().len()).map_err(|_| corrupt())?;
        if length > 16 * 1024 * 1024 {
            return Err(corrupt().into());
        }
        bytes = bytes
            .checked_add(length)
            .filter(|n| *n <= MAX_BYTES)
            .ok_or_else(corrupt)?;
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
        captured.latest_entry = Some(latest(store, &captured, session, snapshot).await?);
    }
    read.complete();
    Ok(captured)
}

async fn latest(
    store: &MongoSandboxStore,
    captured: &CapturedLog,
    session: &mut ClientSession,
    snapshot: bool,
) -> Result<AgentLogEntry, TransactionLogError> {
    let seq = captured.active_head.last_seq;
    let seal = reads::containing(store, &captured.active_head, seq, session, snapshot).await?;
    let source = store.agent_log.clone_with_type::<RawDocumentBuf>();
    let mut read = crate::store::mongo_read_telemetry::Read::find(source.name());
    let query = source.find_one(doc! {"agent_id":captured.agent.id.as_str(),"seq":seq});
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
    if raw.as_bytes().len() > 16 * 1024 * 1024 {
        return Err(corrupt().into());
    }
    let entry = read_entry(&Document::try_from(&raw).map_err(|_| corrupt())?)?;
    if entry.seq != captured.through() || entry.agent_id != captured.agent.id {
        return Err(corrupt().into());
    }
    digest::check_leaf(
        &seal.leaf_digests,
        seq - seal.start_seq,
        &digest::leaf(
            captured.agent.id.as_str(),
            &captured.incarnation,
            seq,
            raw.as_bytes(),
        ),
    )?;
    read.complete();
    Ok(entry)
}

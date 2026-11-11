//! Source stays in one read-only snapshot; evidence writes use short transactions.
use super::super::{projection, Seal, SEGMENTS};
use super::*;
use crate::store::agent_log::read_entry;
use layer2_core::sandbox::{AgentLogKind, QueuedMessageState};
use mongodb::{bson::RawDocumentBuf, options::ReadConcern};
use persistence::Batch;
use sha2::{Digest, Sha256};

pub(super) async fn run(
    client: &Client,
    database: &Database,
    progress: &mut Progress,
) -> Result<(), SandboxStoreError> {
    let mut snapshot = client
        .start_session()
        .snapshot(true)
        .await
        .map_err(unavailable)?;
    let (agent, current) =
        progress::current(database, &progress.agent_id, &mut snapshot, true).await?;
    let captured = &progress.captured_head;
    if current.incarnation != captured.incarnation
        || current.revision < captured.revision
        || current.last_seq < captured.last_seq
        || current.active_ordinal < captured.active_ordinal
        || (current.revision == captured.revision && current != *captured)
    {
        return Err(corrupt());
    }
    // Completed units are immutable. Capture current metadata for the next
    // unprocessed unit so a newly sealed former tail is audited normally.
    progress.captured_agent = agent;
    progress.captured_head = current.clone();
    progress.through_seq = current.last_seq;
    progress.head_revision = current.revision;
    let generation = format!("audit-stage:{}", uuid::Uuid::new_v4().simple());
    progress.stage = Some(Stage::Collecting {
        generation: generation.clone(),
    });
    persistence::write(client, database, progress, &Batch::default()).await?;
    let staged = progress.head(&generation);
    let output = progress.head(&progress.result_generation);
    let (seal, start, end, expected_bytes, tail) =
        bounds(database, progress, &mut snapshot).await?;
    let source = collection(database, "agent_log").clone_with_type::<RawDocumentBuf>();
    let mut telemetry = crate::store::mongo_read_telemetry::Read::find(source.name());
    let mut rows = source
        .find(doc! {"agent_id":&progress.agent_id,"seq":{"$gte":start,"$lte":end}})
        .sort(doc! {"seq":1})
        .limit(MAX_ENTRIES + 1)
        // Source replies and evidence writes have independent bounds. Reading
        // more source rows per reply keeps command work bounded when one row
        // produces both semantic and output-index evidence. Mongo still caps
        // each reply; persistence retains its smaller row/byte transaction cap.
        .batch_size(512)
        .read_concern(ReadConcern::snapshot())
        .session(&mut snapshot)
        .await
        .map_err(unavailable)?;
    let mut next = start;
    let mut bytes = 0_i64;
    let mut semantic = progress.completed.semantic.clone();
    let mut batch = Batch::default();
    let mut output_index = output::resume(database, progress, &staged, &output).await?;
    while let Some(raw) = telemetry
        .next_in_session(&mut rows, &mut snapshot)
        .await
        .map_err(unavailable)?
    {
        let entry = read_entry(&Document::try_from(&raw).map_err(|_| corrupt())?)?;
        if entry.agent_id.as_str() != progress.agent_id
            || entry.seq != next as u64
            || next > end
            || (next == 1) != matches!(entry.kind, AgentLogKind::Created { .. })
        {
            return Err(corrupt());
        }
        let length = i64::try_from(raw.as_bytes().len()).map_err(|_| corrupt())?;
        if length > 16 * 1024 * 1024 {
            return Err(corrupt());
        }
        bytes = bytes
            .checked_add(length)
            .filter(|n| *n <= MAX_BYTES)
            .ok_or_else(corrupt)?;
        if let Some(seal) = &seal {
            digest::check_leaf(
                &seal.leaf_digests,
                next - start,
                &digest::leaf(
                    &progress.agent_id,
                    &progress.incarnation,
                    next,
                    raw.as_bytes(),
                ),
            )?;
        }
        let queued = AgentLogProjectionState::queued_owner(&entry);
        if let Some(sequence) = queued {
            let fact = lookup(
                database,
                &staged,
                &output,
                &batch,
                "queued",
                &sequence.to_string(),
            )
            .await?
            .ok_or_else(corrupt)?;
            if !matches!(fact,projection::EvidenceRecord::Queued {sequence:original,consumed_by:None} if original==sequence && sequence<entry.seq)
            {
                return Err(corrupt());
            }
        }
        for change in semantic
            .observe(&progress.captured_agent, &entry, queued)
            .map_err(SandboxStoreError::Corrupt)?
        {
            let row = projection::plan(&staged, change)?;
            if !batch.push(&row)? {
                persistence::write(client, database, progress, &batch).await?;
                batch = Batch::default();
                if !batch.push(&row)? {
                    return Err(corrupt());
                }
            }
        }
        for row in output::observe(
            database,
            &staged,
            &output,
            &batch,
            &mut output_index,
            &entry,
        )
        .await?
        {
            if !batch.push(&row)? {
                persistence::write(client, database, progress, &batch).await?;
                batch = Batch::default();
                if !batch.push(&row)? {
                    return Err(corrupt());
                }
            }
        }
        // Even a unit with no derived changes renews its fence in bounded steps.
        if (next - start + 1) % persistence::MAX_ROWS as i64 == 0
            || now() >= progress.expires_at_ms - LEASE_MS / 2
        {
            persistence::write(client, database, progress, &batch).await?;
            batch = Batch::default();
        }
        next += 1;
    }
    if next != end + 1 || bytes != expected_bytes {
        return Err(corrupt());
    }
    if tail {
        semantic
            .validate_aggregate(&progress.captured_agent)
            .map_err(SandboxStoreError::Corrupt)?;
        validate_queue(database, progress, &staged, &output, &batch, &mut snapshot).await?;
        let source = collection(database, "agent_log");
        if source.find_one(doc! {"agent_id":&progress.agent_id,"$or":[{"seq":{"$gt":current.last_seq}},{"seq":{"$lt":1_i64}}]})
            .projection(doc! {"_id":1}).read_concern(ReadConcern::snapshot()).session(&mut snapshot)
            .await.map_err(unavailable)?.is_some() {return Err(corrupt());}
    }
    // No completed-prefix progress advances until every source row and all
    // aggregate/queue proofs passed. Only this durable phase permits promotion.
    let checked_bytes = progress
        .completed
        .checked_bytes
        .checked_add(bytes)
        .ok_or_else(corrupt)?;
    if tail
        && (end != progress.through_seq || checked_bytes != progress.captured_head.lifetime_bytes)
    {
        return Err(corrupt());
    }
    progress.stage = Some(Stage::Promoting {
        generation,
        completed: Box::new(Checkpoint {
            next_ordinal: progress.completed.next_ordinal + 1,
            checked_entries: end,
            checked_bytes,
            previous_seal: seal.map(|s| s.seal_digest),
            semantic,
        }),
        tail,
        cursor: None,
    });
    persistence::write(client, database, progress, &batch).await?;
    telemetry.complete();
    Ok(())
}

async fn bounds(
    database: &Database,
    progress: &Progress,
    session: &mut ClientSession,
) -> Result<(Option<Seal>, i64, i64, i64, bool), SandboxStoreError> {
    let head = &progress.captured_head;
    let ordinal = progress.completed.next_ordinal;
    let seals = collection(database, SEGMENTS);
    if seals.find_one(doc! {"agent_id":&head.agent_id,"incarnation":&head.incarnation,"ordinal":{"$gte":head.active_ordinal}})
        .projection(doc! {"_id":1}).read_concern(ReadConcern::snapshot()).session(&mut *session)
        .await.map_err(unavailable)?.is_some() {return Err(corrupt());}
    let tail = ordinal == head.active_ordinal;
    let seal = if tail {
        None
    } else {
        Some(Seal::read(seals.find_one(doc! {"agent_id":&head.agent_id,"incarnation":&head.incarnation,"ordinal":ordinal})
            .read_concern(ReadConcern::snapshot()).session(&mut *session).await.map_err(unavailable)?.ok_or_else(corrupt)?)?)
    };
    let (start, end, bytes) = if let Some(seal) = &seal {
        if seal.agent_id != head.agent_id
            || seal.incarnation != head.incarnation
            || seal.ordinal != ordinal
            || seal.previous_seal != progress.completed.previous_seal
            || progress
                .completed
                .checked_bytes
                .checked_add(seal.encoded_bytes)
                != Some(seal.prefix_bytes)
        {
            return Err(corrupt());
        }
        (seal.start_seq, seal.end_seq, seal.encoded_bytes)
    } else {
        if head.previous_seal != progress.completed.previous_seal {
            return Err(corrupt());
        }
        (head.active_start_seq, head.last_seq, head.active_bytes)
    };
    if ordinal > head.active_ordinal
        || start != progress.completed.checked_entries + 1
        || end < start - 1
        || end - start + 1 > MAX_ENTRIES
    {
        return Err(corrupt());
    }
    Ok((seal, start, end, bytes, tail))
}

pub(super) async fn lookup(
    database: &Database,
    staged: &Head,
    output: &Head,
    batch: &Batch,
    kind: &str,
    key: &str,
) -> Result<Option<projection::EvidenceRecord>, SandboxStoreError> {
    let id = projection::identity(staged, kind, key);
    if let Some(row) = batch.get(&id) {
        return projection::read_evidence(staged, kind, key, row.clone()).map(Some);
    }
    // Staging writes occur after the source snapshot was opened. These are
    // majority reads of this fenced generation, never reads of user source.
    for head in [staged, output] {
        if let Some(row) = collection(database, EVIDENCE)
            .find_one(doc! {"_id":projection::identity(head,kind,key)})
            .await
            .map_err(unavailable)?
        {
            return projection::read_evidence(head, kind, key, row).map(Some);
        }
    }
    Ok(None)
}
async fn validate_queue(
    database: &Database,
    progress: &Progress,
    staged: &Head,
    output: &Head,
    batch: &Batch,
    session: &mut ClientSession,
) -> Result<(), SandboxStoreError> {
    for queued in &progress.captured_agent.conversation.queued_messages {
        let fact = lookup(
            database,
            staged,
            output,
            batch,
            "queued",
            &queued.sequence.to_string(),
        )
        .await?
        .ok_or_else(corrupt)?;
        if !matches!(fact,projection::EvidenceRecord::Queued {sequence,consumed_by}
            if sequence==queued.sequence && (queued.state!=QueuedMessageState::Queued || consumed_by.is_none()))
        {
            return Err(corrupt());
        }
        let row = collection(database, "agent_log")
            .find_one(doc! {"agent_id":&progress.agent_id,"seq":queued.sequence as i64})
            .read_concern(ReadConcern::snapshot())
            .session(&mut *session)
            .await
            .map_err(unavailable)?
            .ok_or_else(corrupt)?;
        let entry = read_entry(&row)?;
        if entry.agent_id.as_str() != progress.agent_id || entry.seq != queued.sequence {
            return Err(corrupt());
        }
        let AgentLogKind::OwnerMessage { content } = entry.kind else {
            return Err(corrupt());
        };
        if format!("{:x}", Sha256::digest(content.as_bytes())) != queued.payload_sha256 {
            return Err(corrupt());
        }
    }
    Ok(())
}

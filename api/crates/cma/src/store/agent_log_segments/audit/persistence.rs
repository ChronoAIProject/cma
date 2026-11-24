//! Each evidence batch and its durable progress cursor commit together.
use super::super::projection;
use super::*;
use crate::store::transaction_terminal::GuardedTerminal as _;
use futures_util::TryStreamExt;
use std::collections::BTreeMap;

pub(super) const MAX_ROWS: usize = 128;
pub(super) const MAX_BATCH_BYTES: usize = 1024 * 1024;
const MAX_ROW_BYTES: usize = 256 * 1024;

#[derive(Default)]
pub(super) struct Batch {
    rows: BTreeMap<String, Document>,
    bytes: usize,
}
impl Batch {
    pub(super) fn push(&mut self, row: &Document) -> Result<bool, SandboxStoreError> {
        let id = row.get_str("_id").map_err(|_| corrupt())?;
        let size = bson::serialize_to_vec(row).map_err(|_| corrupt())?.len();
        if size > MAX_ROW_BYTES {
            return Err(corrupt());
        }
        let old = self
            .rows
            .get(id)
            .map(bson::serialize_to_vec)
            .transpose()
            .map_err(|_| corrupt())?
            .map_or(0, |v| v.len());
        let bytes = self
            .bytes
            .checked_sub(old)
            .and_then(|n| n.checked_add(size))
            .ok_or_else(corrupt)?;
        if bytes > MAX_BATCH_BYTES || (!self.rows.contains_key(id) && self.rows.len() == MAX_ROWS) {
            return Ok(false);
        }
        self.rows.insert(id.into(), row.clone());
        self.bytes = bytes;
        Ok(true)
    }
    pub(super) fn get(&self, key: &str) -> Option<&Document> {
        self.rows.get(key)
    }
}

pub(super) async fn write(
    client: &Client,
    database: &Database,
    progress: &mut Progress,
    batch: &Batch,
) -> Result<(), SandboxStoreError> {
    let at = now();
    let mut next = progress.clone();
    next.step = next.step.checked_add(1).ok_or_else(corrupt)?;
    next.expires_at_ms = if next.status == "running" {
        at.checked_add(LEASE_MS).ok_or_else(corrupt)?
    } else {
        0
    };
    let row = next.document()?;
    let mut session = client.start_session().await.map_err(unavailable)?;
    start_transaction(&mut session).await.map_err(unavailable)?;
    let result = async {
        // Write the fence first. A replacement holder, cancellation, or competing
        // batch conflicts before any evidence can commit under stale authority.
        if collection(database, AUDITS)
            .replace_one(progress.filter(at), row)
            .session(&mut session)
            .await
            .map_err(unavailable)?
            .matched_count
            != 1
        {
            return Err(SandboxStoreError::Conflict);
        }
        if !batch.rows.is_empty() {
            // One bounded update command preserves the same transaction/fence
            // while avoiding a network round trip for each evidence row. Use
            // the collection update protocol, which does not raise the server
            // version floor to the newer cross-namespace bulkWrite command.
            let updates: Vec<_> = batch
                .rows
                .iter()
                .map(|(id, row)| {
                    doc! {"q":{"_id":id},"u":row.clone(),"upsert":true,"multi":false}
                })
                .collect();
            let reply = database
                .run_command(doc! {"update":EVIDENCE,
                "updates":updates,"ordered":true})
                .session(&mut session)
                .await
                .map_err(unavailable)?;
            require_complete_batch(&reply, batch.rows.len())?;
        }
        Ok(())
    }
    .await;
    if let Err(error) = result {
        let _ = session.abort_guarded().await;
        return Err(error);
    }
    commit(&mut session).await?;
    *progress = next;
    Ok(())
}

pub(super) fn require_complete_batch(
    reply: &Document,
    expected: usize,
) -> Result<(), SandboxStoreError> {
    let complete = match reply.get("n") {
        Some(bson::Bson::Int32(n)) => i64::from(*n),
        Some(bson::Bson::Int64(n)) => *n,
        _ => -1,
    } == expected as i64;
    let no_errors = match reply.get("writeErrors") {
        None => true,
        Some(bson::Bson::Array(errors)) => errors.is_empty(),
        _ => false,
    };
    if !complete || !no_errors || reply.contains_key("writeConcernError") {
        return Err(SandboxStoreError::Unavailable(
            "Audit evidence batch was not fully acknowledged; resume the same audit ID".into(),
        ));
    }
    Ok(())
}

pub(super) async fn promote(
    client: &Client,
    database: &Database,
    progress: &mut Progress,
) -> Result<(), SandboxStoreError> {
    loop {
        if promote_batch(client, database, progress).await? {
            return Ok(());
        }
    }
}

/// True means the validated unit has been fully promoted. A crash after any
/// successful batch resumes after the cursor without repeating source folding.
pub(super) async fn promote_batch(
    client: &Client,
    database: &Database,
    progress: &mut Progress,
) -> Result<bool, SandboxStoreError> {
    let Some(Stage::Promoting {
        generation,
        completed,
        tail,
        cursor,
    }) = progress.stage.clone()
    else {
        return Err(corrupt());
    };
    let staged = progress.head(&generation);
    let target = progress.head(&progress.result_generation);
    let mut filter = doc! {"agent_id":&progress.agent_id,"incarnation":&progress.incarnation,"evidence_generation":&generation};
    if let Some(cursor) = cursor {
        filter.insert("_id", doc! {"$gt":cursor});
    }
    let evidence = collection(database, EVIDENCE);
    let mut cursor = evidence
        .find(filter)
        .sort(doc! {"_id":1})
        .limit(MAX_ROWS as i64 + 1)
        // Include the lookahead in the same bounded reply. Otherwise every
        // full promotion page needs another getMore for its single extra row.
        .batch_size(MAX_ROWS as u32 + 1)
        .await
        .map_err(unavailable)?;
    let mut batch = Batch::default();
    let mut through = None;
    let mut exhausted = true;
    while let Some(row) = cursor.try_next().await.map_err(unavailable)? {
        let source: projection::Evidence =
            bson::deserialize_from_document(row.clone()).map_err(|_| corrupt())?;
        projection::read_evidence(&staged, &source.kind, &source.entity_key, row)?;
        let source_id = source.id.clone();
        let mut output = source;
        output.id = projection::identity(&target, &output.kind, &output.entity_key);
        output.evidence_generation = target.evidence_generation.clone();
        let row = bson::serialize_to_document(&output).map_err(|_| corrupt())?;
        if !batch.push(&row)? {
            exhausted = false;
            break;
        }
        through = Some(source_id);
    }
    if exhausted {
        progress.completed = *completed;
        progress.stage = None;
        if tail {
            progress.status = "complete".into();
        }
    } else if let Some(Stage::Promoting { cursor, .. }) = progress.stage.as_mut() {
        *cursor = through;
    }
    write(client, database, progress, &batch).await?;
    if exhausted && progress.status == "running" {
        // Release this unit's lease. The next invocation captures the next unit.
        release(client, database, progress).await?;
    }
    Ok(exhausted)
}

async fn release(
    client: &Client,
    database: &Database,
    progress: &mut Progress,
) -> Result<(), SandboxStoreError> {
    let at = now();
    let mut next = progress.clone();
    next.expires_at_ms = 0;
    next.step = next.step.checked_add(1).ok_or_else(corrupt)?;
    let mut session = client.start_session().await.map_err(unavailable)?;
    start_transaction(&mut session).await.map_err(unavailable)?;
    let result = collection(database, AUDITS)
        .replace_one(progress.filter(at), next.document()?)
        .session(&mut session)
        .await;
    match result {
        Ok(result) if result.matched_count == 1 => {
            commit(&mut session).await?;
            *progress = next;
            Ok(())
        }
        Ok(_) => {
            let _ = session.abort_guarded().await;
            Err(SandboxStoreError::Conflict)
        }
        Err(error) => {
            let _ = session.abort_guarded().await;
            Err(unavailable(error))
        }
    }
}

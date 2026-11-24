//! Invisible evidence generations, built in bounded lease-fenced transactions.
use super::*;
use crate::store::transaction_terminal::GuardedTerminal as _;
use crate::store::{
    agent_log_integrity::TransactionLogError,
    agent_log_segments::{
        capture::{self, CaptureMode},
        collections::collection,
        projection, writer_admission, EVIDENCE,
    },
    sandbox_mongo::{commit_transaction, start_transaction, CommitOutcome, MongoSandboxStore},
};
use layer2_core::sandbox::agent_output_index::{AgentOutputTool, AgentTurnOutputIndex};
use layer2_core::sandbox::BoundedAgentLogFold;
use mongodb::{
    bson::{self, doc, DateTime, Document},
    ClientSession,
};
use projection::{output, EvidenceRecord};
use std::collections::BTreeMap;

const BATCH_ROWS: usize = 64;
const BATCH_BYTES: usize = 1024 * 1024;
const RECORD_BYTES: usize = 256 * 1024;

/// Called only after exact admission and preparation-slot claim are committed.
/// This method owns its staging transactions, never the publication transaction.
/// A failed/unknown batch leaves an unreferenced generation and returns no plan.
/// Restart claims a new permit and builds a fresh generation; source rows are
/// never rewritten. Final prepare independently recaptures the exact source.
pub(in crate::store) async fn stage(
    store: &MongoSandboxStore,
    current: &Agent,
    operation: &PublicationIdentity,
    admission: &Admission,
    mut permit: Permit,
) -> Result<BootstrapPlan, TransactionLogError> {
    admission.permits(current, operation, admission.kind)?;
    permit.validate_admission(admission)?;
    let mut snapshot = store
        .client
        .start_session()
        .snapshot(true)
        .await
        .map_err(TransactionLogError::Driver)?;
    let captured = capture::capture(store, current, &mut snapshot, CaptureMode::Snapshot).await?;
    if captured.format() != CapturedFormat::Legacy {
        return Err(SandboxStoreError::Conflict.into());
    }
    let mut head = captured.active_head().clone();
    head.revision = 1;
    head.evidence_generation = format!(
        "writer-{}-{}",
        permit.fence(),
        uuid::Uuid::new_v4().simple()
    );
    head.validate()?;
    let mut fold = BoundedAgentLogFold::default();
    let mut batch = Batch::default();
    let mut output_index: Option<AgentTurnOutputIndex> = None;
    let mut writer = store
        .client
        .start_session()
        .await
        .map_err(TransactionLogError::Driver)?;
    // Read one source point at a time: the S1 point budget applies even to a
    // batch of individually valid large events. No adaptive retry may hide a
    // corrupt or missing row. Oversize source evidence fails explicitly.
    for sequence in 1..=captured.through() {
        let entries = capture::points(
            store,
            &captured,
            &[sequence],
            &mut snapshot,
            CaptureMode::Snapshot,
        )
        .await?;
        let entry = entries.first().ok_or_else(corrupt)?;
        let mut rows = fold
            .observe(current, entry)
            .map_err(SandboxStoreError::Corrupt)?
            .into_iter()
            .map(|change| projection::plan(&head, change))
            .collect::<Result<Vec<_>, _>>()?;
        let tool = if let (Some(index), Some(call)) = (&output_index, output::call_id(entry)) {
            let key = output::tool_key(index.turn_sequence, call);
            let id = projection::identity(&head, "tool_output", &key);
            let row = if let Some((row, _)) = batch.rows.get(&id) {
                Some(row.clone())
            } else {
                collection(store, EVIDENCE)
                    .find_one(doc! {"_id":id})
                    .session(&mut writer)
                    .await
                    .map_err(TransactionLogError::Driver)?
            };
            match row
                .map(|row| projection::read_evidence(&head, "tool_output", &key, row))
                .transpose()?
            {
                Some(EvidenceRecord::ToolOutput { tool }) => Some(tool),
                None => None,
                _ => return Err(corrupt().into()),
            }
        } else {
            None
        };
        let supplied = match (output::call_id(entry), tool.as_ref()) {
            (Some(_), Some(tool)) => AgentOutputTool::Stored(tool),
            (Some(_), None) => AgentOutputTool::Absent,
            (None, _) => AgentOutputTool::None,
        };
        rows.extend(output::fold(&head, &mut output_index, entry, supplied)?);
        for row in rows {
            let size = bson::serialize_to_vec(&row).map_err(|_| corrupt())?.len();
            if size > RECORD_BYTES {
                return Err(budget().into());
            }
            if !batch.fits(&row, size)? {
                permit = persist_batch(
                    store,
                    current,
                    operation,
                    admission,
                    &permit,
                    &mut writer,
                    batch,
                )
                .await?;
                batch = Batch::default();
            }
            batch.insert(row, size)?;
        }
        // Diagnostic-only source can produce no evidence rows. Bound the
        // lease renewal interval by source work as well as buffered writes.
        if sequence % BATCH_ROWS as u64 == 0 {
            permit = persist_batch(
                store,
                current,
                operation,
                admission,
                &permit,
                &mut writer,
                batch,
            )
            .await?;
            batch = Batch::default();
        }
    }
    let state = fold.finish(current).map_err(SandboxStoreError::Corrupt)?;
    if state != *captured.projection() {
        return Err(corrupt().into());
    }
    // Also renew/recheck an empty final batch. A plan must never be issued for
    // an uncommitted, expired or replaced admission/lease.
    permit = persist_batch(
        store,
        current,
        operation,
        admission,
        &permit,
        &mut writer,
        batch,
    )
    .await?;
    Ok(BootstrapPlan {
        current: current.clone(),
        head,
        leaves: captured.active_leaf_digests().clone(),
        projection: state,
        operation: operation.clone(),
        admission: admission.clone(),
        permit,
    })
}

#[derive(Default)]
struct Batch {
    rows: BTreeMap<String, (Document, usize)>,
    bytes: usize,
}

impl Batch {
    fn fits(&self, row: &Document, size: usize) -> Result<bool, SandboxStoreError> {
        let id = row.get_str("_id").map_err(|_| corrupt())?;
        let old_size = self.rows.get(id).map_or(0, |(_, size)| *size);
        Ok(size <= RECORD_BYTES
            && (self.rows.contains_key(id) || self.rows.len() < BATCH_ROWS)
            && self.bytes - old_size + size <= BATCH_BYTES)
    }

    fn insert(&mut self, row: Document, size: usize) -> Result<(), SandboxStoreError> {
        if !self.fits(&row, size)? {
            return Err(budget());
        }
        let id = row.get_str("_id").map_err(|_| corrupt())?.to_owned();
        if let Some((_, old_size)) = self.rows.insert(id, (row, size)) {
            self.bytes -= old_size;
        }
        self.bytes += size;
        Ok(())
    }
}

async fn persist_batch(
    store: &MongoSandboxStore,
    current: &Agent,
    operation: &PublicationIdentity,
    admission: &Admission,
    permit: &Permit,
    session: &mut ClientSession,
    batch: Batch,
) -> Result<Permit, TransactionLogError> {
    start_transaction(session)
        .await
        .map_err(TransactionLogError::Driver)?;
    let result = write_batch(store, current, operation, admission, permit, session, batch).await;
    let renewed = match result {
        Ok(renewed) => renewed,
        Err(error) => {
            let _ = session.abort_guarded().await;
            return Err(error);
        }
    };
    match commit_transaction(session).await {
        CommitOutcome::Committed => Ok(renewed),
        CommitOutcome::Unknown(_) => Err(SandboxStoreError::Unavailable(
            "Agent log staging commit is unknown; reconcile the lease before restarting".into(),
        )
        .into()),
        CommitOutcome::Retry(error) | CommitOutcome::Failed(error) => {
            let _ = session.abort_guarded().await;
            Err(TransactionLogError::Driver(error))
        }
    }
}

async fn write_batch(
    store: &MongoSandboxStore,
    current: &Agent,
    operation: &PublicationIdentity,
    admission: &Admission,
    permit: &Permit,
    session: &mut ClientSession,
    batch: Batch,
) -> Result<Permit, TransactionLogError> {
    let database = store.client.database(&store.agents.namespace().db);
    let stored = writer_admission::read(&database, current, operation, session)
        .await?
        .ok_or(SandboxStoreError::Conflict)?;
    if bson::serialize_to_document(&stored).map_err(|_| corrupt())?
        != bson::serialize_to_document(admission).map_err(|_| corrupt())?
    {
        return Err(SandboxStoreError::Conflict.into());
    }
    let renewed = permit.renew(&database, DateTime::now(), session).await?;
    let evidence = collection(store, EVIDENCE);
    for (id, (row, _)) in batch.rows {
        if row.get_str("kind").map_err(|_| corrupt())? == "output_change" {
            evidence
                .insert_one(row)
                .session(&mut *session)
                .await
                .map_err(TransactionLogError::Driver)?;
            continue;
        }
        evidence
            .replace_one(doc! {"_id":id}, row)
            .upsert(true)
            .session(&mut *session)
            .await
            .map_err(TransactionLogError::Driver)?;
    }
    Ok(renewed)
}

fn budget() -> SandboxStoreError {
    SandboxStoreError::Unavailable(
        "Agent log staged evidence exceeds its bounded write budget".into(),
    )
}

#[cfg(test)]
#[path = "writer_bootstrap_stage_tests.rs"]
mod tests;

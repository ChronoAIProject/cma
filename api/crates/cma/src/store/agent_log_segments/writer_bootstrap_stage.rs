//! Invisible evidence generations, built in bounded lease-fenced transactions.
use super::*;
use crate::store::{
    agent_log_integrity::TransactionLogError,
    agent_log_segments::{
        capture::{self, CaptureMode},
        collections::collection,
        projection, writer_admission, EVIDENCE,
    },
    sandbox_mongo::{commit_transaction, start_transaction, CommitOutcome, MongoSandboxStore},
};
use layer2_core::sandbox::BoundedAgentLogFold;
use mongodb::{
    bson::{self, doc, DateTime, Document},
    ClientSession,
};
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
        for change in fold
            .observe(current, entry)
            .map_err(SandboxStoreError::Corrupt)?
        {
            let row = projection::plan(&head, change)?;
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
            let _ = session.abort_transaction().await;
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
            let _ = session.abort_transaction().await;
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

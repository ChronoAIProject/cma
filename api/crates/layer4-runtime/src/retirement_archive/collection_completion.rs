//! Archive completion is atomic with the immutable acknowledgement and removal
//! of its one pending slot. A source commit is never inferred from absent rows.
use layer3_adapter::runtime_ledger_maintenance::LedgerBindingV1;
use rusqlite::{params, Connection, OptionalExtension};

use super::collection_receipt::CollectionCompletion;
use super::{collection, invalid, storage_error, Archive, CollectionReference, GroupProof};
use crate::mutation_ledger::retirement_inventory::DeactivationReceipt;
use crate::{failure::Result, ledger_reclamation::CollectionEvidence};

impl Archive {
    pub(crate) async fn collection_completion(
        &self,
        accepted: LedgerBindingV1,
        reference: CollectionReference,
    ) -> Result<Option<CollectionCompletion>> {
        self.verify_collection_binding(&accepted)?;
        let config = self.config.clone();
        let result = self
            .storage
            .call(move |db| {
                let tx = db.transaction().map_err(|_| invalid())?;
                let result = read(&tx, &config, &accepted, &reference)?;
                tx.commit().map_err(|_| invalid())?;
                Ok(result)
            })
            .await?;
        self.verify()?;
        Ok(result)
    }

    pub(super) async fn acknowledge_collection(
        &self,
        reference: CollectionReference,
        evidence: CollectionEvidence,
        proof: GroupProof,
        source: DeactivationReceipt,
    ) -> Result<CollectionCompletion> {
        self.verify_collection(&evidence.binding().archive_id)?;
        let completion = CollectionCompletion::new(&reference, &evidence, &proof, source)?;
        let binding = evidence.binding().clone();
        let config = self.config.clone();
        let expected = completion.clone();
        let directory = self.path.clone();
        let anchor = self.anchor.clone();
        self.storage.call(move |db| {
            let tx = db.transaction().map_err(|_| storage_error())?;
            if let Some(prior) = read(&tx, &config, &binding, &reference)? {
                if prior != expected { return Err(invalid()); }
            } else {
                collection::require_slot(&tx, &reference.operation_id)?;
                let receipt = super::existing(&tx, &proof)?.ok_or_else(invalid)?;
                if receipt.complete { return Err(invalid()); }
                super::indexed::verification::group(&tx, &config, &proof, &receipt, None)?;
                super::require_reserve(&directory, anchor.as_deref())?;
                let bytes = serde_json::to_vec(&expected).map_err(|_| invalid())?;
                tx.execute("INSERT INTO collection_acknowledgements VALUES (?,?,?)",
                    params![reference.operation_id,expected.receipt_sha256,bytes])
                    .map_err(|_| storage_error())?;
                if tx.execute("UPDATE retired_groups SET complete=1 WHERE group_key=? AND complete=0",
                    [&proof.key]).map_err(|_| storage_error())? != 1
                    || tx.execute("UPDATE collection_evidence SET pending=0 WHERE operation_id=? AND pending=1",
                    [&reference.operation_id]).map_err(|_| storage_error())? != 1
                    || tx.execute("DELETE FROM collection_pending WHERE singleton=1 AND operation_id=?",
                    [&reference.operation_id]).map_err(|_| storage_error())? != 1
                { return Err(invalid()); }
            }
            if read(&tx, &config, &binding, &reference)? != Some(expected.clone()) {
                return Err(invalid());
            }
            tx.commit().map_err(|_| storage_error())?;
            if read(db, &config, &binding, &reference)? != Some(expected) {
                return Err(invalid());
            }
            Ok(())
        }).await?;
        self.verify()?;
        Ok(completion)
    }
}

pub(super) fn read(
    db: &Connection,
    config: &crate::config::RuntimeConfig,
    accepted: &LedgerBindingV1,
    reference: &CollectionReference,
) -> Result<Option<CollectionCompletion>> {
    let evidence = collection::read(db, accepted, reference)?;
    let length: Option<i64> = db
        .query_row(
            "SELECT length(receipt) FROM collection_acknowledgements WHERE operation_id=?",
            [&reference.operation_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|_| invalid())?;
    let Some(length) = length else {
        collection::require_slot(db, &reference.operation_id)?;
        return Ok(None);
    };
    if !(1..=16_384).contains(&length) {
        return Err(invalid());
    }
    let (hash, bytes): (String, Vec<u8>) = db
        .query_row(
            "SELECT receipt_sha256,receipt FROM collection_acknowledgements WHERE operation_id=?",
            [&reference.operation_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|_| invalid())?;
    let completion: CollectionCompletion = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    let proof = db.query_row(
        "SELECT source_sha256,observation_sha256,records FROM retired_groups WHERE group_key=? AND complete=1",
        [&reference.group_key], |r| Ok(GroupProof { key:reference.group_key.clone(),
            source_sha256:r.get(0)?,observation_sha256:r.get(1)?,records:r.get(2)? }))
        .map_err(|_| invalid())?;
    completion.validate(reference, &evidence, &proof)?;
    if completion.receipt_sha256 != hash {
        return Err(invalid());
    }
    let settled: bool = db
        .query_row(
            "SELECT pending=0 FROM collection_evidence WHERE operation_id=?",
            [&reference.operation_id],
            |r| r.get(0),
        )
        .map_err(|_| invalid())?;
    if !settled {
        return Err(invalid());
    }
    let receipt = super::existing(db, &proof)?.ok_or_else(invalid)?;
    super::indexed::verification::group(db, config, &proof, &receipt, None)
        .map_err(|_| invalid())?;
    let still_pending: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM collection_pending WHERE operation_id=?)",
            [&reference.operation_id],
            |r| r.get(0),
        )
        .map_err(|_| invalid())?;
    if still_pending {
        return Err(invalid());
    }
    Ok(Some(completion))
}

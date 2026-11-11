//! Durable original evidence and one bounded pending operation. These records
//! preserve intent across restart; they never establish writer exclusion.
use layer3_adapter::runtime_ledger_maintenance::LedgerBindingV1;
use rusqlite::{params, Connection, OptionalExtension};

use super::{invalid, storage_error, Archive, Version};
use crate::{failure::Result, ledger_reclamation::CollectionEvidence};

use super::{collection_reference as reference, CollectionReference};

impl Archive {
    /// Put and read back the exact accepted bytes before B may record Selected.
    /// One pending operation blocks serving, including before source archival.
    pub(crate) async fn retain_collection(
        &self,
        evidence: CollectionEvidence,
        plan_sha256: String,
    ) -> Result<CollectionReference> {
        self.verify_collection_binding(evidence.binding())?;
        let reference = reference::from_evidence(&evidence, plan_sha256)?;
        reference::validate(&reference)?;
        let binding = evidence.binding().clone();
        let originals = evidence.into_originals();
        let expected = reference.clone();
        let config = self.config.clone();
        let directory = self.path.clone();
        let anchor = self.anchor.clone();
        self.storage
            .call(move |db| {
                let tx = db.transaction().map_err(|_| storage_error())?;
                let exists: bool = tx
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM collection_evidence WHERE operation_id=?)",
                        [&expected.operation_id],
                        |r| r.get(0),
                    )
                    .map_err(|_| invalid())?;
                if exists {
                    let retained = read(&tx, &binding, &expected)?;
                    if retained.originals() != originals.each_ref().map(Vec::as_slice) {
                        return Err(invalid());
                    }
                    super::collection_completion::read(&tx, &config, &binding, &expected)?;
                } else {
                    if binding.generation != config.generation || pending(&tx)? {
                        return Err(invalid());
                    }
                    super::require_reserve(&directory, anchor.as_deref())?;
                    let sizes = originals.each_ref().map(Vec::len);
                    tx.execute(
                        "INSERT INTO collection_evidence VALUES (?,?,?,?,?,?,?,?,1)",
                        params![
                            expected.operation_id,
                            expected.group_key,
                            expected.binding_sha256,
                            expected.plan_sha256,
                            expected.artifacts_sha256,
                            sizes[0],
                            sizes[1],
                            sizes[2]
                        ],
                    )
                    .map_err(|_| storage_error())?;
                    super::collection_artifacts::put(&tx, &expected.operation_id, &originals)?;
                    tx.execute(
                        "INSERT INTO collection_pending VALUES (1,?)",
                        [&expected.operation_id],
                    )
                    .map_err(|_| storage_error())?;
                    read(&tx, &binding, &expected)?;
                }
                tx.commit().map_err(|_| storage_error())?;
                read(db, &binding, &expected)?;
                Ok(())
            })
            .await?;
        self.verify()?;
        Ok(reference)
    }

    /// Find a put whose reply or B Selected publication was lost. Empty B
    /// progress must never authorize replacing the archive's staged intent.
    pub(crate) async fn pending_collection(
        &self,
        accepted: LedgerBindingV1,
    ) -> Result<Option<CollectionReference>> {
        self.verify_collection_binding(&accepted)?;
        let result = self
            .storage
            .call(move |db| {
                let tx = db.transaction().map_err(|_| invalid())?;
                if !pending(&tx)? {
                    return Ok(None);
                }
                let operation: String = tx
                    .query_row(
                        "SELECT operation_id FROM collection_pending WHERE singleton=1",
                        [],
                        |r| r.get(0),
                    )
                    .map_err(|_| invalid())?;
                let reference = reference::load(&tx, &operation)?;
                read(&tx, &accepted, &reference)?;
                tx.commit().map_err(|_| invalid())?;
                Ok(Some(reference))
            })
            .await?;
        self.verify()?;
        Ok(result)
    }

    pub(crate) async fn read_collection(
        &self,
        accepted: LedgerBindingV1,
        reference: CollectionReference,
    ) -> Result<CollectionEvidence> {
        self.verify_collection_binding(&accepted)?;
        reference::validate(&reference)?;
        let evidence = self
            .storage
            .call(move |db| read(db, &accepted, &reference))
            .await?;
        self.verify()?;
        Ok(evidence)
    }

    pub(super) fn verify_collection(&self, archive_id: &str) -> Result<()> {
        self.verify()?;
        if self.version != Version::Collection || archive_id != self.archive_id {
            return Err(invalid());
        }
        Ok(())
    }

    pub(super) fn verify_collection_binding(&self, binding: &LedgerBindingV1) -> Result<()> {
        self.verify_collection(&binding.archive_id)?;
        if binding.validate().is_err()
            || binding.sandbox_id != self.config.sandbox_id
            || Some(&binding.home_identity) != self.config.home_identity.as_ref()
            || binding.generation > self.config.generation
            || binding.source_format
                != layer3_adapter::runtime_ledger_maintenance::LedgerSourceFormatV1::V2
        {
            return Err(invalid());
        }
        Ok(())
    }
}

pub(super) fn pending(db: &Connection) -> Result<bool> {
    let slot: Option<String> = db
        .query_row(
            "SELECT operation_id FROM collection_pending WHERE singleton=1",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(|_| invalid())?;
    let evidence: Option<String> = db
        .query_row(
            "SELECT operation_id FROM collection_evidence WHERE pending=1",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(|_| invalid())?;
    if slot != evidence {
        return Err(invalid());
    }
    Ok(slot.is_some())
}

pub(super) fn require_slot(db: &Connection, operation: &str) -> Result<()> {
    if !pending(db)? {
        return Err(invalid());
    }
    let current: Option<String> = db
        .query_row(
            "SELECT operation_id FROM collection_pending WHERE singleton=1",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(|_| invalid())?;
    if current.as_deref() != Some(operation) {
        return Err(invalid());
    }
    Ok(())
}

pub(super) fn read(
    db: &Connection,
    accepted: &LedgerBindingV1,
    expected: &CollectionReference,
) -> Result<CollectionEvidence> {
    reference::validate(expected)?;
    let reference = reference::load(db, &expected.operation_id)?;
    let originals = super::collection_artifacts::read(db, &expected.operation_id)?;
    if &reference != expected {
        return Err(invalid());
    }
    let evidence = CollectionEvidence::parse(accepted, &reference.group_key, originals)?;
    if reference::from_evidence(&evidence, reference.plan_sha256.clone())? != reference {
        return Err(invalid());
    }
    Ok(evidence)
}

/// Verify that archival belongs to the one durably retained intent. This is
/// internal consistency only; the coordinator still validates accepted authority.
pub(super) fn require_selection(
    db: &Connection,
    config: &crate::config::RuntimeConfig,
    proof: &super::GroupProof,
    revision: u64,
) -> Result<()> {
    if !pending(db)? {
        return Err(invalid());
    }
    let operation:String=db.query_row("SELECT e.operation_id FROM collection_evidence e JOIN collection_pending p ON e.operation_id=p.operation_id WHERE p.singleton=1 AND e.group_key=?",
        [&proof.key],|r|r.get(0)).map_err(|_|invalid())?;
    let reference = reference::load(db, &operation)?;
    let [_, request, _] = super::collection_artifacts::read(db, &reference.operation_id)?;
    let accepted = layer3_adapter::runtime_ledger_collection::CollectionRequest::decode(&request)
        .ok_or_else(invalid)?
        .binding;
    read(db, &accepted, &reference)?.validate_source(config, proof, revision)
}

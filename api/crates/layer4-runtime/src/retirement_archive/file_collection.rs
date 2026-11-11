//! V1 original-byte retention and explicit observed-source revision. A pending
//! operation must be reconciled before any fresh observation replaces its census.
use super::{
    collection, file_collection_state as state, invalid, storage_error, Archive,
    CollectionReference, GroupProof, Version,
};
use crate::{
    failure::Result, ledger_import::collection_source, ledger_reclamation::CollectionEvidence,
};
use layer3_adapter::runtime_ledger_maintenance::{LedgerBindingV1, LedgerSourceFormatV1};
use rusqlite::Connection;

impl Archive {
    pub(crate) async fn observe_file_collection(&self) -> Result<u64> {
        self.verify()?;
        if self.version != Version::FileCollection {
            return Err(invalid());
        }
        let config = self.config.clone();
        let result = self
            .storage
            .call(move |db| {
                let tx = db.transaction().map_err(|_| storage_error())?;
                let unfinished: bool = tx
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM retired_groups WHERE complete=0)",
                        [],
                        |r| r.get(0),
                    )
                    .map_err(|_| invalid())?;
                if collection::pending(&tx)? || unfinished {
                    return Err(invalid());
                }
                let snapshot = collection_source::scan(&config)?;
                let previous = state::read(&tx)?;
                let revision = match previous {
                    Some((revision, old)) if old == snapshot => revision,
                    Some((revision, _)) => revision.checked_add(1).ok_or_else(invalid)?,
                    None => 1,
                };
                if collection_source::scan(&config)? != snapshot {
                    return Err(invalid());
                }
                state::write(&tx, revision, &snapshot)?;
                tx.commit().map_err(|_| storage_error())?;
                Ok(revision)
            })
            .await?;
        self.verify()?;
        Ok(result)
    }

    pub(crate) async fn prepare_file_collection(
        &self,
        evidence: CollectionEvidence,
        plan_sha256: String,
    ) -> Result<(CollectionReference, GroupProof)> {
        self.verify_collection_binding(evidence.binding())?;
        if self.version != Version::FileCollection {
            return Err(invalid());
        }
        let accepted = evidence.binding().clone();
        let group = evidence.group_key().to_owned();
        let reference = self.retain_collection(evidence, plan_sha256).await?;
        let expected = reference.clone();
        let config = self.config.clone();
        let (proof, snapshot) = self
            .storage
            .call(move |db| {
                let evidence = collection::read(db, &accepted, &expected)?;
                if let Some(proof) = prior_proof(db, &group)? {
                    let receipt = super::existing(db, &proof)?.ok_or_else(invalid)?;
                    super::verification::group(db, &config, &proof, &receipt)?;
                    if proof.observation_sha256 != evidence.observation_sha256() {
                        return Err(invalid());
                    }
                    return Ok((proof, None));
                }
                let (revision, snapshot) = state::read(db)?.ok_or_else(invalid)?;
                if collection_source::scan(&config)? != snapshot {
                    return Err(invalid());
                }
                let proof = snapshot.proof(&config, &group, evidence.observation_sha256())?;
                evidence.validate_source_format(
                    &config,
                    &proof,
                    revision,
                    LedgerSourceFormatV1::V1,
                )?;
                Ok((proof, Some(snapshot)))
            })
            .await?;
        if let Some(snapshot) = snapshot {
            let config = self.config.clone();
            let indexes = snapshot.group_indexes(&proof.key);
            self.prepare(proof.clone(), move |ordinal| {
                snapshot.read_record(&config, *indexes.get(ordinal as usize).ok_or_else(invalid)?)
            })
            .await?;
        }
        self.verify()?;
        Ok((reference, proof))
    }
}

pub(super) fn require_selection(
    db: &Connection,
    config: &crate::config::RuntimeConfig,
    proof: &GroupProof,
) -> Result<()> {
    let operation: String = db
        .query_row(
            "SELECT operation_id FROM collection_pending WHERE singleton=1",
            [],
            |r| r.get(0),
        )
        .map_err(|_| invalid())?;
    let reference = super::collection_reference::load(db, &operation)?;
    if reference.archive_version != 4 || reference.group_key != proof.key {
        return Err(invalid());
    }
    let [_, bytes, _] = super::collection_artifacts::read(db, &operation)?;
    let accepted: LedgerBindingV1 =
        layer3_adapter::runtime_ledger_collection::CollectionRequest::decode(&bytes)
            .ok_or_else(invalid)?
            .binding;
    let evidence = collection::read(db, &accepted, &reference)?;
    let (revision, _) = state::read(db)?.ok_or_else(invalid)?;
    evidence.validate_source_format(config, proof, revision, LedgerSourceFormatV1::V1)
}

fn prior_proof(db: &Connection, group: &str) -> Result<Option<GroupProof>> {
    use rusqlite::OptionalExtension;
    db.query_row(
        "SELECT source_sha256,observation_sha256,records FROM retired_groups WHERE group_key=?",
        [group],
        |r| {
            Ok(GroupProof {
                key: group.into(),
                source_sha256: r.get(0)?,
                observation_sha256: r.get(1)?,
                records: r.get(2)?,
            })
        },
    )
    .optional()
    .map_err(|_| invalid())
}

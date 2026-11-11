//! Held V1 moves preserve every original in both the archive and retained files.
//! No marker or owned local guard is a constructor for qualified C authority.
use super::{
    collection, file_collection_state as state, invalid, storage_error, Archive,
    CollectionCompletion, CollectionReference, GroupProof, Version,
};
use crate::{
    failure::Result, ledger_import::collection_source,
    mutation_ledger::retirement_inventory::DeactivationReceipt,
};
use layer3_adapter::runtime_ledger_maintenance::{LedgerBindingV1, LedgerSourceFormatV1};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy)]
pub(super) enum Boundary {
    BeforeFiles,
    FileMoved,
    FilesVerified,
    Acknowledged,
    Closed,
}

pub(crate) async fn complete_file_collection_held(
    archive: Archive,
    accepted: LedgerBindingV1,
    reference: CollectionReference,
    proof: GroupProof,
    exclusion: impl Send + 'static,
) -> Result<CollectionCompletion> {
    complete_checked(archive, accepted, reference, proof, exclusion, |_| Ok(())).await
}

pub(super) async fn complete_checked(
    archive: Archive,
    accepted: LedgerBindingV1,
    reference: CollectionReference,
    proof: GroupProof,
    exclusion: impl Send + 'static,
    boundary: impl FnMut(Boundary) -> Result<()> + Send + 'static,
) -> Result<CollectionCompletion> {
    tokio::spawn(async move {
        let boundary = Arc::new(Mutex::new(boundary));
        // The spawned owner survives cancellation of the awaiting caller. The
        // file job also owns this guard until every descriptor has been settled.
        let exclusion = Arc::new(Mutex::new(exclusion));
        let result = apply(
            &archive,
            accepted,
            reference,
            proof,
            exclusion.clone(),
            boundary.clone(),
        )
        .await;
        let closed = archive.close().await;
        let receipt = result?;
        closed?;
        boundary.lock().map_err(|_| invalid())?(Boundary::Closed)?;
        Ok(receipt)
    })
    .await
    .map_err(|_| storage_error())?
}

async fn apply(
    archive: &Archive,
    accepted: LedgerBindingV1,
    reference: CollectionReference,
    proof: GroupProof,
    exclusion: Arc<Mutex<impl Send + 'static>>,
    boundary: Arc<Mutex<impl FnMut(Boundary) -> Result<()> + Send + 'static>>,
) -> Result<CollectionCompletion> {
    archive.verify_collection_binding(&accepted)?;
    if archive.version != Version::FileCollection {
        return Err(invalid());
    }
    let evidence = archive
        .read_collection(accepted.clone(), reference.clone())
        .await?;
    if let Some(receipt) = archive
        .collection_completion(accepted.clone(), reference.clone())
        .await?
    {
        receipt.validate(&reference, &evidence, &proof)?;
        return Ok(receipt);
    }
    let config = archive.config.clone();
    let archive_id = archive.archive_id.clone();
    let expected = reference.clone();
    let selected = proof.clone();
    let job_boundary = boundary.clone();
    let (outcome, next) = archive
        .storage
        .call(move |db| {
            let _owned_exclusion = exclusion;
            collection::require_slot(db, &expected.operation_id)?;
            let evidence = collection::read(db, &accepted, &expected)?;
            let (revision, before) = state::read(db)?.ok_or_else(invalid)?;
            evidence.validate_source_format(
                &config,
                &selected,
                revision,
                LedgerSourceFormatV1::V1,
            )?;
            let receipt = super::existing(db, &selected)?.ok_or_else(invalid)?;
            if receipt.complete {
                return Err(invalid());
            }
            super::verification::group(db, &config, &selected, &receipt)?;
            before.require_partial(&collection_source::scan(&config)?, &selected.key)?;
            job_boundary.lock().map_err(|_| invalid())?(Boundary::BeforeFiles)?;
            super::verification::visit(db, &config, &selected, &receipt, |record| {
                before.transfer(&config, &archive_id, record)?;
                job_boundary.lock().map_err(|_| invalid())?(Boundary::FileMoved)
            })?;
            let next = before.without(&selected.key);
            if collection_source::scan(&config)? != next {
                return Err(invalid());
            }
            job_boundary.lock().map_err(|_| invalid())?(Boundary::FilesVerified)?;
            let [operations, authority] = before.counts(&selected.key);
            if operations == 0
                || authority == 0
                || operations + authority != u64::from(selected.records)
            {
                return Err(invalid());
            }
            Ok((
                DeactivationReceipt {
                    source_sha256: selected.source_sha256,
                    before_sha256: before.hash()?,
                    after_sha256: next.hash()?,
                    before_revision: revision,
                    after_revision: revision.checked_add(1).ok_or_else(invalid)?,
                    operations,
                    authority,
                    cleanup: 0,
                    free_pages_before: 0,
                    free_pages_after: 0,
                },
                next,
            ))
        })
        .await?;
    let result = archive
        .acknowledge_with_file_source(reference, evidence, proof, outcome, Some(next))
        .await?;
    boundary.lock().map_err(|_| invalid())?(Boundary::Acknowledged)?;
    Ok(result)
}

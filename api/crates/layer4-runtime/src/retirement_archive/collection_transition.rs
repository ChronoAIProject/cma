//! Crash-resumable held completion. The pending archive blocks serving until
//! source reconciliation and its immutable acknowledgement have both succeeded.
use layer3_adapter::runtime_ledger_maintenance::LedgerBindingV1;
use std::sync::{Arc, Mutex};

use super::indexed_deactivation::{selection, Eligibility};
use super::{
    invalid, storage_error, Archive, CollectionCompletion, CollectionReference, GroupProof,
};
use crate::{failure::Result, mutation_ledger::Ledger};

#[derive(Clone, Copy)]
pub(super) enum CollectionBoundary {
    Selected,
    SourceCommitted,
    Acknowledged,
    Closed,
}

/// Internal held core, not a public maintenance command. Real use requires C's
/// qualified exclusion and installed serving dependency before retaining intent.
/// The generic owned guard is not itself evidence of that authority.
pub(crate) async fn complete_collection_held(
    archive: Archive,
    source: Ledger,
    accepted: LedgerBindingV1,
    reference: CollectionReference,
    proof: GroupProof,
    exclusion: impl Send + 'static,
) -> Result<CollectionCompletion> {
    complete_checked(
        archive,
        source,
        accepted,
        reference,
        proof,
        exclusion,
        |_| Ok(()),
    )
    .await
}

pub(super) async fn complete_checked(
    archive: Archive,
    source: Ledger,
    accepted: LedgerBindingV1,
    reference: CollectionReference,
    proof: GroupProof,
    exclusion: impl Send + 'static,
    mut boundary: impl FnMut(CollectionBoundary) -> Result<()> + Send + 'static,
) -> Result<CollectionCompletion> {
    tokio::spawn(async move {
        let archive = Arc::new(archive);
        let exclusion = Arc::new(Mutex::new(exclusion));
        let result = apply(
            &archive,
            &source,
            accepted,
            reference,
            proof,
            exclusion.clone(),
            &mut boundary,
        )
        .await;
        let source_closed = source.close().await;
        let archive_closed = match Arc::try_unwrap(archive) {
            Ok(archive) => archive.close().await,
            Err(_) => Err(invalid()),
        };
        let receipt = result?;
        source_closed?;
        archive_closed?;
        boundary(CollectionBoundary::Closed)?;
        Ok(receipt)
    })
    .await
    .map_err(|_| storage_error())?
}

async fn apply(
    archive: &Arc<Archive>,
    source: &Ledger,
    accepted: LedgerBindingV1,
    reference: CollectionReference,
    proof: GroupProof,
    exclusion: Arc<Mutex<impl Send + 'static>>,
    boundary: &mut (impl FnMut(CollectionBoundary) -> Result<()> + Send),
) -> Result<CollectionCompletion> {
    archive.verify_collection(&accepted.archive_id)?;
    let evidence = archive
        .read_collection(accepted.clone(), reference.clone())
        .await?;
    if let Some(receipt) = archive
        .collection_completion(accepted.clone(), reference.clone())
        .await?
    {
        archive.verify_indexed(proof.clone()).await?;
        receipt.validate(&reference, &evidence, &proof)?;
        // A cold acknowledgement is independent of later active-store work.
        return Ok(receipt);
    }
    let selected = selection(
        archive,
        proof.clone(),
        Eligibility::Collected(Box::new(evidence)),
    )
    .await?;
    boundary(CollectionBoundary::Selected)?;
    let outcome = source
        .deactivate_retirement(selected, (archive.clone(), exclusion))
        .await?;
    boundary(CollectionBoundary::SourceCommitted)?;
    let evidence = archive.read_collection(accepted, reference.clone()).await?;
    let receipt = archive
        .acknowledge_collection(reference, evidence, proof, outcome)
        .await?;
    boundary(CollectionBoundary::Acknowledged)?;
    Ok(receipt)
}

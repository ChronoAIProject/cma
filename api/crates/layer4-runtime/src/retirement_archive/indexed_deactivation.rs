//! Own both stores and the caller's qualified exclusion for the whole transition.
//! This held core has no retained-home command or activation publisher.
use std::sync::{Arc, Mutex};

use super::{invalid, storage_error, Archive, GroupProof};
use crate::failure::Result;
use crate::mutation_ledger::{
    retirement_inventory::{
        Context, DeactivationReceipt, Selection, MAX_SELECTION_BYTES, MAX_SELECTION_ROWS,
    },
    Ledger,
};

/// Consume both database owners. The caller must already have excluded every
/// old writer and published a required, held archive dependency before any real
/// source use. The observation alone grants none of that authority.
///
/// The detached task owns the exclusion guard across cancellation, verification,
/// commit and close. No caller can accidentally reopen these same handles while
/// their transaction is still executing. Every archive group remains pending.
pub(crate) async fn deactivate_held(
    archive: Archive,
    source: Ledger,
    proof: GroupProof,
    observation: Vec<u8>,
    exclusion: impl Send + 'static,
) -> Result<DeactivationReceipt> {
    deactivate_checked(archive, source, proof, observation, exclusion, |_| Ok(())).await
}

#[derive(Clone, Copy)]
pub(super) enum Boundary {
    Verified,
    Committed,
    Closed,
}

pub(super) async fn deactivate_checked(
    archive: Archive,
    source: Ledger,
    proof: GroupProof,
    observation: Vec<u8>,
    exclusion: impl Send + 'static,
    mut boundary: impl FnMut(Boundary) -> Result<()> + Send + 'static,
) -> Result<DeactivationReceipt> {
    tokio::spawn(async move {
        let exclusion = Arc::new(Mutex::new(exclusion));
        let archive = Arc::new(archive);
        let result = apply(
            &archive,
            &source,
            proof,
            observation,
            &mut boundary,
            exclusion.clone(),
        )
        .await;
        // Always close both owners before replying, including validation refusal.
        let source_closed = source.close().await;
        let archive_closed = match Arc::try_unwrap(archive) {
            Ok(archive) => archive.close().await,
            Err(_) => Err(invalid()),
        };
        let receipt = result?;
        source_closed?;
        archive_closed?;
        boundary(Boundary::Closed)?;
        Ok(receipt)
    })
    .await
    .map_err(|_| storage_error())?
}

async fn apply(
    archive: &Arc<Archive>,
    source: &Ledger,
    proof: GroupProof,
    observation: Vec<u8>,
    boundary: &mut (impl FnMut(Boundary) -> Result<()> + Send),
    exclusion: Arc<Mutex<impl Send + 'static>>,
) -> Result<DeactivationReceipt> {
    archive.verify()?;
    super::require_reserve(&archive.path, archive.anchor.as_deref())?;
    if proof.records == 0 || proof.records as usize > MAX_SELECTION_ROWS {
        return Err(invalid());
    }
    let bounded_proof = proof.clone();
    archive
        .storage
        .call(move |db| {
            let receipt = super::existing(db, &bounded_proof)
                .map_err(|_| invalid())?
                .ok_or_else(invalid)?;
            if receipt.complete || receipt.bytes > 64 * 1024 * 1024 {
                return Err(invalid());
            }
            Ok(())
        })
        .await?;
    let collected = Arc::new(Mutex::new((None::<Context>, Vec::new(), 0_usize)));
    let output = collected.clone();
    let retained = archive
        .visit_indexed_sources(proof.clone(), move |context, row| {
            let mut output = output.lock().map_err(|_| invalid())?;
            if output.0.as_ref().is_some_and(|prior| prior != context) {
                return Err(invalid());
            }
            output.0.get_or_insert_with(|| context.clone());
            output.2 = output.2.checked_add(row.bytes()).ok_or_else(invalid)?;
            if output.1.len() == MAX_SELECTION_ROWS || output.2 > MAX_SELECTION_BYTES {
                return Err(invalid());
            }
            output.1.push(row);
            Ok(())
        })
        .await?;
    if retained.complete {
        return Err(invalid());
    }
    let (context, rows, _) = std::mem::take(&mut *collected.lock().map_err(|_| invalid())?);
    let selection = Selection::new(
        &archive.config,
        proof,
        context.ok_or_else(invalid)?,
        rows,
        &observation,
    )?;
    archive.verify()?;
    boundary(Boundary::Verified)?;
    let receipt = source
        .deactivate_retirement(selection, (archive.clone(), exclusion))
        .await?;
    boundary(Boundary::Committed)?;
    Ok(receipt)
}

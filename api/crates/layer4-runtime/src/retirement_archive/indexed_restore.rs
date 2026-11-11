//! Bounded archive-to-source restore inspection. This owns both handles until
//! close, but neither establishes exclusion nor admits any restoring write.
use std::sync::{Arc, Mutex};

use super::{existing, invalid, Archive, GroupProof, Receipt, Version};
use crate::failure::Result;
use crate::mutation_ledger::{
    retirement_inventory::{
        restore::RestoreSourceAssessment, Context, DeactivationReceipt, Selection,
        MAX_SELECTION_BYTES, MAX_SELECTION_ROWS,
    },
    Ledger,
};

pub(crate) struct RestoreInspection {
    pub(crate) archive_id: String,
    pub(crate) proof: GroupProof,
    pub(crate) observation: Vec<u8>,
    pub(crate) transition: DeactivationReceipt,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct HeldRestoreAssessment {
    pub(crate) format: &'static str,
    pub(crate) archive_records: u64,
    pub(crate) archive_bytes: u64,
    pub(crate) archive_group_complete: bool,
    pub(crate) source: RestoreSourceAssessment,
}

/// Consuming the handles prevents accidental reuse while an abandoned caller's
/// read is still running. This does not change a serving/maintenance marker.
pub(crate) async fn assess_restore_held(
    archive: Archive,
    source: Ledger,
    request: RestoreInspection,
) -> Result<HeldRestoreAssessment> {
    inspect_and_close(archive, source, request, || Ok(())).await
}

async fn inspect_and_close(
    archive: Archive,
    source: Ledger,
    request: RestoreInspection,
    mut archive_verified: impl FnMut() -> Result<()> + Send + 'static,
) -> Result<HeldRestoreAssessment> {
    tokio::spawn(async move {
        let result = inspect(&archive, &source, request, &mut archive_verified).await;
        // Await both closes even when inspection or the first close fails.
        let source_closed = source.close().await;
        let archive_closed = archive.close().await;
        let assessment = result?;
        source_closed?;
        archive_closed?;
        Ok(assessment)
    })
    .await
    .map_err(|_| invalid())?
}

async fn inspect(
    archive: &Archive,
    source: &Ledger,
    request: RestoreInspection,
    archive_verified: &mut (impl FnMut() -> Result<()> + Send),
) -> Result<HeldRestoreAssessment> {
    archive.verify()?;
    let id = uuid::Uuid::parse_str(&request.archive_id).map_err(|_| invalid())?;
    if archive.version != Version::Indexed
        || id.is_nil()
        || id.to_string() != request.archive_id
        || request.proof.records == 0
        || request.proof.records as usize > MAX_SELECTION_ROWS
        || request.observation.len()
            > layer3_adapter::runtime_ledger_retirement::MAX_LEDGER_RETIREMENT_SNAPSHOT_BYTES
    {
        return Err(invalid());
    }
    let proof = request.proof.clone();
    let receipt = archive
        .storage
        .call(move |db| {
            // Normalize inspection errors inside Storage::call, keeping reserve
            // release strictly out of this read-only path.
            let tx = db.transaction().map_err(|_| invalid())?;
            let matches: bool = tx
                .query_row(
                    "SELECT archive_id=? FROM archive_metadata WHERE singleton=1",
                    [request.archive_id],
                    |row| row.get(0),
                )
                .map_err(|_| invalid())?;
            let receipt = existing(&tx, &proof)
                .map_err(|_| invalid())?
                .ok_or_else(invalid)?;
            if !matches || receipt.bytes > 64 * 1024 * 1024 {
                return Err(invalid());
            }
            tx.rollback().map_err(|_| invalid())?;
            Ok(receipt)
        })
        .await?;
    let collected = Arc::new(Mutex::new((None::<Context>, Vec::new(), 0_usize)));
    let output = collected.clone();
    // Reuse the verifier that checks every payload and permanent replay index
    // before it calls us with the first source row, in the same read transaction.
    let verified = archive
        .visit_indexed_sources(request.proof.clone(), move |context, row| {
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
    if verified != receipt {
        return Err(invalid());
    }
    let (context, rows, _) = std::mem::take(&mut *collected.lock().map_err(|_| invalid())?);
    let selection = Selection::new(
        &archive.config,
        request.proof,
        context.ok_or_else(invalid)?,
        rows,
        &request.observation,
    )?;
    archive.verify()?;
    archive_verified()?;
    let source = source
        .assess_retirement_restore(selection, request.transition)
        .await?;
    let Receipt {
        records,
        bytes,
        complete,
    } = verified;
    Ok(HeldRestoreAssessment {
        format: "runtime-ledger-held-restore-assessment-v1",
        archive_records: records,
        archive_bytes: bytes,
        archive_group_complete: complete,
        source,
    })
}

#[cfg(test)]
pub(super) async fn test_inspect_and_close(
    archive: Archive,
    source: Ledger,
    request: RestoreInspection,
    boundary: impl FnMut() -> Result<()> + Send + 'static,
) -> Result<HeldRestoreAssessment> {
    inspect_and_close(archive, source, request, boundary).await
}

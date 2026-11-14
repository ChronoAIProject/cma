//! A terminal purge may erase custody only while permanent creation identity remains.
use super::super::agent_log_integrity::TransactionLogError;
use super::*;
use layer2_core::sandbox::store::SandboxStoreError;

pub(in crate::store) async fn require_finality(
    store: &MongoSandboxStore,
    rows: &[Document],
    session: &mut mongodb::ClientSession,
) -> Result<(), TransactionLogError> {
    for row in rows {
        let effect = read(row).map_err(|_| SandboxStoreError::Conflict)?;
        if effect.unknown() {
            tracing::warn!("sandbox purge refused: unresolved automatic title provider effect");
            return Err(SandboxStoreError::Conflict.into());
        }
        if effect.accepted().is_some() {
            let id = row
                .get_str("_id")
                .map_err(|_| SandboxStoreError::Conflict)?;
            let provenance = store
                .agent_creation_provenance
                .find_one(doc! {"_id": id})
                .session(&mut *session)
                .await
                .map_err(TransactionLogError::Driver)?
                .ok_or(SandboxStoreError::Conflict)?;
            super::super::agent_provenance::read(&provenance)?;
            // Production admission inserts this same permanent unique key; the
            // low-level creation port also refuses its reuse. Neither purge nor
            // any title/fallback/rollback writer deletes creation provenance.
        }
    }
    Ok(())
}

//! Operator-only checks at durable operation boundaries whose resource steps
//! subsequently retain the existing platform spend checks.

use layer2_core::sandbox::store::SandboxStoreError;
use mongodb::{
    bson::{doc, Document},
    ClientSession, Collection,
};

/// Existing transactions retain their in-snapshot admission read.
pub(super) async fn require_fresh(
    settings: &Collection<Document>,
    session: &mut ClientSession,
) -> Result<(), SandboxStoreError> {
    let document = settings
        .find_one(doc! {"_id": "platform"})
        .session(session)
        .await
        .map_err(|_| SandboxStoreError::Unavailable("admission control read failed".into()))?;
    require_readable_open(document.as_ref())
}

/// Single-document writes keep their original retryable write behavior. Read
/// majority control after replay validation and immediately before the write.
pub(super) async fn require_fresh_read(
    settings: &Collection<Document>,
) -> Result<(), SandboxStoreError> {
    let document = settings
        .find_one(doc! {"_id": "platform"})
        .await
        .map_err(|_| SandboxStoreError::Unavailable("admission control read failed".into()))?;
    require_readable_open(document.as_ref())
}

fn require_readable_open(document: Option<&Document>) -> Result<(), SandboxStoreError> {
    let Some(document) = document else {
        return Ok(());
    };
    match super::admission_control::from_document(document) {
        Ok(control) if control.operator_stop.is_none() => Ok(()),
        // Unreadable control fails closed for Fresh work only. Admitted work
        // never calls this operator-only gate.
        _ => Err(SandboxStoreError::AdmissionPaused),
    }
}

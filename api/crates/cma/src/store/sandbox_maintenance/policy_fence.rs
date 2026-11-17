//! Serialize global inactivity admission with mutable platform settings.
use layer2_core::sandbox::store::{SandboxInactivityPauseRequest, SandboxStoreError};
use mongodb::{bson::doc, ClientSession};

use super::transaction::ApplyError;
use crate::store::sandbox_mongo::{signed, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION};

pub(super) async fn fence(
    store: &MongoSandboxStore,
    request: &SandboxInactivityPauseRequest,
    session: &mut ClientSession,
) -> Result<bool, ApplyError> {
    // This existing handle addresses the shared platform_settings singleton;
    // inactivity owns its own outer fence, independent of budget counters.
    let Some(document) = store
        .budget_settings
        .find_one(doc! { "_id": "platform" })
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
    else {
        return Ok(false);
    };
    let stored = crate::store::settings::read_settings(document)
        .map_err(|error| ApplyError::Domain(SandboxStoreError::Corrupt(error.to_string())))?;
    if !request.matches_policy(&stored) {
        return Ok(false);
    }
    let result = store
        .budget_settings
        .update_one(
            doc! {
                "_id": "platform", "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                "revision": signed(stored.revision, "inactivity settings revision")
                    .map_err(|error| ApplyError::Domain(SandboxStoreError::Corrupt(error)))?,
            },
            doc! { "$inc": { "inactivity_admission_fence": 1_i64 } },
        )
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?;
    if result.matched_count != 1 {
        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
    }
    Ok(true)
}

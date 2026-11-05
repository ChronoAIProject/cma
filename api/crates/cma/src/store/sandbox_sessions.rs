//! Short lived desktop session leases used by inactivity admission.

use layer2_core::sandbox::store::{SandboxSessionStore, SandboxStoreError};
use layer2_core::{seconds_from_rfc3339, SandboxId};
use mongodb::bson::doc;
use mongodb::options::IndexOptions;
use mongodb::IndexModel;

use super::sandbox_mongo::{is_duplicate_key, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION};

const MAX_SESSION_ID_BYTES: usize = 128;

#[async_trait::async_trait]
impl SandboxSessionStore for MongoSandboxStore {
    async fn open_sandbox_session(
        &self,
        sandbox_id: &SandboxId,
        session_id: &str,
        lease_expires_at: &str,
    ) -> Result<(), SandboxStoreError> {
        validate(sandbox_id, session_id, lease_expires_at)?;
        fence_active_sandbox(self, sandbox_id).await?;
        self.sandbox_sessions
            .insert_one(doc! {
                "_id": session_id,
                "sandbox_id": sandbox_id.as_str(),
                "lease_expires_at": lease_expires_at,
            })
            .await
            .map(|_| ())
            .map_err(|error| {
                if is_duplicate_key(&error) {
                    SandboxStoreError::Conflict
                } else {
                    SandboxStoreError::Unavailable(error.to_string())
                }
            })
    }

    async fn renew_sandbox_session(
        &self,
        sandbox_id: &SandboxId,
        session_id: &str,
        lease_expires_at: &str,
    ) -> Result<(), SandboxStoreError> {
        validate(sandbox_id, session_id, lease_expires_at)?;
        let changed = self
            .sandbox_sessions
            .update_one(
                doc! { "_id": session_id, "sandbox_id": sandbox_id.as_str() },
                doc! { "$set": { "lease_expires_at": lease_expires_at } },
            )
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
        if changed.matched_count != 1 {
            return Err(SandboxStoreError::NotFound);
        }
        fence_active_sandbox(self, sandbox_id).await
    }

    async fn close_sandbox_session(
        &self,
        sandbox_id: &SandboxId,
        session_id: &str,
    ) -> Result<(), SandboxStoreError> {
        if sandbox_id.as_str().trim().is_empty()
            || session_id.trim().is_empty()
            || session_id.len() > MAX_SESSION_ID_BYTES
        {
            return Err(SandboxStoreError::InvalidTransaction(
                "sandbox session request is invalid",
            ));
        }
        let removed = self
            .sandbox_sessions
            .delete_one(doc! { "_id": session_id, "sandbox_id": sandbox_id.as_str() })
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
        if removed.deleted_count == 1 {
            fence_active_sandbox(self, sandbox_id).await?;
        }
        Ok(())
    }

    async fn has_open_sandbox_session(
        &self,
        sandbox_id: &SandboxId,
        observed_at: &str,
    ) -> Result<bool, SandboxStoreError> {
        if sandbox_id.as_str().trim().is_empty() || seconds_from_rfc3339(observed_at).is_none() {
            return Err(SandboxStoreError::InvalidTransaction(
                "sandbox session observation is invalid",
            ));
        }
        self.sandbox_sessions
            .find_one(doc! {
                "sandbox_id": sandbox_id.as_str(),
                "lease_expires_at": { "$gt": observed_at },
            })
            .await
            .map(|row| row.is_some())
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))
    }
}

pub(super) async fn ensure_indexes(
    collection: &mongodb::Collection<mongodb::bson::Document>,
) -> Result<(), layer2_core::StoreError> {
    collection
        .create_index(
            IndexModel::builder()
                .keys(doc! { "sandbox_id": 1, "lease_expires_at": 1 })
                .options(
                    IndexOptions::builder()
                        .name("sandbox_session_expiry".to_owned())
                        .build(),
                )
                .build(),
        )
        .await
        .map(|_| ())
        .map_err(|error| layer2_core::StoreError::Unavailable(error.to_string()))
}

async fn fence_active_sandbox(
    store: &MongoSandboxStore,
    sandbox_id: &SandboxId,
) -> Result<(), SandboxStoreError> {
    let changed = store
        .sandboxes
        .update_one(
            doc! {
                "_id": sandbox_id.as_str(),
                "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                "status": "active",
            },
            doc! { "$inc": { "desktop_activity_fence": 1_i64 } },
        )
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
    if changed.matched_count == 1 {
        Ok(())
    } else {
        Err(SandboxStoreError::Conflict)
    }
}

fn validate(
    sandbox_id: &SandboxId,
    session_id: &str,
    lease_expires_at: &str,
) -> Result<(), SandboxStoreError> {
    if sandbox_id.as_str().trim().is_empty()
        || session_id.trim().is_empty()
        || session_id.len() > MAX_SESSION_ID_BYTES
        || seconds_from_rfc3339(lease_expires_at).is_none()
    {
        return Err(SandboxStoreError::InvalidTransaction(
            "sandbox session request is invalid",
        ));
    }
    Ok(())
}

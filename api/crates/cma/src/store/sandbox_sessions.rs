//! Short lived desktop session leases used by inactivity admission.

use layer2_core::sandbox::store::{SandboxSessionStore, SandboxStoreError};
use layer2_core::{seconds_from_rfc3339, SandboxId};
use mongodb::bson::{doc, Document};
use mongodb::options::IndexOptions;
use mongodb::IndexModel;

use super::sandbox_mongo::{is_duplicate_key, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION};

pub(super) const MAX_SESSION_ID_BYTES: usize = 128;
pub(super) const EXPIRY_INDEX: &str = "sandbox_session_expiry";
pub(super) const LEGACY_V1_FIELD_COUNT: usize = 3;

mod exclusive;

#[async_trait::async_trait]
impl SandboxSessionStore for MongoSandboxStore {
    async fn acquire_exclusive_sandbox_session(
        &self,
        request: &layer2_core::sandbox::store::DesktopSessionRequest,
    ) -> Result<layer3_adapter::DesktopOwnership, SandboxStoreError> {
        exclusive::claim(self, request, None).await
    }

    async fn renew_exclusive_sandbox_session(
        &self,
        request: &layer2_core::sandbox::store::DesktopSessionRequest,
        authority: &layer3_adapter::DesktopOwnership,
    ) -> Result<layer3_adapter::DesktopOwnership, SandboxStoreError> {
        exclusive::claim(self, request, Some(authority)).await
    }

    async fn release_exclusive_sandbox_session(
        &self,
        authority: &layer3_adapter::DesktopOwnership,
    ) -> Result<(), SandboxStoreError> {
        exclusive::release(self, authority).await
    }

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
                "document_schema_version": DOCUMENT_SCHEMA_VERSION,
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
                doc! {
                    "_id": session_id, "sandbox_id": sandbox_id.as_str(),
                    "$or": [
                        { "document_schema_version": {
                            "$eq": DOCUMENT_SCHEMA_VERSION, "$type": ["int", "long"],
                            "$not": { "$type": "array" },
                        } },
                        { "document_schema_version": { "$exists": false } },
                    ],
                },
                doc! { "$set": {
                    "lease_expires_at": lease_expires_at,
                    "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                } },
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

/// Only the exact unversioned shape written by #1341 is a known v1 lease.
pub(super) fn is_legacy_v1(document: &Document) -> bool {
    if document.len() != LEGACY_V1_FIELD_COUNT {
        return false;
    }
    let (Ok(id), Ok(sandbox_id), Ok(expiry)) = (
        document.get_str("_id"),
        document.get_str("sandbox_id"),
        document.get_str("lease_expires_at"),
    ) else {
        return false;
    };
    validate(&SandboxId(sandbox_id.to_owned()), id, expiry).is_ok()
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
                        .name(EXPIRY_INDEX.to_owned())
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
            crate::store::work_schema::query(doc! {
                "_id": sandbox_id.as_str(),
                "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                "status": "active",
            }),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_shape_requires_exact_fields_and_original_writer_validation() {
        let row = doc! {
            "_id": "desktop-lease",
            "sandbox_id": "sbx_legacy",
            "lease_expires_at": "2026-11-06T00:01:30Z",
        };
        assert!(is_legacy_v1(&row));
        for (key, value) in [
            ("_id", " ".into()),
            ("_id", "x".repeat(MAX_SESSION_ID_BYTES + 1)),
            ("sandbox_id", " ".into()),
            ("lease_expires_at", "invalid".into()),
        ] {
            let mut invalid = row.clone();
            invalid.insert(key, value);
            assert!(!is_legacy_v1(&invalid));
        }
        for key in ["document_schema_version", "unknown_field"] {
            let mut invalid = row.clone();
            invalid.insert(key, 1_i64);
            assert!(!is_legacy_v1(&invalid));
        }
    }
}

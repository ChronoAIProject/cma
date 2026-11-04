//! Majority-persisted, version-fenced funding state independent of runtime writes.
use super::sandbox_mongo::MongoSandboxStore;
pub(crate) use crate::sandbox::funding::SandboxBillingState;
use layer2_core::{sandbox::store::SandboxStoreError, SandboxId};
use mongodb::bson::{doc, Document};
use mongodb::{options::IndexOptions, IndexModel};

pub(crate) async fn ensure_indexes(
    collection: &mongodb::Collection<Document>,
) -> Result<(), layer2_core::StoreError> {
    collection
        .create_index(
            IndexModel::builder()
                .keys(doc! {"retry_at":1,"_id":1})
                .options(
                    IndexOptions::builder()
                        .name(Some("sandbox_billing_due".into()))
                        .build(),
                )
                .build(),
        )
        .await
        .map(|_| ())
        .map_err(|e| layer2_core::StoreError::Unavailable(e.to_string()))
}

fn decode(mut document: Document) -> Result<SandboxBillingState, SandboxStoreError> {
    super::sandbox_mongo::require_version(&document).map_err(SandboxStoreError::Corrupt)?;
    if document.get_str("_id").ok() != document.get_str("sandbox_id").ok()
        || document.get_str("_id").is_err()
    {
        return Err(SandboxStoreError::Corrupt(
            "funding state identity differs from its Mongo key".into(),
        ));
    }
    document.remove("_id");
    document.remove("document_schema_version");
    mongodb::bson::deserialize_from_document(document)
        .map_err(|e| SandboxStoreError::Corrupt(format!("sandbox funding state: {e}")))
}

impl MongoSandboxStore {
    pub(crate) async fn sandbox_billing_state(
        &self,
        id: &SandboxId,
    ) -> Result<Option<SandboxBillingState>, SandboxStoreError> {
        self.sandbox_billing
            .find_one(doc! {"_id":id.as_str()})
            .await
            .map_err(|e| SandboxStoreError::Unavailable(e.to_string()))?
            .map(decode)
            .transpose()
    }

    pub(crate) async fn save_sandbox_billing_state(
        &self,
        state: &mut SandboxBillingState,
    ) -> Result<(), SandboxStoreError> {
        let mut next = state.clone();
        next.revision = state
            .revision
            .checked_add(1)
            .ok_or(SandboxStoreError::Conflict)?;
        let mut document = mongodb::bson::serialize_to_document(&next)
            .map_err(|e| SandboxStoreError::Corrupt(e.to_string()))?;
        document.insert("_id", &state.sandbox_id);
        document.insert(
            "document_schema_version",
            super::sandbox_mongo::DOCUMENT_SCHEMA_VERSION,
        );
        if state.revision == 0 {
            match self.sandbox_billing.insert_one(document).await {
                Ok(_) => {}
                Err(e) if super::sandbox_mongo::is_duplicate_key(&e) => {
                    return Err(SandboxStoreError::Conflict)
                }
                Err(e) => return Err(SandboxStoreError::Unavailable(e.to_string())),
            }
        } else {
            let version = i64::try_from(state.revision).map_err(|_| SandboxStoreError::Conflict)?;
            let result = self
                .sandbox_billing
                .replace_one(doc! {"_id":&state.sandbox_id,"revision":version}, document)
                .await
                .map_err(|e| SandboxStoreError::Unavailable(e.to_string()))?;
            if result.matched_count != 1 {
                return Err(SandboxStoreError::Conflict);
            }
        }
        *state = next;
        Ok(())
    }

    pub(crate) async fn due_sandbox_billing_states(
        &self,
        before: u64,
        limit: u16,
    ) -> Result<Vec<SandboxBillingState>, SandboxStoreError> {
        let before = i64::try_from(before)
            .map_err(|_| SandboxStoreError::Corrupt("funding deadline overflow".into()))?;
        let mut cursor = self
            .sandbox_billing
            .find(doc! {"retry_at":{"$lte":before},"$or":[
                {"stopped":false}, {"windows.0":{"$exists":true}},
                {"retained_disk_gib":{"$gt":0}}, {"pending":{"$ne":mongodb::bson::Bson::Null}}
            ]})
            .sort(doc! {"retry_at":1,"_id":1})
            .limit(i64::from(limit))
            .await
            .map_err(|e| SandboxStoreError::Unavailable(e.to_string()))?;
        let mut states = Vec::new();
        while cursor
            .advance()
            .await
            .map_err(|e| SandboxStoreError::Unavailable(e.to_string()))?
        {
            states.push(decode(
                cursor
                    .deserialize_current()
                    .map_err(|e| SandboxStoreError::Corrupt(e.to_string()))?,
            )?);
        }
        Ok(states)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bson_round_trip_strips_mongo_identity() {
        // Production reads must not trip deny_unknown_fields on Mongo's _id.
        let mut doc = doc! {"document_schema_version":1_i64,"_id":"s", "sandbox_id":"s", "owner_id":"o", "revision":1_i64,
        "source_operation":"i", "operation_id":"i", "runtime_generation":1_i64,
        "quote":mongodb::bson::Bson::Null,"pending":mongodb::bson::Bson::Null,"current":mongodb::bson::Bson::Null,
        "coverage_start":0_i64,"windows":[],"evidence_cursor":0_i64,"stopped":true,"last_error":mongodb::bson::Bson::Null,
        "retry_at":0_i64,"attempts":0_i32};
        assert_eq!(decode(doc.clone()).unwrap().revision, 1);
        doc.insert("unexpected", true);
        assert!(decode(doc).is_err());
    }
}

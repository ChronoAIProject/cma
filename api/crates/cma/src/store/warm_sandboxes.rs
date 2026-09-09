//! Durable authority for credential-free runtime preparation and atomic ownership.

use futures_util::TryStreamExt;
use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::{WarmSandbox, WarmSandboxState};
use layer2_core::{SandboxId, SandboxProfileId};
use mongodb::bson::{doc, Bson, Document};
use mongodb::{Collection, IndexModel};

use super::sandbox_mongo::{
    read_record, record_bson, signed, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION,
};

mod admission;
mod planning;
mod work;
pub(super) use admission::claim_ready;
pub(crate) use work::WarmSandboxClaim;

pub(super) async fn ensure_indexes(
    collection: &Collection<Document>,
) -> Result<(), layer2_core::StoreError> {
    collection
        .create_indexes([
            IndexModel::builder()
                .keys(doc! { "profile_id": 1, "state": 1, "_id": 1 })
                .build(),
            IndexModel::builder()
                .keys(doc! { "state": 1, "check_after": 1, "lease_until": 1 })
                .build(),
        ])
        .await
        .map(|_| ())
        .map_err(|error| layer2_core::StoreError::Unavailable(error.to_string()))
}

fn document(record: &WarmSandbox) -> Result<Document, SandboxStoreError> {
    record.validate()?;
    Ok(doc! {
        "_id": record.id.as_str(),
        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "profile_id": record.plan.profile.id.as_str(),
        "state": state(record.state)?,
        "revision": signed(record.revision, "warm Sandbox revision").map_err(SandboxStoreError::Corrupt)?,
        "record": record_bson(record).map_err(SandboxStoreError::Corrupt)?,
    })
}

pub(super) fn read(document: &Document) -> Result<WarmSandbox, SandboxStoreError> {
    let record: WarmSandbox = read_record(document).map_err(SandboxStoreError::Corrupt)?;
    record.validate()?;
    let expected = self::document(&record)?;
    if expected
        .iter()
        .any(|(key, value)| document.get(key) != Some(value))
    {
        return Err(SandboxStoreError::Corrupt(
            "warm Sandbox query fields disagree with record".into(),
        ));
    }
    Ok(record)
}

fn state(value: WarmSandboxState) -> Result<Bson, SandboxStoreError> {
    record_bson(&value).map_err(SandboxStoreError::Corrupt)
}

fn unavailable(error: mongodb::error::Error) -> SandboxStoreError {
    SandboxStoreError::Unavailable(error.to_string())
}

impl MongoSandboxStore {
    pub(crate) async fn warm_sandbox(
        &self,
        id: &SandboxId,
    ) -> Result<Option<WarmSandbox>, SandboxStoreError> {
        self.warm_sandboxes
            .find_one(doc! { "_id": id.as_str() })
            .await
            .map_err(unavailable)?
            .as_ref()
            .map(read)
            .transpose()
    }

    /// Includes disabled/retired Profiles while any unowned runtime needs cleanup.
    pub(crate) async fn warmup_profile_ids(
        &self,
    ) -> Result<Vec<SandboxProfileId>, SandboxStoreError> {
        let mut ids = std::collections::BTreeSet::new();
        let mut profiles = self
            .sandbox_profiles
            .find(doc! {
                "visibility": "public", "retired_at": Bson::Null,
                "record.availability.warmup_enabled": true,
            })
            .projection(doc! { "_id": 1 })
            .await
            .map_err(unavailable)?;
        while let Some(profile) = profiles.try_next().await.map_err(unavailable)? {
            ids.insert(SandboxProfileId(
                profile
                    .get_str("_id")
                    .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?
                    .to_owned(),
            ));
        }
        let records = self
            .warm_sandboxes
            .distinct(
                "profile_id",
                doc! {
                    "state": { "$in": ["preparing", "installing", "ready", "retiring"] },
                },
            )
            .await
            .map_err(unavailable)?;
        for value in records {
            let id = value
                .as_str()
                .ok_or_else(|| SandboxStoreError::Corrupt("invalid warm Profile ID".into()))?;
            ids.insert(SandboxProfileId(id.to_owned()));
        }
        Ok(ids.into_iter().collect())
    }
}

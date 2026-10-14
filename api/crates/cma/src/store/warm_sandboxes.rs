//! Durable authority for credential-free runtime preparation and atomic ownership.

use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::{WarmSandbox, WarmSandboxState};
use layer2_core::SandboxId;
use mongodb::bson::{doc, Bson, Document};
use mongodb::{Collection, IndexModel};

use super::sandbox_mongo::{
    read_record, record_bson, signed, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION,
};

mod admission;
mod adoption;
#[cfg(test)]
mod hierarchy;
pub(super) use adoption::{adopt_agent, adopt_workspace};
#[cfg(test)]
mod planning;
mod retirement;
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

pub(super) fn document(record: &WarmSandbox) -> Result<Document, SandboxStoreError> {
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
}

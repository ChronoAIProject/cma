//! Separate durable authority for a bounded native, credential-free hierarchy pool.

use super::sandbox_mongo::{
    read_record, record_bson, signed, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION,
};
use layer2_core::sandbox::{SandboxStoreError, WarmSandbox};
use layer2_core::SandboxId;
use mongodb::bson::{doc, Document};
use mongodb::{Collection, IndexModel};

mod admission;
mod adoption;
mod authority;
mod model;
mod planning;
#[cfg(test)]
mod tests;
mod tools;
mod work;
pub(super) use admission::claim_ready;
pub(super) use adoption::{adopt_agent, adopt_workspace};
pub(crate) use model::NativePreparedPlan;
use model::Record;
pub(super) use tools::prepared_conversation;
pub(crate) use work::NativePreparedClaim;

pub(super) const COLLECTION: &str = "native_prepared_sandboxes";

pub(super) async fn ensure_indexes(
    collection: &Collection<Document>,
) -> Result<(), layer2_core::StoreError> {
    collection
        .create_indexes([IndexModel::builder()
            .keys(doc! {"kind":1,"state":1,"check_after":1,"lease_until":1})
            .build()])
        .await
        .map(|_| ())
        .map_err(|e| layer2_core::StoreError::Unavailable(e.to_string()))
}

fn document(record: &Record) -> Result<Document, SandboxStoreError> {
    record.validate()?;
    Ok(doc! {
        "_id": record.payload.id.as_str(), "kind":"reservation",
        "document_schema_version":DOCUMENT_SCHEMA_VERSION,
        "state":record_bson(&record.payload.state).map_err(SandboxStoreError::Corrupt)?,
        "revision":signed(record.payload.revision,"native prepared revision").map_err(SandboxStoreError::Corrupt)?,
        "record":record_bson(record).map_err(SandboxStoreError::Corrupt)?,
    })
}

fn read(document: &Document) -> Result<Record, SandboxStoreError> {
    let record: Record = read_record(document).map_err(SandboxStoreError::Corrupt)?;
    let expected = self::document(&record)?;
    if expected
        .iter()
        .any(|(key, value)| document.get(key) != Some(value))
    {
        return Err(model::corrupt("query fields disagree with record"));
    }
    Ok(record)
}

fn unavailable(error: mongodb::error::Error) -> SandboxStoreError {
    SandboxStoreError::Unavailable(error.to_string())
}

impl MongoSandboxStore {
    async fn native_prepared_record(
        &self,
        id: &SandboxId,
    ) -> Result<Option<Record>, SandboxStoreError> {
        self.native_prepared
            .find_one(doc! {"_id":id.as_str(),"kind":"reservation"})
            .await
            .map_err(unavailable)?
            .as_ref()
            .map(read)
            .transpose()
    }

    /// Lifecycle uses this read to defer Agent admission until parent readiness.
    pub(crate) async fn consumed_native_prepared_for_sandbox(
        &self,
        id: &SandboxId,
    ) -> Result<Option<WarmSandbox>, SandboxStoreError> {
        Ok(self
            .native_prepared_record(id)
            .await?
            .filter(|r| r.ownership.is_some())
            .map(|r| r.payload))
    }
}

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
mod agent_open_ready;
mod agent_replay;
pub(in crate::store) use agent_open_ready::open_ready_proof_revision;
mod authority;
mod model;
mod planning;
#[cfg(test)]
pub(crate) mod tests;
#[cfg(test)]
pub(crate) use tests::machine_tests::admit_machine_for_pointer_fixture;
mod tools;
mod work;
mod workspace_proof;
pub(super) use admission::{claim_ready, ClaimOutcome};
pub(super) use adoption::{
    adopt_agent, adopt_workspace, completed_installation_digest, completed_workspace,
};
pub(super) use agent_replay::replay_proof_revision;
pub(crate) use model::NativePreparedPlan;
use model::Record;
#[cfg(test)]
pub(super) use tools::prepared_conversation;
pub(super) use tools::prepared_conversation_transaction;
pub(crate) use work::NativePreparedClaim;
pub(crate) use workspace_proof::NativePreparedWorkspaceProof;

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
    let mut value = doc! {
        "_id": record.payload.id.as_str(), "kind":"reservation",
        "document_schema_version":DOCUMENT_SCHEMA_VERSION,
        "state":record_bson(&record.payload.state).map_err(SandboxStoreError::Corrupt)?,
        "revision":signed(record.payload.revision,"native prepared revision").map_err(SandboxStoreError::Corrupt)?,
        "record":record_bson(record).map_err(SandboxStoreError::Corrupt)?,
    };
    if record.plan.machine() {
        value.insert("machine", true);
    }
    Ok(value)
}

fn read(document: &Document) -> Result<Record, SandboxStoreError> {
    let record: Record = read_record(document).map_err(SandboxStoreError::Corrupt)?;
    let channel = match document.get("machine") {
        None | Some(mongodb::bson::Bson::Boolean(false)) => false,
        Some(mongodb::bson::Bson::Boolean(true)) => true,
        _ => return Err(model::corrupt("invalid preparation channel")),
    };
    if channel != record.plan.machine() {
        return Err(model::corrupt(
            "preparation channel disagrees with immutable plan",
        ));
    }
    let expected = self::document(&record)?;
    if expected
        .iter()
        .any(|(key, value)| document.get(key) != Some(value))
    {
        return Err(model::corrupt("query fields disagree with record"));
    }
    workspace_proof::read(document, &record.payload)?;
    Ok(record)
}

/// Consumed reservations retain historical payloads; ownership now lives in the
/// Sandbox aggregate. Retirement only considers still-unowned preparation work.
pub(super) fn unowned_for_retirement(
    document: &Document,
) -> Result<Option<WarmSandbox>, SandboxStoreError> {
    let record = read(document)?;
    Ok(record.ownership.is_none().then_some(record.payload))
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

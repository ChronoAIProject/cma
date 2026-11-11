//! Default-unwired B2 storage on the same purge-conflicting parent marker.
use super::{parent_filter, unavailable, MARKER};
use crate::store::{
    sandbox_mongo::{record_bson, signed, MongoSandboxStore},
    sandboxes::read_sandbox,
};
use layer2_core::{
    sandbox::store::{
        ledger_collection::checkpoint::{
            LedgerCollectionCheckpointMutation, LedgerCollectionCheckpointStore,
            LedgerCollectionCheckpointV2,
        },
        SandboxStoreError,
    },
    SandboxId,
};
use mongodb::bson::{doc, Document};

fn read(document: &Document) -> Result<Option<LedgerCollectionCheckpointV2>, SandboxStoreError> {
    let Some(raw) = document.get(MARKER) else {
        return Ok(None);
    };
    let value: LedgerCollectionCheckpointV2 = mongodb::bson::deserialize_from_bson(raw.clone())
        .map_err(|_| {
            SandboxStoreError::Corrupt("ledger collection checkpoint is malformed".into())
        })?;
    if !value.validate() {
        return Err(SandboxStoreError::Corrupt(
            "ledger collection checkpoint is invalid".into(),
        ));
    }
    Ok(Some(value))
}

#[async_trait::async_trait]
impl LedgerCollectionCheckpointStore for MongoSandboxStore {
    async fn ledger_collection_checkpoint(
        &self,
        sandbox: &SandboxId,
    ) -> Result<Option<LedgerCollectionCheckpointV2>, SandboxStoreError> {
        let document = self
            .sandboxes
            .find_one(doc! {"_id":sandbox.as_str()})
            .await
            .map_err(unavailable)?
            .ok_or(SandboxStoreError::NotFound)?;
        read_sandbox(&document)?;
        read(&document)
    }

    async fn retain_ledger_collection_checkpoint(
        &self,
        sandbox: &SandboxId,
        expected_sandbox_revision: u64,
        initial: LedgerCollectionCheckpointV2,
    ) -> Result<(), SandboxStoreError> {
        if !initial.is_initial() {
            return Err(SandboxStoreError::Conflict);
        }
        let mut filter = parent_filter(sandbox, expected_sandbox_revision)?;
        let document = self
            .sandboxes
            .find_one(filter.clone())
            .await
            .map_err(unavailable)?
            .ok_or(SandboxStoreError::Conflict)?;
        read_sandbox(&document)?;
        if let Some(current) = read(&document)? {
            return if current == initial {
                Ok(())
            } else {
                Err(SandboxStoreError::Conflict)
            };
        }
        filter.insert(MARKER, doc! {"$exists":false});
        let mut fields = Document::new();
        fields.insert(
            MARKER,
            record_bson(&initial).map_err(SandboxStoreError::Corrupt)?,
        );
        let outcome = self
            .sandboxes
            .update_one(filter, doc! {"$set":fields})
            .await
            .map_err(unavailable)?;
        if outcome.matched_count != 1 {
            return Err(SandboxStoreError::Conflict);
        }
        Ok(())
    }

    async fn update_ledger_collection_checkpoint(
        &self,
        sandbox: &SandboxId,
        expected_sandbox_revision: u64,
        expected_progress_revision: u64,
        mutation: LedgerCollectionCheckpointMutation,
    ) -> Result<LedgerCollectionCheckpointV2, SandboxStoreError> {
        let mut filter = parent_filter(sandbox, expected_sandbox_revision)?;
        filter.insert(
            format!("{MARKER}.progress.revision"),
            signed(expected_progress_revision, "collector checkpoint revision")
                .map_err(SandboxStoreError::Corrupt)?,
        );
        let document = self
            .sandboxes
            .find_one(filter.clone())
            .await
            .map_err(unavailable)?
            .ok_or(SandboxStoreError::Conflict)?;
        read_sandbox(&document)?;
        let current = read(&document)?.ok_or(SandboxStoreError::Conflict)?;
        let next = current
            .apply(mutation)
            .map_err(|_| SandboxStoreError::Conflict)?;
        if next == current {
            return Ok(current);
        }
        // Compare the whole original marker as well as its revision. This
        // refuses schema/authority replacement without relying on its revision.
        filter.insert(MARKER, document.get(MARKER).expect("read marker").clone());
        let mut fields = Document::new();
        fields.insert(
            MARKER,
            record_bson(&next).map_err(SandboxStoreError::Corrupt)?,
        );
        let outcome = self
            .sandboxes
            .update_one(filter, doc! {"$set":fields})
            .await
            .map_err(unavailable)?;
        if outcome.matched_count != 1 {
            return Err(SandboxStoreError::Conflict);
        }
        Ok(next)
    }
}

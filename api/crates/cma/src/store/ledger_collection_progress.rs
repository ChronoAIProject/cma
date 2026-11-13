//! One bounded progress/retention marker per parent, updated with the parent CAS.
use layer2_core::{
    sandbox::store::{
        ledger_collection::{
            LedgerCollectionStore, LedgerCollectorMutation, LedgerCollectorProgressV1,
        },
        SandboxStoreError,
    },
    SandboxId,
};
use mongodb::bson::{doc, Document};

use super::{
    sandbox_mongo::{record_bson, signed, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION},
    sandboxes::read_sandbox,
};

pub(super) const MARKER: &str = "ledger_collection_v1";

/// A completed collection group is not acknowledgement of the final home
/// transition. C/A must supply that later release integration. Unknown or
/// malformed markers retain authority; missing data never invents a tombstone.
pub(super) fn retention_required(document: &Document) -> Result<bool, SandboxStoreError> {
    if document.contains_key(MARKER) {
        return Ok(true);
    }
    let sandbox = read_sandbox(document)?;
    Ok(sandbox
        .lifecycle
        .home
        .as_ref()
        .is_some_and(|home| !home.released))
}

fn read(document: &Document) -> Result<Option<LedgerCollectorProgressV1>, SandboxStoreError> {
    let Some(raw) = document.get(MARKER) else {
        return Ok(None);
    };
    let value: LedgerCollectorProgressV1 = mongodb::bson::deserialize_from_bson(raw.clone())
        .map_err(|_| {
            SandboxStoreError::Corrupt("ledger collection progress is malformed".into())
        })?;
    if !value.validate() {
        return Err(SandboxStoreError::Corrupt(
            "ledger collection progress is invalid".into(),
        ));
    }
    Ok(Some(value))
}

#[async_trait::async_trait]
impl LedgerCollectionStore for MongoSandboxStore {
    async fn ledger_collection_progress(
        &self,
        sandbox: &SandboxId,
    ) -> Result<Option<LedgerCollectorProgressV1>, SandboxStoreError> {
        let document = self
            .sandboxes
            .find_one(doc! {"_id":sandbox.as_str()})
            .await
            .map_err(unavailable)?
            .ok_or(SandboxStoreError::NotFound)?;
        read_sandbox(&document)?;
        read(&document)
    }

    async fn retain_ledger_collection(
        &self,
        sandbox: &SandboxId,
        expected_sandbox_revision: u64,
        initial: LedgerCollectorProgressV1,
    ) -> Result<(), SandboxStoreError> {
        if !initial.validate()
            || initial.revision != 1
            || initial.lease.fence != 1
            || initial.pending.is_some()
            || initial.page.is_some()
            || initial.cursor.is_some()
            || initial.last_ack_sha256.is_some()
            || initial.complete
            || initial.sweep != 1
        {
            return Err(SandboxStoreError::Conflict);
        }
        let filter = parent_filter(sandbox, expected_sandbox_revision)?;
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
        let mut filter = filter;
        filter.insert(MARKER, doc! {"$exists":false});
        let mut fields = Document::new();
        fields.insert(
            MARKER,
            record_bson(&initial).map_err(SandboxStoreError::Corrupt)?,
        );
        // This write conflicts with the purge transaction's final parent delete.
        // Do not write retention in a disjoint collection (snapshot write skew).
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

    async fn update_ledger_collection(
        &self,
        sandbox: &SandboxId,
        expected_sandbox_revision: u64,
        expected_progress_revision: u64,
        mutation: LedgerCollectorMutation,
    ) -> Result<LedgerCollectorProgressV1, SandboxStoreError> {
        let mut filter = parent_filter(sandbox, expected_sandbox_revision)?;
        filter.insert(
            format!("{MARKER}.revision"),
            signed(expected_progress_revision, "collector revision")
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

fn parent_filter(sandbox: &SandboxId, revision: u64) -> Result<Document, SandboxStoreError> {
    Ok(
        doc! {"_id":sandbox.as_str(),"document_schema_version":DOCUMENT_SCHEMA_VERSION,
        "revision":signed(revision,"collector parent revision").map_err(SandboxStoreError::Corrupt)?},
    )
}

fn unavailable(_: mongodb::error::Error) -> SandboxStoreError {
    SandboxStoreError::Unavailable("ledger collection progress is unavailable".into())
}

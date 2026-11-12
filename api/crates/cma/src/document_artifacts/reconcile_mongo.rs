//! Bounded projections only; private document text is never loaded or reported.
use super::*;
use crate::document_artifacts::quota::{number, ready, validate_row, IO, QUOTA_COLLECTION, STATE};
use futures_util::TryStreamExt;
use mongodb::{
    bson::{doc, Document},
    Collection, Database,
};

pub(super) struct MongoReconcileStore {
    ledger: Collection<Document>,
    documents: Collection<Document>,
}
impl MongoReconcileStore {
    pub(super) fn new(database: &Database) -> Self {
        let options = mongodb::options::CollectionOptions::builder()
            .read_concern(mongodb::options::ReadConcern::majority())
            .build();
        Self {
            ledger: database.collection_with_options(QUOTA_COLLECTION, options.clone()),
            documents: database
                .collection_with_options(crate::document_artifacts::COLLECTION, options),
        }
    }
}
#[async_trait::async_trait]
impl ReconcileStore for MongoReconcileStore {
    async fn ledger_page(&self, after: Option<&str>) -> Result<Vec<LedgerEntry>, DocumentError> {
        let mut filter = doc! {"_id": {"$ne": STATE}};
        if let Some(after) = after {
            filter.insert("_id", doc! {"$gt": after, "$ne": STATE});
        }
        let rows: Vec<Document> = self
            .ledger
            .find(filter)
            .projection(doc! {"_id": 1, "document_schema_version": 1, "kind": 1,
            "documents": 1, "bytes": 1, "objects": 1, "state": 1,
            "reserved_at": 1, "condemned_at": 1,
            "baseline_documents": 1, "baseline_bytes": 1, "baseline_objects": 1})
            .sort(doc! {"_id": 1})
            .limit(PAGE as i64)
            .batch_size(PAGE as u32)
            .max_time(IO)
            .await
            .map_err(unavailable)?
            .try_collect()
            .await
            .map_err(unavailable)?;
        rows.iter().map(ledger_entry).collect()
    }
    async fn reference_page(
        &self,
        after: Option<&str>,
    ) -> Result<Option<ReferenceEntry>, DocumentError> {
        let filter = after.map_or_else(|| doc! {}, |id| doc! {"_id": {"$gt": id}});
        let row = self
            .documents
            .find_one(filter)
            .sort(doc! {"_id": 1})
            .projection(
                doc! {"_id": 1, "document_schema_version": 1, "owner_subject": 1,
                "record.metadata.id": 1, "record.metadata.owner_subject": 1,
                "record.revisions.object_key": 1},
            )
            .max_time(IO)
            .await
            .map_err(unavailable)?;
        row.as_ref().map(reference_entry).transpose()
    }
    async fn object_state(&self, key: &str) -> Result<Option<String>, DocumentError> {
        let source = parse_key(key)?;
        let id = format!(
            "object:{}:{}:{}",
            source.owner_hash, source.document_id, source.sha256
        );
        let row = self
            .ledger
            .find_one(doc! {"_id": id})
            .projection(doc! {"document_schema_version": 1, "kind": 1, "state": 1})
            .max_time(IO)
            .await
            .map_err(unavailable)?;
        row.map(|row| {
            validate_row(&row, "object")?;
            let state = match row.get("state") {
                None => "reserved",
                Some(value) => value.as_str().ok_or(DocumentError::Corrupt)?,
            };
            Ok(state.to_owned())
        })
        .transpose()
    }
    async fn referenced(&self, key: &str) -> Result<bool, DocumentError> {
        parse_key(key)?;
        Ok(self
            .documents
            .find_one(doc! {"record.revisions.object_key": key})
            .projection(doc! {"_id": 1})
            .max_time(IO)
            .await
            .map_err(unavailable)?
            .is_some())
    }
    async fn ready(&self) -> Result<bool, DocumentError> {
        Ok(self
            .ledger
            .find_one(doc! {"_id": STATE})
            .max_time(IO)
            .await
            .map_err(unavailable)?
            .as_ref()
            .is_some_and(ready))
    }
}
fn ledger_entry(row: &Document) -> Result<LedgerEntry, DocumentError> {
    let id = row.get_str("_id").map_err(|_| DocumentError::Corrupt)?;
    let kind = row.get_str("kind").map_err(|_| DocumentError::Corrupt)?;
    validate_row(row, kind)?;
    let parts: Vec<_> = id.split(':').collect();
    let owner = *parts.get(1).ok_or(DocumentError::Corrupt)?;
    if !valid_digest(owner) {
        return Err(DocumentError::Corrupt);
    }
    let mut entry = LedgerEntry {
        id: id.into(),
        owner: owner.into(),
        kind: kind.into(),
        used: None,
        baseline: None,
        object_key: None,
        bytes: 0,
        state: None,
        since_ms: None,
    };
    match (kind, parts.as_slice()) {
        ("owner", ["owner", _]) => {
            entry.used = Some([
                number(row, "documents")?,
                number(row, "bytes")?,
                number(row, "objects")?,
            ]);
            let keys = ["baseline_documents", "baseline_bytes", "baseline_objects"];
            if keys.iter().any(|key| row.contains_key(key)) {
                entry.baseline = Some([
                    number(row, keys[0])?,
                    number(row, keys[1])?,
                    number(row, keys[2])?,
                ]);
            }
        }
        ("document", ["document", _, id]) => {
            let source = DocumentSourceReservation {
                owner_hash: owner.into(),
                document_id: (*id).into(),
                sha256: "0".repeat(64),
                bytes: 1,
            };
            source.validate().map_err(|_| DocumentError::Corrupt)?;
        }
        ("object", ["object", _, id, sha]) => {
            let source = DocumentSourceReservation {
                owner_hash: owner.into(),
                document_id: (*id).into(),
                sha256: (*sha).into(),
                bytes: number(row, "bytes")?,
            };
            source.validate().map_err(|_| DocumentError::Corrupt)?;
            entry.object_key = Some(source.object_key());
            entry.bytes = source.bytes;
            entry.state = row
                .get("state")
                .map(|value| {
                    value
                        .as_str()
                        .map(str::to_owned)
                        .ok_or(DocumentError::Corrupt)
                })
                .transpose()?;
            let timestamp = if entry.state.as_deref() == Some("condemned") {
                "condemned_at"
            } else {
                "reserved_at"
            };
            entry.since_ms = row
                .get(timestamp)
                .map(|value| {
                    value
                        .as_datetime()
                        .map(|value| value.timestamp_millis())
                        .ok_or(DocumentError::Corrupt)
                })
                .transpose()?;
        }
        _ => return Err(DocumentError::Corrupt),
    }
    Ok(entry)
}
fn reference_entry(row: &Document) -> Result<ReferenceEntry, DocumentError> {
    let corrupt = |_| DocumentError::Corrupt;
    if row.get_i64("document_schema_version").ok() != Some(1) {
        return Err(DocumentError::Corrupt);
    }
    let id = row.get_str("_id").map_err(corrupt)?;
    let subject = row.get_str("owner_subject").map_err(corrupt)?;
    let record = row.get_document("record").map_err(corrupt)?;
    let metadata = record.get_document("metadata").map_err(corrupt)?;
    if metadata.get_str("id").ok() != Some(id)
        || metadata.get_str("owner_subject").ok() != Some(subject)
    {
        return Err(DocumentError::Corrupt);
    }
    let owner = digest(subject.as_bytes());
    let revisions = record.get_array("revisions").map_err(corrupt)?;
    if revisions.is_empty() || revisions.len() > MAX_REVISIONS {
        return Err(DocumentError::Corrupt);
    }
    let mut keys = Vec::with_capacity(revisions.len());
    for revision in revisions {
        let key = revision
            .as_document()
            .ok_or(DocumentError::Corrupt)?
            .get_str("object_key")
            .map_err(corrupt)?;
        let source = parse_key(key).map_err(|_| DocumentError::Corrupt)?;
        if source.owner_hash != owner || source.document_id != id {
            return Err(DocumentError::Corrupt);
        }
        keys.push(key.into());
    }
    Ok(ReferenceEntry {
        id: id.into(),
        owner,
        keys,
    })
}
fn unavailable(_: mongodb::error::Error) -> DocumentError {
    DocumentError::Unavailable
}

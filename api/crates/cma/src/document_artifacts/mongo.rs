use futures_util::TryStreamExt;
use layer2_core::document_artifacts::*;
use mongodb::{
    bson::{doc, Document},
    Collection, Database, IndexModel,
};

pub(crate) const COLLECTION: &str = "cma_document_artifacts";
#[derive(Clone)]
pub struct MongoDocumentStore {
    pub(super) rows: Collection<Document>,
    pub(super) deletion_enabled: bool,
    pub(super) deletion_supported: bool,
}
impl MongoDocumentStore {
    pub(crate) fn reclaim(&self, enabled: bool) -> super::MongoDocumentReclaimStore {
        super::MongoDocumentReclaimStore::new(
            &self
                .rows
                .client()
                .database(self.rows.namespace().db.as_str()),
            enabled,
        )
    }

    pub(crate) fn quota(&self, enabled: bool) -> super::MongoDocumentQuotaStore {
        super::MongoDocumentQuotaStore::new(
            &self
                .rows
                .client()
                .database(self.rows.namespace().db.as_str()),
            enabled,
        )
    }

    pub(super) fn quota_rows(&self) -> Collection<Document> {
        self.rows
            .client()
            .database(self.rows.namespace().db.as_str())
            .collection(super::QUOTA_COLLECTION)
    }
    pub fn new(database: Database) -> Self {
        Self {
            deletion_enabled: false,
            deletion_supported: false,
            rows: database.collection_with_options(
                COLLECTION,
                mongodb::options::CollectionOptions::builder()
                    .read_concern(mongodb::options::ReadConcern::majority())
                    .write_concern(mongodb::options::WriteConcern::majority())
                    .build(),
            ),
        }
    }
    pub async fn ensure_indexes(&self) -> Result<(), DocumentError> {
        for keys in [doc! {"state":1,"purge.next_at":1}, doc! {"owner_hash":1}] {
            self.lifecycle_rows()
                .create_index(IndexModel::builder().keys(keys).build())
                .await
                .map_err(|_| DocumentError::Unavailable)?;
        }
        for keys in [
            doc! {"owner_subject": 1, "_id": 1},
            doc! {"recipient_subjects": 1, "_id": 1},
        ] {
            self.rows
                .create_index(IndexModel::builder().keys(keys).build())
                .await
                .map_err(|_| DocumentError::Unavailable)?;
        }
        self.quota_rows()
            .create_index(
                IndexModel::builder()
                    .keys(doc! {"kind": 1, "state": 1, "reserved_at": 1})
                    .build(),
            )
            .await
            .map_err(|_| DocumentError::Unavailable)?;
        Ok(())
    }
}
#[async_trait::async_trait]
impl DocumentStore for MongoDocumentStore {
    async fn get(&self, id: &str) -> Result<Option<StoredDocument>, DocumentError> {
        let value = self
            .rows
            .find_one(doc! {"_id": id})
            .await
            .map_err(|_| DocumentError::Unavailable)?;
        value.as_ref().map(decode).transpose()
    }
    async fn insert(
        &self,
        record: &StoredDocument,
        source: Option<(&DocumentSourceReservation, SourceTicket)>,
    ) -> Result<InsertOutcome, DocumentError> {
        if self.deletion_enabled {
            let ledgered = source
                .and_then(|(source, ticket)| (ticket == SourceTicket::Ledgered).then_some(source));
            return super::lifecycle_create::insert(self, record, ledgered).await;
        }
        // Rollout must drain old creates before enabling deletion admission.
        // This compatibility point read deliberately preserves the plain write path.
        if self.read_tombstone(&record.metadata.id).await?.is_some() {
            return Ok(InsertOutcome::Deleted);
        }
        if let Some((source, SourceTicket::Ledgered)) = source {
            return super::commit::insert(self, record, source)
                .await
                .map(|inserted| {
                    if inserted {
                        InsertOutcome::Inserted
                    } else {
                        InsertOutcome::Exists
                    }
                });
        }
        match self.rows.insert_one(encode(record)?).await {
            Ok(_) => Ok(InsertOutcome::Inserted),
            Err(error) if crate::store::is_duplicate_key(&error) => Ok(InsertOutcome::Exists),
            Err(_) => Err(DocumentError::Unavailable),
        }
    }
    async fn delete(
        &self,
        expected: u64,
        tombstone: &DocumentTombstone,
    ) -> Result<DeleteOutcome, DocumentError> {
        self.delete_document(expected, tombstone).await
    }
    async fn tombstone(&self, id: &str) -> Result<Option<DocumentTombstone>, DocumentError> {
        self.read_tombstone(id).await
    }
    async fn purge_state(&self, id: &str) -> Result<Option<DocumentPurgeState>, DocumentError> {
        self.deletion_state(id).await
    }
    async fn pending_deletions(&self, owner_hash: &str) -> Result<u64, DocumentError> {
        if !valid_digest(owner_hash) {
            return Err(DocumentError::Invalid);
        }
        self.lifecycle_rows()
            .count_documents(doc! {"owner_hash":owner_hash,"state":"deleting"})
            .max_time(super::quota::IO)
            .await
            .map_err(super::quota::unavailable)
    }
    async fn compare_exchange(
        &self,
        expected: u64,
        record: &StoredDocument,
        source: Option<(&DocumentSourceReservation, SourceTicket)>,
    ) -> Result<bool, DocumentError> {
        let expected = i64::try_from(expected).map_err(|_| DocumentError::Corrupt)?;
        let expected_revision = expected;
        if record.metadata.revision != expected as u64 + 1 {
            return Err(DocumentError::Corrupt);
        }
        if let Some((source, SourceTicket::Ledgered)) = source {
            return super::commit::compare_exchange(self, expected_revision as u64, record, source)
                .await;
        }
        let result = self.rows.replace_one(doc! {"_id": &record.metadata.id, "owner_subject": &record.metadata.owner_subject, "revision": expected}, encode(record)?)
            .await.map_err(|_| DocumentError::Unavailable)?;
        Ok(result.matched_count == 1)
    }
    async fn list(
        &self,
        subject: &str,
        query: &DocumentQuery,
    ) -> Result<Vec<DocumentListing>, DocumentError> {
        query.validate()?;
        let mut filter = match query.scope.as_deref().unwrap_or("all") {
            "owned" => doc! {"owner_subject": subject},
            "shared" => doc! {"recipient_subjects": subject, "owner_subject": {"$ne": subject}},
            _ => doc! {"$or": [{"owner_subject": subject}, {"recipient_subjects": subject}]},
        };
        if let Some(cursor) = &query.cursor {
            filter.insert("_id", doc! {"$gt": cursor});
        }
        if let Some(search) = query.q.as_ref().filter(|s| !s.is_empty()) {
            // Literal substring only; user input never becomes a regular expression.
            filter.insert(
                "$expr",
                doc! {"$gte": [{"$indexOfCP": ["$title_search", search.to_lowercase()]}, 0]},
            );
        }
        let values: Vec<Document> = self.rows.find(filter).projection(doc! {"record.metadata": 1, "record.shares": 1, "document_schema_version": 1, "owner_subject": 1, "revision": 1}).sort(doc! {"_id": 1})
            .limit((query.limit() + 1) as i64).max_time(std::time::Duration::from_secs(3))
            .await.map_err(|_| DocumentError::Unavailable)?.try_collect().await.map_err(|_| DocumentError::Unavailable)?;
        values
            .iter()
            .map(|value| {
                let record = value
                    .get_document("record")
                    .map_err(|_| DocumentError::Corrupt)?;
                let listing: DocumentListing =
                    mongodb::bson::deserialize_from_document(record.clone())
                        .map_err(|_| DocumentError::Corrupt)?;
                if value.get_i64("document_schema_version").ok() != Some(1)
                    || value.get_str("_id").ok() != Some(listing.metadata.id.as_str())
                    || value.get_str("owner_subject").ok()
                        != Some(listing.metadata.owner_subject.as_str())
                    || value.get_i64("revision").ok()
                        != i64::try_from(listing.metadata.revision).ok()
                {
                    return Err(DocumentError::Corrupt);
                }
                Ok(listing)
            })
            .collect()
    }
}
pub(super) fn encode(record: &StoredDocument) -> Result<Document, DocumentError> {
    record.validate()?;
    let data = mongodb::bson::serialize_to_bson(record).map_err(|_| DocumentError::Corrupt)?;
    Ok(
        doc! {"_id": &record.metadata.id, "document_schema_version": 1_i64,
        "owner_subject": &record.metadata.owner_subject, "revision": i64::try_from(record.metadata.revision).map_err(|_| DocumentError::Corrupt)?,
        "title_search": &record.title_search, "recipient_subjects": record.shares.iter().map(|s| s.subject.clone()).collect::<Vec<_>>(),
        "record": data},
    )
}
pub(super) fn decode(value: &Document) -> Result<StoredDocument, DocumentError> {
    let data = value.get("record").ok_or(DocumentError::Corrupt)?.clone();
    let record: StoredDocument =
        mongodb::bson::deserialize_from_bson(data).map_err(|_| DocumentError::Corrupt)?;
    let expected = encode(&record)?;
    if expected.iter().any(|(key, v)| value.get(key) != Some(v)) {
        return Err(DocumentError::Corrupt);
    }
    Ok(record)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn metadata_roundtrip_rejects_owner_and_index_corruption() {
        use layer2_core::{Authorization, Caller, Capability, SubjectId};
        use std::sync::Arc;
        let rows = Arc::new(memory::MemoryDocuments::default());
        let service = DocumentService::new(
            rows.clone(),
            Arc::new(memory::MemoryDocumentBlobs::default()),
        );
        let caller = Caller::new(
            SubjectId::new("owner"),
            Authorization::new([Capability::UseOwnSandboxes]),
        );
        let document = service
            .create(
                &caller,
                "create",
                CreateDocument {
                    title: "Title".into(),
                    markdown: "# Source".into(),
                    origin_agent_id: None,
                },
            )
            .await
            .unwrap();
        let record = rows.get(&document.id).await.unwrap().unwrap();
        let mut encoded = encode(&record).unwrap();
        assert_eq!(decode(&encoded).unwrap().metadata, document);
        encoded.insert("owner_subject", "another-owner");
        assert!(matches!(decode(&encoded), Err(DocumentError::Corrupt)));
    }
}

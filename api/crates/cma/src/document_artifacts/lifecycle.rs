//! Permanent identity fences and atomic logical document deletion.
use super::{
    mongo::{decode, MongoDocumentStore},
    quota::{number, owner_id, unavailable, IO},
    transaction::{self, Error, Operation},
};
use futures_util::TryStreamExt;
use layer2_core::document_artifacts::*;
use mongodb::{
    bson::{doc, DateTime, Document},
    options::ReadConcern,
    ClientSession, Collection,
};

pub(crate) const LIFECYCLE_COLLECTION: &str = "cma_document_lifecycle";
impl MongoDocumentStore {
    pub(super) fn lifecycle_rows(&self) -> Collection<Document> {
        self.rows
            .client()
            .database(&self.rows.namespace().db)
            .collection_with_options(
                LIFECYCLE_COLLECTION,
                mongodb::options::CollectionOptions::builder()
                    .read_concern(ReadConcern::majority())
                    .write_concern(mongodb::options::WriteConcern::majority())
                    .build(),
            )
    }
    pub(crate) fn deletion(mut self, enabled: bool, supported: bool) -> Self {
        self.deletion_enabled = enabled;
        self.deletion_supported = supported;
        self
    }
    pub(super) async fn read_tombstone(
        &self,
        id: &str,
    ) -> Result<Option<DocumentTombstone>, DocumentError> {
        let row = self
            .lifecycle_rows()
            .find_one(doc! {"_id":id})
            .max_time(IO)
            .await
            .map_err(unavailable)?;
        row.as_ref().map(tombstone).transpose().map(Option::flatten)
    }
    pub(super) async fn deletion_state(
        &self,
        id: &str,
    ) -> Result<Option<DocumentPurgeState>, DocumentError> {
        let row = self
            .lifecycle_rows()
            .find_one(doc! {"_id":id})
            .max_time(IO)
            .await
            .map_err(unavailable)?;
        row.as_ref().map(state).transpose().map(Option::flatten)
    }
    pub(super) async fn delete_document(
        &self,
        expected: u64,
        tombstone: &DocumentTombstone,
    ) -> Result<DeleteOutcome, DocumentError> {
        tombstone.validate()?;
        if tombstone.revision != expected.checked_add(1).ok_or(DocumentError::Corrupt)? {
            return Err(DocumentError::Corrupt);
        }
        // BSON integers are signed; reject an unrepresentable tombstone before
        // starting a transaction or removing the live aggregate.
        let expected = i64::try_from(expected).map_err(|_| DocumentError::Corrupt)?;
        let revision = i64::try_from(tombstone.revision).map_err(|_| DocumentError::Corrupt)?;
        transaction::run(
            self.rows.client(),
            Delete {
                store: self,
                expected,
                revision,
                tombstone,
            },
        )
        .await
    }
}
pub(super) fn state(row: &Document) -> Result<Option<DocumentPurgeState>, DocumentError> {
    if number(row, "document_schema_version")? != 1 {
        return Err(DocumentError::Corrupt);
    }
    match row.get_str("state").ok() {
        Some("live") => Ok(None),
        Some("purged") => Ok(Some(DocumentPurgeState::Complete)),
        Some("deleting") => Ok(Some(
            if row
                .get_document("purge")
                .map_err(|_| DocumentError::Corrupt)?
                .get_str("blocked")
                .is_ok()
            {
                DocumentPurgeState::Blocked
            } else {
                DocumentPurgeState::Pending
            },
        )),
        _ => Err(DocumentError::Corrupt),
    }
}
pub(super) fn tombstone(row: &Document) -> Result<Option<DocumentTombstone>, DocumentError> {
    if state(row)?.is_none() {
        return Ok(None);
    }
    let deletion = row
        .get_document("deletion")
        .map_err(|_| DocumentError::Corrupt)?;
    let value = DocumentTombstone {
        id: row
            .get_str("_id")
            .map_err(|_| DocumentError::Corrupt)?
            .into(),
        owner_hash: row
            .get_str("owner_hash")
            .map_err(|_| DocumentError::Corrupt)?
            .into(),
        revision: number(deletion, "revision")?,
        deleted_at: deletion
            .get_str("deleted_at")
            .map_err(|_| DocumentError::Corrupt)?
            .into(),
        receipt_key: deletion
            .get_str("receipt_key")
            .map_err(|_| DocumentError::Corrupt)?
            .into(),
        fingerprint: deletion
            .get_str("fingerprint")
            .map_err(|_| DocumentError::Corrupt)?
            .into(),
        objects: row
            .get_array("objects")
            .map_err(|_| DocumentError::Corrupt)?
            .iter()
            .map(|v| v.as_str().map(str::to_owned).ok_or(DocumentError::Corrupt))
            .collect::<Result<_, _>>()?,
    };
    value.validate()?;
    Ok(Some(value))
}
struct Delete<'a> {
    store: &'a MongoDocumentStore,
    expected: i64,
    revision: i64,
    tombstone: &'a DocumentTombstone,
}
#[async_trait::async_trait]
impl Operation for Delete<'_> {
    type Output = DeleteOutcome;
    async fn apply(&self, session: &mut ClientSession) -> Result<Self::Output, Error> {
        let t = self.tombstone;
        let lifecycle = self.store.lifecycle_rows();
        let prior = lifecycle
            .find_one(doc! {"_id":&t.id})
            .session(&mut *session)
            .await?;
        if let Some(prior) = prior.as_ref() {
            if let Some(tombstone) = tombstone(prior)? {
                return Ok(DeleteOutcome::AlreadyDeleted(tombstone));
            }
        }
        let Some(raw) = self
            .store
            .rows
            .find_one(doc! {"_id":&t.id})
            .session(&mut *session)
            .await?
        else {
            return Ok(DeleteOutcome::NotFound);
        };
        let record = decode(&raw)?;
        if digest(record.metadata.owner_subject.as_bytes()) != t.owner_hash {
            return Ok(DeleteOutcome::NotFound);
        }
        if i64::try_from(record.metadata.revision).map_err(|_| DocumentError::Corrupt)?
            != self.expected
        {
            return Ok(DeleteOutcome::Conflict);
        }
        let objects: Vec<_> = record
            .revisions
            .iter()
            .map(|r| r.object_key.clone())
            .collect();
        if objects != t.objects {
            return Err(DocumentError::Corrupt.into());
        }
        let removed = self.store.rows.delete_one(doc! {"_id":&t.id,"owner_subject":&record.metadata.owner_subject,"revision":self.expected}).session(&mut *session).await?;
        if removed.deleted_count != 1 {
            return Ok(DeleteOutcome::Conflict);
        }
        let deletion = doc! {"deleted_at":&t.deleted_at,"revision":self.revision,"receipt_key":&t.receipt_key,"fingerprint":&t.fingerprint};
        let created = prior
            .as_ref()
            .and_then(|r| r.get_datetime("created_at").ok())
            .copied()
            .unwrap_or_else(DateTime::now);
        let mut purge = doc! {"pass":1_i64,"next_at":"$$NOW","attempts":0_i64};
        if !self.store.deletion_supported {
            purge.insert("blocked", "storage_deletion_unsupported");
        }
        lifecycle.update_one(doc! {"_id":&t.id}, vec![doc! {"$set":{"document_schema_version":1_i64,"owner_hash":&t.owner_hash,"state":"deleting","created_at":created,"deletion":{"$literal":deletion},"objects":{"$literal":&t.objects},"purge":purge}}]).upsert(true).session(&mut *session).await?;
        let rows = self.store.quota_rows();
        let owner = owner_id(&t.owner_hash);
        let document = format!("document:{}:{}", t.owner_hash, t.id);
        // Missing quota rows stay missing; logical deletion is independent of activation.
        rows.update_one(doc! {"_id":&owner}, doc! {"$inc":{"fence":1_i64}})
            .session(&mut *session)
            .await?;
        rows.update_one(doc! {"_id":document}, doc! {"$set":{"state":"deleting"}})
            .session(&mut *session)
            .await?;
        let mut cursor = rows
            .aggregate([
                doc! {"$match":object_range(&t.owner_hash,&t.id)},
                doc! {"$group":{"_id":null,"latest":{"$max":"$reserved_at"}}},
            ])
            .session(&mut *session)
            .await?;
        let reservations: Vec<Document> = cursor.stream(&mut *session).try_collect().await?;
        let deleted_at = DateTime::parse_rfc3339_str(&t.deleted_at)
            .map_err(|_| DocumentError::Corrupt)?
            .timestamp_millis();
        let latest = reservations
            .first()
            .and_then(|r| r.get_datetime("latest").ok())
            .map(|d| d.timestamp_millis())
            .unwrap_or(deleted_at)
            .max(deleted_at);
        lifecycle
            .update_one(
                doc! {"_id":&t.id},
                doc! {"$set":{"purge.last_reserved_at":DateTime::from_millis(latest)}},
            )
            .session(&mut *session)
            .await?;
        Ok(DeleteOutcome::Deleted)
    }
    async fn reconcile(&self, snapshot: &mut ClientSession) -> Result<Self::Output, DocumentError> {
        let row = self
            .store
            .lifecycle_rows()
            .find_one(doc! {"_id":&self.tombstone.id})
            .read_concern(ReadConcern::snapshot())
            .max_time(IO)
            .session(&mut *snapshot)
            .await
            .map_err(unavailable)?;
        let main = self
            .store
            .rows
            .find_one(doc! {"_id":&self.tombstone.id})
            .read_concern(ReadConcern::snapshot())
            .max_time(IO)
            .session(&mut *snapshot)
            .await
            .map_err(unavailable)?;
        match row.as_ref().map(tombstone).transpose()?.flatten() {
            Some(actual) if actual == *self.tombstone && main.is_none() => {
                Ok(DeleteOutcome::Deleted)
            }
            _ => Err(DocumentError::Unavailable),
        }
    }
}

pub(super) fn object_range(owner: &str, id: &str) -> Document {
    doc! {"_id":{"$gte":format!("object:{owner}:{id}:"),"$lt":format!("object:{owner}:{id};")}}
}

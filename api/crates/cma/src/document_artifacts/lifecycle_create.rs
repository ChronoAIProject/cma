//! Creation publishes metadata, identity and a ledgered source atomically.
use super::{
    mongo::{encode, MongoDocumentStore},
    quota::{object_row, unavailable, IO},
    transaction::{self, Error, Operation},
};
use layer2_core::document_artifacts::*;
use mongodb::{
    bson::{doc, Document},
    options::ReadConcern,
    ClientSession,
};

pub(super) async fn insert(
    store: &MongoDocumentStore,
    record: &StoredDocument,
    source: Option<&DocumentSourceReservation>,
) -> Result<InsertOutcome, DocumentError> {
    let encoded = encode(record)?;
    transaction::run(
        store.rows.client(),
        Create {
            store,
            record,
            encoded,
            source,
        },
    )
    .await
}
struct Create<'a> {
    store: &'a MongoDocumentStore,
    record: &'a StoredDocument,
    encoded: Document,
    source: Option<&'a DocumentSourceReservation>,
}
#[async_trait::async_trait]
impl Operation for Create<'_> {
    type Output = InsertOutcome;
    async fn apply(&self, session: &mut ClientSession) -> Result<Self::Output, Error> {
        self.store.lifecycle_rows().insert_one(doc! {"_id":&self.record.metadata.id,"document_schema_version":1_i64,"owner_hash":digest(self.record.metadata.owner_subject.as_bytes()),"state":"live","created_at":mongodb::bson::DateTime::now()}).session(&mut *session).await?;
        self.store
            .rows
            .insert_one(self.encoded.clone())
            .session(&mut *session)
            .await?;
        if let Some(source) = self.source {
            let object = object_row(source)?;
            let changed = self.store.quota_rows().update_one(doc! {"_id":object.get_str("_id").map_err(|_|DocumentError::Corrupt)?,"$or":[{"state":{"$in":["reserved","committed"]}},{"state":{"$exists":false}}]},doc! {"$set":{"state":"committed"}}).session(&mut *session).await?;
            if changed.matched_count != 1 {
                return Err(DocumentError::Unavailable.into());
            }
        }
        Ok(InsertOutcome::Inserted)
    }
    async fn duplicate(&self) -> Result<Option<Self::Output>, DocumentError> {
        Ok(Some(
            if self
                .store
                .read_tombstone(&self.record.metadata.id)
                .await?
                .is_some()
            {
                InsertOutcome::Deleted
            } else {
                InsertOutcome::Exists
            },
        ))
    }
    async fn reconcile(&self, snapshot: &mut ClientSession) -> Result<Self::Output, DocumentError> {
        let main = self
            .store
            .rows
            .find_one(doc! {"_id":&self.record.metadata.id})
            .read_concern(ReadConcern::snapshot())
            .max_time(IO)
            .session(&mut *snapshot)
            .await
            .map_err(unavailable)?;
        let lifecycle = self
            .store
            .lifecycle_rows()
            .find_one(doc! {"_id":&self.record.metadata.id})
            .read_concern(ReadConcern::snapshot())
            .max_time(IO)
            .session(&mut *snapshot)
            .await
            .map_err(unavailable)?;
        if main.as_ref() != Some(&self.encoded)
            || !lifecycle.is_some_and(|r| r.get_str("state").ok() == Some("live"))
        {
            return Err(DocumentError::Unavailable);
        }
        if let Some(source) = self.source {
            let row = object_row(source)?;
            let object = self
                .store
                .quota_rows()
                .find_one(doc! {"_id":row.get_str("_id").map_err(|_|DocumentError::Corrupt)?})
                .read_concern(ReadConcern::snapshot())
                .max_time(IO)
                .session(snapshot)
                .await
                .map_err(unavailable)?;
            if !object.is_some_and(|r| r.get_str("state").ok() == Some("committed")) {
                return Err(DocumentError::Unavailable);
            }
        }
        Ok(InsertOutcome::Inserted)
    }
}

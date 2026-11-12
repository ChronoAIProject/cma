//! Account erasure authority and its append-only audit, independent of Sandboxes.
use futures_util::TryStreamExt;
use layer2_core::{account_deletion::*, StoreError, SubjectId};
use mongodb::{
    bson::{doc, Document},
    Collection, Database,
};
use std::{collections::BTreeSet, time::Duration};

mod events;
mod indexes;
mod records;
mod requests;
#[cfg(test)]
mod snapshot_tests;
#[cfg(test)]
mod tests;
mod writes;

pub(crate) const RECORDS: &str = "account_deletions";
pub(crate) const EVENTS: &str = "account_deletion_events";
use events::event_document;
use records::{key_hash, read_record, record_document};

#[derive(Clone)]
pub(crate) struct MongoAccountDeletionStore {
    database: Database,
    records: Collection<Document>,
    events: Collection<Document>,
}
impl MongoAccountDeletionStore {
    pub(crate) async fn open(database: &Database) -> Result<Self, StoreError> {
        super::sandbox_mongo::require_transaction_topology(database).await?;
        super::schema::initialize_collections(database, &[RECORDS, EVENTS, "sandbox_admissions"])
            .await?;
        let store = Self {
            database: database.clone(),
            records: super::durability::majority_collection(database, RECORDS),
            events: super::durability::majority_collection(database, EVENTS),
        };
        indexes::ensure(&store).await.map_err(|error| {
            StoreError::Unavailable(format!("initialize account deletion indexes: {error}"))
        })?;
        Ok(store)
    }
    pub(crate) async fn active_hashes(&self) -> Result<BTreeSet<String>, AccountDeletionError> {
        let mut cursor = self
            .records
            .find(doc! {"status":{"$ne":"completed"}})
            .projection(doc! {"_id":0,"subject_hash":1,"active_subject_hash":1})
            .await
            .map_err(driver)?;
        let mut hashes = BTreeSet::new();
        while let Some(row) = cursor.try_next().await.map_err(driver)? {
            hashes.insert(records::active_hash(&row)?.into());
        }
        Ok(hashes)
    }
    pub(crate) async fn has_request_key(
        &self,
        subject: &SubjectId,
        key: &str,
    ) -> Result<bool, AccountDeletionError> {
        self.records
            .find_one(doc! {"subject_hash":subject_hash(subject),"request_keys":key_hash(key)})
            .projection(doc! {"_id":1})
            .await
            .map(|row| row.is_some())
            .map_err(driver)
    }
}
#[async_trait::async_trait]
impl AccountDeletionStore for MongoAccountDeletionStore {
    async fn request(
        &self,
        subject: &SubjectId,
        key: &str,
        at: &str,
    ) -> Result<AccountDeletion, AccountDeletionError> {
        requests::request(self, subject, key, at).await
    }
    async fn active(
        &self,
        subject: &SubjectId,
    ) -> Result<Option<AccountDeletion>, AccountDeletionError> {
        self.records
            .find_one(doc! {"subject_hash":subject_hash(subject),"status":{"$ne":"completed"}})
            .await
            .map_err(driver)?
            .map(read_record)
            .transpose()
    }
    async fn latest(
        &self,
        subject: &SubjectId,
    ) -> Result<Option<AccountDeletion>, AccountDeletionError> {
        self.records
            .find_one(doc! {"subject_hash":subject_hash(subject)})
            .sort(doc! {"active_subject_hash":-1,"requested_at":-1,"_id":-1})
            .await
            .map_err(driver)?
            .map(read_record)
            .transpose()
    }
    async fn get(
        &self,
        id: &AccountDeletionId,
    ) -> Result<Option<AccountDeletion>, AccountDeletionError> {
        self.records
            .find_one(doc! {"_id":id.as_str()})
            .await
            .map_err(driver)?
            .map(read_record)
            .transpose()
    }
    async fn claim_due(
        &self,
        holder: &str,
        now: &str,
        lease: Duration,
    ) -> Result<Option<AccountDeletion>, AccountDeletionError> {
        writes::claim_due(self, holder, now, lease).await
    }
    async fn commit(
        &self,
        deletion: AccountDeletion,
        events: Vec<ErasureEvent>,
        expected_revision: u64,
    ) -> Result<(), AccountDeletionError> {
        writes::commit(self, deletion, events, expected_revision, None).await
    }
}
/// Must execute after an owner fence write in the same transaction.
pub(crate) async fn erasing_in_session(
    database: &Database,
    subject: &SubjectId,
    session: &mut mongodb::ClientSession,
) -> Result<bool, mongodb::error::Error> {
    database
        .collection::<Document>(RECORDS)
        .find_one(doc! {"subject_hash":subject_hash(subject),"status":{"$ne":"completed"}})
        .projection(doc! {"_id":1})
        .session(session)
        .await
        .map(|record| record.is_some())
}
/// Serialize a recipient grant or admission with account deletion before reading.
pub(crate) async fn fence_subject_in_session(
    database: &Database,
    subject: &SubjectId,
    session: &mut mongodb::ClientSession,
) -> Result<bool, mongodb::error::Error> {
    database.collection::<Document>("sandbox_admissions").update_one(
        doc!{"_id":subject.as_str()},
        doc!{"$inc":{"fence":1_i64},"$setOnInsert":{"document_schema_version":super::schema::DOCUMENT_SCHEMA_VERSION}},
    ).upsert(true).session(&mut *session).await?;
    erasing_in_session(database, subject, session).await
}
fn driver(error: mongodb::error::Error) -> AccountDeletionError {
    AccountDeletionError::Store(format!("account deletion persistence: {error}"))
}
async fn backoff(attempt: usize, jitter: u64) {
    if attempt > 0 {
        let delay = (10_u64 << (attempt - 1).min(3)) + (jitter.rotate_left(attempt as u32) & 15);
        tokio::time::sleep(Duration::from_millis(delay)).await;
    }
}

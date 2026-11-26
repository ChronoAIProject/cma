//! Account erasure authority and its append-only audit, independent of Sandboxes.
use futures_util::TryStreamExt;
use layer2_core::{account_deletion::*, StoreError, SubjectId};
use mongodb::{
    bson::{doc, Document},
    options::Hint,
    Collection, Database,
};
use std::{collections::BTreeSet, time::Duration};

mod completion_guard;
mod events;
mod indexes;
mod records;
mod requests;
#[cfg(test)]
mod snapshot_tests;
mod standing;
#[cfg(test)]
mod tests;
pub(in crate::store) mod trigger_erasure;
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
    pub(crate) fn from_database(database: &Database) -> Self {
        Self {
            database: database.clone(),
            records: super::durability::majority_collection(database, RECORDS),
            events: super::durability::majority_collection(database, EVENTS),
        }
    }
    pub(crate) async fn require_admission_schema(&self) -> Result<(), StoreError> {
        indexes::verify(self).await
    }
    #[cfg(test)]
    pub(crate) async fn open(database: &Database) -> Result<Self, StoreError> {
        super::sandbox_mongo::require_transaction_topology(database).await?;
        super::schema::initialize_collections(database, &[RECORDS, EVENTS, "sandbox_admissions"])
            .await?;
        let store = Self::from_database(database);
        indexes::ensure(&store).await.map_err(|error| {
            StoreError::Unavailable(format!("initialize account deletion indexes: {error}"))
        })?;
        Ok(store)
    }
    pub(crate) async fn active_hashes(&self) -> Result<BTreeSet<String>, AccountDeletionError> {
        let mut hashes = BTreeSet::new();
        if self
            .database
            .list_collection_names()
            .filter(doc! {"name":RECORDS})
            .await
            .map_err(driver)?
            .is_empty()
        {
            return Ok(hashes);
        }
        // Arrays need a separate branch and partial index: scalar equality also
        // matches array elements, and a general status index scans tombstones.
        let hints = [
            "account_deletions_active_subject",
            "account_deletions_status_subject",
            "account_deletions_array_status_subject",
        ];
        for (filter, hint) in erasing_filters().into_iter().zip(hints) {
            let mut cursor = self
                .records
                .find(filter)
                .hint(Hint::Name(hint.into()))
                .projection(doc! {"_id":0,"subject_hash":1,"active_subject_hash":1})
                .await
                .map_err(driver)?;
            while let Some(row) = cursor.try_next().await.map_err(driver)? {
                match records::active_hash(&row) {
                    Ok(hash) => {
                        hashes.insert(hash.into());
                    }
                    Err(error) => {
                        tracing::warn!(%error, "malformed account deletion snapshot marker");
                        if let Some(hash) = records::recover_subject_hash(&row) {
                            hashes.insert(hash.into());
                        }
                    }
                }
            }
        }
        Ok(hashes)
    }
    pub(crate) async fn erasing(&self, subject: &SubjectId) -> Result<bool, AccountDeletionError> {
        standing::erasing(self, subject).await
    }
}
fn erasing_filters() -> [Document; 3] {
    [
        doc! {"active_subject_hash":{"$exists":true}},
        doc! {"status":{"$ne":"completed"}},
        doc! {"status":{"$type":"array"}},
    ]
}
fn subject_erasing_filter(subject: &SubjectId) -> Document {
    doc! {"subject_hash":subject_hash(subject),"$or":erasing_filters().to_vec()}
}
#[async_trait::async_trait]
impl AccountDeletionStore for MongoAccountDeletionStore {
    async fn replay(
        &self,
        subject: &SubjectId,
        key: &str,
    ) -> Result<Option<AccountDeletion>, AccountDeletionError> {
        self.records
            .find_one(doc! {"subject_hash":subject_hash(subject),"request_keys":key_hash(key)})
            .projection(doc! {"request_keys":0})
            .await
            .map_err(driver)?
            .map(read_record)
            .transpose()
    }
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
            .projection(doc! {"request_keys":0})
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
        expected_lease_token: Option<&str>,
    ) -> Result<(), AccountDeletionError> {
        writes::commit(
            self,
            deletion,
            events,
            expected_revision,
            None,
            expected_lease_token,
        )
        .await
    }
}
/// Read standing from the transaction snapshot. Fence the subject first only
/// when the caller requires serialization with account deletion.
pub(crate) async fn erasing_in_session(
    database: &Database,
    subject: &SubjectId,
    session: &mut mongodb::ClientSession,
) -> Result<bool, mongodb::error::Error> {
    database
        .collection::<Document>(RECORDS)
        .find_one(subject_erasing_filter(subject))
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

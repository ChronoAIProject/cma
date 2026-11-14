use super::{schema::COLLECTION, *};
use mongodb::{
    bson::{doc, spec::BinarySubtype, Binary, Document},
    options::{
        Acknowledgment, CollectionOptions, ReadConcern, ReadPreference, ReturnDocument,
        SelectionCriteria, WriteConcern,
    },
    Collection, Database,
};

#[derive(Clone)]
pub struct MongoSessionBackend {
    pub(super) db: Database,
    pub(super) rows: Collection<Document>,
}

pub(super) fn majority() -> WriteConcern {
    WriteConcern::builder()
        .w(Acknowledgment::Majority)
        .w_timeout(SERVER_TIMEOUT)
        .build()
}

impl MongoSessionBackend {
    /// Construction does no DDL. `schema::verify` is an activation prerequisite.
    pub fn new(db: Database) -> Self {
        let rows = db.collection_with_options(
            COLLECTION,
            CollectionOptions::builder()
                .read_concern(ReadConcern::majority())
                .write_concern(majority())
                .selection_criteria(SelectionCriteria::ReadPreference(ReadPreference::Primary))
                .build(),
        );
        Self { db, rows }
    }

    pub(super) async fn observe(
        &self,
        id: &str,
        touch: bool,
    ) -> Result<Option<Stored>, StoreError> {
        let mut set = doc! {"observed_at":"$$NOW", "fence":{"$add":["$fence",1_i64]}};
        if touch {
            set.insert("last_seen", "$$NOW");
            set.insert(
                "expires_at",
                doc! {"$min":["$absolute_at",{"$add":["$$NOW","$idle_ms"]}]},
            );
        }
        // A real majority write supplies the final admission point. A cached
        // positive or a majority snapshot alone can lag a completed logout.
        let row = self
            .rows
            .find_one_and_update(live(id), vec![doc! {"$set":set}])
            .return_document(ReturnDocument::After)
            .max_time(SERVER_TIMEOUT)
            .await
            .map_err(|_| StoreError::Unavailable)?;
        row.map(stored).transpose()
    }

    async fn revoked(&self, id: &str) -> Result<bool, StoreError> {
        let rows = self.db.collection_with_options::<Document>(
            COLLECTION,
            CollectionOptions::builder()
                .read_concern(ReadConcern::linearizable())
                .selection_criteria(SelectionCriteria::ReadPreference(ReadPreference::Primary))
                .build(),
        );
        Ok(rows
            .find_one(doc! {"_id":id,"kind":"session"})
            .max_time(SERVER_TIMEOUT)
            .await
            .map_err(|_| StoreError::Unavailable)?
            .is_none())
    }
}

impl SessionBackend for MongoSessionBackend {
    fn create<'a>(
        &'a self,
        id: &'a str,
        subject_hash: &'a str,
        payload: Vec<u8>,
        access_ttl_ms: i64,
        limits: &'a SessionLimits,
    ) -> BoxFuture<'a, Result<Stored, StoreError>> {
        Box::pin(self.create_session(id, subject_hash, payload, access_ttl_ms, limits))
    }

    fn load<'a>(
        &'a self,
        id: &'a str,
        touch: bool,
    ) -> BoxFuture<'a, Result<Option<Stored>, StoreError>> {
        Box::pin(async move {
            if !identifier(id) {
                return Err(StoreError::Invalid);
            }
            bounded("load", self.observe(id, touch)).await
        })
    }

    fn compare_exchange<'a>(
        &'a self,
        id: &'a str,
        expected: &'a [u8],
        next: Vec<u8>,
        access_ttl_ms: Option<i64>,
    ) -> BoxFuture<'a, Result<Option<Stored>, StoreError>> {
        Box::pin(async move {
            if !identifier(id)
                || !payload_valid(expected)
                || !payload_valid(&next)
                || access_ttl_ms.is_some_and(|ttl| ttl <= 0 || ttl > 86_400_000)
            {
                return Err(StoreError::Invalid);
            }
            let mut filter = live(id);
            filter.insert("payload", binary(expected.to_vec()));
            let mut set = doc! {"payload":{"$literal":binary(next)},"observed_at":"$$NOW","fence":{"$add":["$fence",1_i64]}};
            if let Some(ttl) = access_ttl_ms {
                set.insert("access_expires_at", doc! {"$add":["$$NOW",ttl]});
            }
            bounded("compare_exchange", async {
                self.rows
                    .find_one_and_update(filter, vec![doc! {"$set":set}])
                    .return_document(ReturnDocument::After)
                    .max_time(SERVER_TIMEOUT)
                    .await
                    .map_err(|_| StoreError::Unavailable)?
                    .map(stored)
                    .transpose()
            })
            .await
        })
    }

    fn revoke<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<Option<Vec<u8>>, StoreError>> {
        Box::pin(async move {
            if !identifier(id) {
                return Err(StoreError::Invalid);
            }
            let deleted = bounded("revoke", async {
                self.rows
                    .find_one_and_delete(doc! {"_id":id,"kind":"session"})
                    .max_time(SERVER_TIMEOUT)
                    .await
                    .map_err(|_| StoreError::Unavailable)
            })
            .await;
            if let Ok(Some(row)) = deleted {
                let payload = row
                    .get_binary_generic("payload")
                    .map_err(|_| StoreError::Invalid)?
                    .clone();
                if !payload_valid(&payload) {
                    return Err(StoreError::Invalid);
                }
                return Ok(Some(payload));
            }
            // A timed-out acknowledgement may hide a committed deletion. Only
            // primary linearizable absence resolves it; no local cookie verdict.
            if bounded("revoke_readback", self.revoked(id)).await? {
                Ok(None)
            } else {
                Err(StoreError::Unavailable)
            }
        })
    }
}

pub(super) fn live(id: &str) -> Document {
    let mut filter = live_all();
    filter.insert("_id", id);
    filter
}

pub(super) fn live_all() -> Document {
    doc! {"kind":"session","fence":{"$lt":i64::MAX},
    "$expr":{"$and":[{"$gt":["$expires_at","$$NOW"]},{"$gt":["$absolute_at","$$NOW"]}]}}
}

pub(super) fn binary(bytes: Vec<u8>) -> Binary {
    Binary {
        subtype: BinarySubtype::Generic,
        bytes,
    }
}

pub(super) fn stored(row: Document) -> Result<Stored, StoreError> {
    let date = |key| {
        row.get_datetime(key)
            .map(|value| value.timestamp_millis())
            .map_err(|_| StoreError::Invalid)
    };
    let payload = row
        .get_binary_generic("payload")
        .map_err(|_| StoreError::Invalid)?
        .clone();
    if !payload_valid(&payload) {
        return Err(StoreError::Invalid);
    }
    let subject_hash = row
        .get_str("subject_hash")
        .map_err(|_| StoreError::Invalid)?
        .to_owned();
    if !identifier(&subject_hash) {
        return Err(StoreError::Invalid);
    }
    Ok(Stored {
        payload,
        subject_hash,
        now_ms: date("observed_at")?,
        created_ms: date("created_at")?,
        last_seen_ms: date("last_seen")?,
        expires_ms: date("absolute_at")?,
        access_expires_ms: date("access_expires_at")?,
        evicted: Vec::new(),
    })
}

pub(super) async fn bounded<T>(
    operation: &'static str,
    future: impl std::future::Future<Output = Result<T, StoreError>>,
) -> Result<T, StoreError> {
    let result = tokio::time::timeout(TIMEOUT, future)
        .await
        .unwrap_or(Err(StoreError::Unavailable));
    if result.is_err() {
        tracing::warn!(operation, "console session storage operation failed");
    }
    result
}

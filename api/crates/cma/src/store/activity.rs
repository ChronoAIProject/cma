//! MongoDB persistence for secret-safe activity records.

use std::time::Duration;

use futures_util::TryStreamExt as _;
use layer2_core::{ActivityAction, ActivityQuery, ActivityRecord, ActivityStore, Page, StoreError};
use mongodb::{
    bson::{doc, Bson, DateTime, Document},
    options::IndexOptions,
    Client, Collection, IndexModel,
};
use serde::{Deserialize, Serialize};

use super::{
    durability::majority_collection,
    schema::{self, DOCUMENT_SCHEMA_VERSION},
};

pub(super) const ACTIVITY: &str = "activity";
const EXPIRY_INDEX: &str = "activity_expiry";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct ActivityDoc {
    #[serde(rename = "_id")]
    id: String,
    document_schema_version: i64,
    recorded_at: DateTime,
    record: ActivityRecord,
}

impl ActivityDoc {
    pub(super) fn from_record(record: &ActivityRecord) -> Self {
        Self {
            id: record.id.0.clone(),
            document_schema_version: DOCUMENT_SCHEMA_VERSION,
            recorded_at: DateTime::now(),
            record: record.clone(),
        }
    }

    fn cursor(&self) -> String {
        format!("{}:{}", self.recorded_at.timestamp_millis(), self.id)
    }
}

/// Durable activity adapter with bounded, cursor-based reads.
pub struct MongoActivityStore {
    entries: Collection<ActivityDoc>,
}

impl MongoActivityStore {
    pub(super) async fn open(
        client: &Client,
        database: &str,
        retention: Duration,
    ) -> Result<Self, StoreError> {
        let database = client.database(database);
        schema::initialize_collections(&database, &[ACTIVITY]).await?;
        let store = Self {
            entries: majority_collection(&database, ACTIVITY),
        };
        store.ensure_indexes(retention).await?;
        Ok(store)
    }

    async fn ensure_indexes(&self, retention: Duration) -> Result<(), StoreError> {
        for keys in [
            doc! { "record.actor": 1, "recorded_at": -1, "_id": -1 },
            doc! { "record.action": 1, "recorded_at": -1, "_id": -1 },
        ] {
            self.entries
                .create_index(IndexModel::builder().keys(keys).build())
                .await
                .map_err(|error| StoreError::Unavailable(format!("activity index: {error}")))?;
        }
        let expiry = IndexModel::builder()
            .keys(doc! { "recorded_at": 1 })
            .options(
                IndexOptions::builder()
                    .name(EXPIRY_INDEX.to_owned())
                    .expire_after(retention)
                    .build(),
            )
            .build();
        // A retention mismatch requires a reviewed migration. An index error
        // must never authorize changing TTL policy during ordinary startup.
        self.entries.create_index(expiry).await.map_err(|error| {
            StoreError::Unavailable(format!(
                "activity expiry index is incompatible or unavailable; explicit migration required: {error}"
            ))
        })?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl ActivityStore for MongoActivityStore {
    async fn append(&self, record: &ActivityRecord) -> Result<(), StoreError> {
        self.entries
            .insert_one(ActivityDoc::from_record(record))
            .await
            .map_err(|error| StoreError::Unavailable(format!("append activity: {error}")))?;
        Ok(())
    }

    async fn append_many(&self, records: &[ActivityRecord]) -> Result<(), StoreError> {
        if records.is_empty() {
            return Ok(());
        }
        match self
            .entries
            .insert_many(records.iter().map(ActivityDoc::from_record))
            .ordered(false)
            .await
        {
            Ok(_) => Ok(()),
            Err(error) if duplicate_batch(&error) => Ok(()),
            Err(error) => Err(StoreError::Unavailable(format!(
                "append activity batch: {error}"
            ))),
        }
    }

    async fn list(&self, query: &ActivityQuery) -> Result<Page<ActivityRecord>, StoreError> {
        query
            .validate()
            .map_err(|error| StoreError::Malformed(error.to_string()))?;
        let mut clauses = Vec::<Document>::new();
        if let Some(actor) = &query.actor {
            clauses.push(doc! { "record.actor": actor.as_str() });
        }
        if let Some(action) = query.action {
            clauses.push(doc! { "record.action": action_bson(action)? });
        }
        if let Some(cursor) = query.cursor.as_deref() {
            let (recorded_at, id) = decode_cursor(cursor)?;
            clauses.push(doc! {
                "$or": [
                    { "recorded_at": { "$lt": recorded_at } },
                    { "recorded_at": recorded_at, "_id": { "$lt": id } },
                ]
            });
        }
        let filter = match clauses.len() {
            0 => doc! {},
            1 => clauses.pop().expect("one clause exists"),
            _ => doc! { "$and": clauses },
        };
        let fetch = i64::from(query.limit.saturating_add(1));
        let mut cursor = self
            .entries
            .find(filter)
            .sort(doc! { "recorded_at": -1, "_id": -1 })
            .limit(fetch)
            .await
            .map_err(|error| StoreError::Unavailable(format!("read activity: {error}")))?;
        let mut rows = Vec::new();
        while let Some(row) = cursor
            .try_next()
            .await
            .map_err(|error| StoreError::Unavailable(format!("read activity cursor: {error}")))?
        {
            schema::require_current(row.document_schema_version)?;
            rows.push(row);
        }
        let has_more = rows.len() > query.limit as usize;
        if has_more {
            rows.pop();
        }
        let next_cursor = has_more.then(|| {
            rows.last()
                .expect("a non-empty overfull page has a retained row")
                .cursor()
        });
        Ok(Page::new(
            rows.into_iter().map(|row| row.record).collect(),
            next_cursor,
        ))
    }
}

fn duplicate_batch(error: &mongodb::error::Error) -> bool {
    // Unordered retries may contain both already-inserted IDs and new rows.
    // A write-concern error still means majority durability is unconfirmed.
    match error.kind.as_ref() {
        mongodb::error::ErrorKind::InsertMany(failure) => {
            failure.write_concern_error.is_none()
                && failure.write_errors.as_ref().is_some_and(|errors| {
                    !errors.is_empty() && errors.iter().all(|error| error.code == 11000)
                })
        }
        _ => false,
    }
}

fn action_bson(action: ActivityAction) -> Result<Bson, StoreError> {
    mongodb::bson::serialize_to_bson(&action)
        .map_err(|error| StoreError::Malformed(format!("activity action: {error}")))
}

fn decode_cursor(raw: &str) -> Result<(DateTime, &str), StoreError> {
    let (millis, id) = raw
        .split_once(':')
        .filter(|(_, id)| !id.is_empty())
        .ok_or_else(|| StoreError::Malformed("activity cursor is invalid".into()))?;
    let millis = millis
        .parse::<i64>()
        .map_err(|_| StoreError::Malformed("activity cursor is invalid".into()))?;
    Ok((DateTime::from_millis(millis), id))
}

#[cfg(test)]
mod tests {
    use layer2_core::{ActivityOutcome, SubjectId};

    use super::*;

    #[test]
    fn stored_records_and_cursors_round_trip_without_secret_inference() {
        let record = ActivityRecord::new(ActivityAction::Read, ActivityOutcome::Succeeded)
            .by(SubjectId::new("subject-a"));
        let stored = ActivityDoc::from_record(&record);
        assert_eq!(stored.document_schema_version, DOCUMENT_SCHEMA_VERSION);
        let cursor = stored.cursor();
        let (at, id) = decode_cursor(&cursor).unwrap();
        assert_eq!(at, stored.recorded_at);
        assert_eq!(id, stored.id);
        assert_eq!(stored.record, record);
    }

    #[test]
    fn malformed_cursors_are_rejected() {
        for value in ["", "123", "nope:id", "123:"] {
            assert!(decode_cursor(value).is_err(), "accepted {value:?}");
        }
    }
}

#[cfg(test)]
#[path = "activity_batch_tests.rs"]
mod batch_tests;

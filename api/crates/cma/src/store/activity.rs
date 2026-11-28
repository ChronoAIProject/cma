//! MongoDB persistence for secret-safe activity records.

use std::time::Duration;

use futures_util::TryStreamExt as _;
use layer2_core::{
    ActivityAction, ActivityChannel, ActivityId, ActivityOutcome, ActivityQuery, ActivityRecord,
    ActivityStore, ActivitySummary, Page, StoreError, ACTIVITY_OUTCOME_UNCONFIRMED,
};
use mongodb::{
    bson::{doc, Bson, DateTime, Document},
    options::Hint,
    Client, Collection,
};
use serde::{Deserialize, Serialize};

use super::{
    durability::majority_collection,
    migrations::activity_audit_index,
    schema::{self as document_schema, Contract, StartupObserver, DOCUMENT_SCHEMA_VERSION},
};

pub(super) const ACTIVITY: &str = "activity";
pub(in crate::store) mod schema;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct ActivityDoc<T = ActivityRecord> {
    #[serde(rename = "_id")]
    id: String,
    document_schema_version: i64,
    recorded_at: DateTime,
    record: T,
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
}

impl<T> ActivityDoc<T> {
    fn cursor(&self) -> String {
        format!("{}:{}", self.recorded_at.timestamp_millis(), self.id)
    }
}

/// Durable activity adapter with bounded, cursor-based reads.
pub struct MongoActivityStore {
    entries: Collection<ActivityDoc>,
    indexes: schema::PageIndexes,
}

impl MongoActivityStore {
    #[cfg(test)]
    pub(super) async fn open(
        client: &Client,
        database: &str,
        retention: Duration,
    ) -> Result<Self, StoreError> {
        Self::open_with_schema(
            client,
            database,
            retention,
            &mut StartupObserver::new(Default::default()),
        )
        .await
    }

    pub(super) async fn open_with_schema(
        client: &Client,
        database: &str,
        retention: Duration,
        observer: &mut StartupObserver,
    ) -> Result<Self, StoreError> {
        let database = client.database(database);
        document_schema::verify_existing_collections(&database, &[ACTIVITY]).await?;
        let assessment = observer
            .assess(Contract::Activity, schema::inspect(&database, retention))
            .await?;
        tracing::info!(
            audit_index_ready = assessment.capability.audit,
            global_page_index_ready = assessment.capability.global,
            "Activity query index observation"
        );
        Ok(Self {
            entries: majority_collection(&database, ACTIVITY),
            indexes: assessment.capability,
        })
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

    async fn attempt(&self, id: &ActivityId) -> Result<Option<ActivityRecord>, StoreError> {
        self.entries
            .find_one(doc! {"_id": &id.0})
            .await
            .map(|document| document.map(|entry| entry.record))
            .map_err(|_| StoreError::Unavailable("activity lookup unavailable".into()))
    }

    async fn identify_attempt(&self, record: &ActivityRecord) -> Result<(), StoreError> {
        let mut filter = open_attempt(&record.id);
        filter.insert("record.actor", Bson::Null);
        let metadata = mongodb::bson::serialize_to_bson(&record.detail)
            .map_err(|error| StoreError::Malformed(error.to_string()))?;
        let target = mongodb::bson::serialize_to_bson(&record.target)
            .map_err(|error| StoreError::Malformed(error.to_string()))?;
        let actor = record
            .actor
            .as_ref()
            .ok_or_else(|| StoreError::Malformed("missing audit actor".into()))?;
        self.update_attempt(
            filter,
            doc! {
                "record.actor": actor.as_str(),
                "record.detail": metadata,
                "record.target": target,
            },
        )
        .await
    }

    async fn finish_attempt(
        &self,
        id: &ActivityId,
        outcome: &ActivityOutcome,
        status: u16,
    ) -> Result<(), StoreError> {
        let outcome = mongodb::bson::serialize_to_bson(outcome)
            .map_err(|error| StoreError::Malformed(error.to_string()))?;
        self.update_attempt(
            open_attempt(id),
            doc! {
                "record.outcome": outcome,
                "record.detail.http_status": status.to_string(),
            },
        )
        .await
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
        self.read_page(query, None).await
    }

    async fn list_summaries(
        &self,
        query: &ActivityQuery,
    ) -> Result<Page<ActivitySummary>, StoreError> {
        self.read_page(query, Some(summary_projection())).await
    }
}

fn open_attempt(id: &ActivityId) -> Document {
    doc! {"_id": &id.0, "record.outcome.status": "failed",
    "record.outcome.code": ACTIVITY_OUTCOME_UNCONFIRMED}
}

fn summary_projection() -> Document {
    doc! {"_id":1,"document_schema_version":1,"recorded_at":1,
    "record.id":1,"record.at":1,"record.actor":1,"record.action":1,
    "record.target":1,"record.outcome":1,"record.detail.activity_channel":1,
    "record.detail.request_id":1,"record.detail.request_method":1,"record.detail.request_route":1,
    "record.detail.credential_kind":1,"record.detail.actor_scope":1,"record.detail.http_status":1}
}

impl MongoActivityStore {
    async fn update_attempt(&self, filter: Document, update: Document) -> Result<(), StoreError> {
        let result = self
            .entries
            .update_one(filter, doc! {"$set": update})
            .await
            .map_err(|error| {
                StoreError::Unavailable(format!("update activity attempt: {error}"))
            })?;
        if result.matched_count == 1 {
            Ok(())
        } else {
            Err(StoreError::Unavailable(
                "activity attempt is absent or settled".into(),
            ))
        }
    }

    async fn read_page<T: serde::de::DeserializeOwned + Send + Sync + Unpin>(
        &self,
        query: &ActivityQuery,
        projection: Option<Document>,
    ) -> Result<Page<T>, StoreError> {
        let started = std::time::Instant::now();
        let view = if projection.is_some() {
            "summary"
        } else {
            "full"
        };
        let result = self.read_page_inner(query, projection).await;
        metrics::histogram!("cma_activity_read_seconds", "view" => view,
            "outcome" => if result.is_ok() { "completed" } else { "error" })
        .record(started.elapsed().as_secs_f64());
        result
    }

    async fn read_page_inner<T: serde::de::DeserializeOwned + Send + Sync + Unpin>(
        &self,
        query: &ActivityQuery,
        projection: Option<Document>,
    ) -> Result<Page<T>, StoreError> {
        query
            .validate()
            .map_err(|error| StoreError::Malformed(error.to_string()))?;
        let default_audit =
            query.exclude_successful_reads && query.actor.is_none() && query.action.is_none();
        let use_audit_index = default_audit && self.indexes.audit;
        let use_global_index = default_audit && !use_audit_index && self.indexes.global;
        let mut clauses = Vec::<Document>::new();
        if let Some(actor) = &query.actor {
            clauses.push(doc! { "record.actor": actor.as_str() });
        }
        if let Some(action) = query.action {
            clauses.push(doc! { "record.action": action_bson(action)? });
        }
        if let Some(result) = query.result {
            clauses.push(doc! { "record.outcome.status": result.as_str() });
        }
        if let Some(channel) = query.channel {
            if channel == ActivityChannel::Unknown {
                clauses.push(
                    doc! { "record.detail.activity_channel": { "$in": ["unknown", Bson::Null] } },
                );
            } else {
                clauses.push(doc! { "record.detail.activity_channel": channel.as_str() });
            }
        }
        let (from, before) = query
            .recorded_bounds_ms()
            .map_err(|error| StoreError::Malformed(error.to_string()))?;
        if from.is_some() || before.is_some() {
            let mut range = Document::new();
            if let Some(from) = from {
                range.insert("$gte", DateTime::from_millis(from));
            }
            if let Some(before) = before {
                range.insert("$lt", DateTime::from_millis(before));
            }
            clauses.push(doc! { "recorded_at": range });
        }
        if use_audit_index {
            clauses.push(activity_audit_index::filter());
        } else if query.exclude_successful_reads {
            let reads = [
                ActivityAction::Read,
                ActivityAction::List,
                ActivityAction::GatewayRecordRead,
            ]
            .into_iter()
            .map(action_bson)
            .collect::<Result<Vec<_>, _>>()?;
            clauses.push(doc! {
                "$nor": [{
                    "record.action": { "$in": reads },
                    "record.outcome.status": "succeeded",
                }],
            });
        }
        let boundary = query.cursor.as_deref().map(decode_cursor).transpose()?;
        if let Some((recorded_at, id)) = boundary.filter(|_| use_global_index) {
            // MongoDB's min bound is inclusive on the two-key global index.
            // Exclude only the exact cursor tuple to keep pagination exclusive.
            clauses.push(doc! { "$nor": [{ "recorded_at": recorded_at, "_id": id }] });
        } else if let Some((recorded_at, id)) = boundary.filter(|_| !use_audit_index) {
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
        let entries = self.entries.clone_with_type::<ActivityDoc<T>>();
        let sort = doc! { "recorded_at": -1, "_id": -1 };
        let find = entries
            .find(filter)
            .with_options(
                mongodb::options::FindOptions::builder()
                    .projection(projection)
                    .batch_size(query.limit + 1)
                    .max_time(Duration::from_secs(5))
                    .build(),
            )
            .sort(sort.clone())
            .limit(fetch);
        let find = if use_audit_index {
            let find = find.hint(Hint::Name(activity_audit_index::INDEX.into()));
            if let Some((recorded_at, id)) = boundary {
                // The prefix identifies exactly one row. Start beyond its
                // trailing action key to seek directly to the next page.
                find.min(doc! {"recorded_at":recorded_at,"_id":id,"record.action":Bson::MaxKey})
            } else {
                find
            }
        } else if use_global_index {
            let find = find.hint(Hint::Keys(sort));
            if let Some((recorded_at, id)) = boundary {
                find.min(doc! { "recorded_at": recorded_at, "_id": id })
            } else {
                find
            }
        } else {
            find
        };
        let mut cursor = find
            .await
            .map_err(|error| StoreError::Unavailable(format!("read activity: {error}")))?;
        let mut rows = Vec::new();
        while let Some(row) = cursor
            .try_next()
            .await
            .map_err(|error| StoreError::Unavailable(format!("read activity cursor: {error}")))?
        {
            document_schema::require_current(row.document_schema_version)?;
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

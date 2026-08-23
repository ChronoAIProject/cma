//! The record of what the service was asked to do, in MongoDB.
//!
//! One document per request, which is the reason this collection is the one
//! most likely to become a problem: it grows with traffic rather than with
//! usage, and nothing about the product ever deletes from it. So it expires on
//! its own, by a TTL index the store creates and keeps correct.
//!
//! Values are written exactly as the core allowlisted them. Nothing here adds
//! a field, and nothing here logs an entry's contents.

use std::collections::BTreeMap;
use std::time::Duration;

use futures_util::TryStreamExt;
use layer2_core::{
    store::{ActivityStore, StoreError},
    ActivityDetail, ActivityId, ActivityQuery, ActivityRecord, SubjectId,
};
use mongodb::bson::{doc, Bson, DateTime, Document};
use mongodb::options::IndexOptions;
use mongodb::{Client, Collection, Database, IndexModel};
use serde::{Deserialize, Serialize};

const ACTIVITY: &str = "activity";

/// Name of the index that expires entries.
///
/// Fixed so the retention can be changed on a collection that already has one.
/// MongoDB refuses to create an index that differs only in its options, and an
/// unnamed index cannot be addressed to be modified instead.
const EXPIRY_INDEX: &str = "activity_expiry";

/// How an entry is shaped on the wire.
///
/// A distinct type from the core's record on purpose: the core owns its model
/// and this owns how it is written.
#[derive(Debug, Serialize, Deserialize)]
struct ActivityDoc {
    #[serde(rename = "_id")]
    id: String,
    /// When it happened, as the core states time.
    at: String,
    /// The same instant as a date, because a TTL index can only be built on
    /// one. Written here rather than derived from `at` on read, so the index
    /// and the field a reader sees can never disagree about when this was.
    recorded_at: DateTime,
    actor: Option<String>,
    /// The action and outcome, stored as the core serialises them, so the
    /// vocabulary lives in one place rather than being mirrored here.
    action: Bson,
    outcome: Bson,
    target: Option<Bson>,
    #[serde(default)]
    detail: BTreeMap<String, String>,
}

fn to_doc(r: &ActivityRecord) -> Result<ActivityDoc, StoreError> {
    Ok(ActivityDoc {
        id: r.id.0.clone(),
        at: r.at.clone(),
        recorded_at: DateTime::now(),
        actor: r.actor.as_ref().map(|s| s.0.clone()),
        action: mongodb::bson::serialize_to_bson(&r.action)
            .map_err(|e| StoreError::Malformed(e.to_string()))?,
        outcome: mongodb::bson::serialize_to_bson(&r.outcome)
            .map_err(|e| StoreError::Malformed(e.to_string()))?,
        target: match &r.target {
            Some(t) => Some(
                mongodb::bson::serialize_to_bson(t)
                    .map_err(|e| StoreError::Malformed(e.to_string()))?,
            ),
            None => None,
        },
        detail: r
            .detail
            .iter()
            .map(|(k, v)| (k.as_str().to_string(), v.to_string()))
            .collect(),
    })
}

fn from_doc(d: ActivityDoc) -> Result<ActivityRecord, StoreError> {
    Ok(ActivityRecord {
        id: ActivityId(d.id),
        at: d.at,
        actor: d.actor.map(SubjectId),
        action: mongodb::bson::deserialize_from_bson(d.action)
            .map_err(|e| StoreError::Malformed(e.to_string()))?,
        outcome: mongodb::bson::deserialize_from_bson(d.outcome)
            .map_err(|e| StoreError::Malformed(e.to_string()))?,
        target: match d.target {
            Some(t) => Some(
                mongodb::bson::deserialize_from_bson(t)
                    .map_err(|e| StoreError::Malformed(e.to_string()))?,
            ),
            None => None,
        },
        // Through the core's allowlist rather than straight in: a key written
        // by something that predates the current set is dropped rather than
        // reappearing unreviewed.
        detail: ActivityDetail::from_stored(d.detail),
    })
}

/// A MongoDB-backed record of actions.
pub struct MongoActivityStore {
    entries: Collection<ActivityDoc>,
    database: Database,
}

impl MongoActivityStore {
    /// Open the store over an existing connection, ensuring its indexes exist.
    ///
    /// `retention` is how long an entry lives. It is configuration rather than
    /// a constant because how long a deployment must be able to account for
    /// itself is a decision about that deployment, not about this code.
    pub(super) async fn open(
        client: &Client,
        database: &str,
        retention: Duration,
    ) -> Result<Self, StoreError> {
        let database = client.database(database);
        let store = Self {
            entries: database.collection(ACTIVITY),
            database,
        };
        store.ensure_indexes(retention).await?;
        tracing::info!(
            retention_days = retention.as_secs() / 86_400,
            "activity store ready; entries older than this are removed by the database"
        );
        Ok(store)
    }

    async fn ensure_indexes(&self, retention: Duration) -> Result<(), StoreError> {
        // Every read is either "everything, newest first" or "one subject,
        // newest first", and this serves both.
        let recent = IndexModel::builder()
            .keys(doc! { "actor": 1, "recorded_at": -1 })
            .build();
        self.entries
            .create_index(recent)
            .await
            .map_err(|e| StoreError::Unavailable(format!("activity index: {e}")))?;

        let expiry = IndexModel::builder()
            .keys(doc! { "recorded_at": 1 })
            .options(
                IndexOptions::builder()
                    .name(EXPIRY_INDEX.to_string())
                    .expire_after(retention)
                    .build(),
            )
            .build();
        match self.entries.create_index(expiry).await {
            Ok(_) => Ok(()),
            Err(e) => {
                // A TTL index that already exists with a different lifetime is
                // refused rather than replaced, so a deployment that changes
                // its retention would be stuck on the old one — and silently,
                // which is the worst way to hold onto records for longer than
                // somebody decided to.
                tracing::warn!(
                    error = %e,
                    "the expiry index already exists with different terms; updating it"
                );
                self.update_expiry(retention).await.map_err(|second| {
                    StoreError::Unavailable(format!(
                        "activity expiry index: could not create ({e}) or update ({second})"
                    ))
                })
            }
        }
    }

    /// Change the lifetime of an existing expiry index in place.
    async fn update_expiry(&self, retention: Duration) -> Result<(), mongodb::error::Error> {
        self.database
            .run_command(doc! {
                "collMod": ACTIVITY,
                "index": {
                    "name": EXPIRY_INDEX,
                    "expireAfterSeconds": retention.as_secs() as i64,
                },
            })
            .await?;
        tracing::info!(
            retention_days = retention.as_secs() / 86_400,
            "the expiry of recorded actions was updated"
        );
        Ok(())
    }
}

#[async_trait::async_trait]
impl ActivityStore for MongoActivityStore {
    async fn append(&self, record: &ActivityRecord) -> Result<(), StoreError> {
        self.entries
            .insert_one(to_doc(record)?)
            .await
            .map_err(|e| StoreError::Unavailable(format!("append activity: {e}")))?;
        Ok(())
    }

    async fn recent(&self, query: &ActivityQuery) -> Result<Vec<ActivityRecord>, StoreError> {
        let filter: Document = match &query.actor {
            Some(actor) => doc! { "actor": &actor.0 },
            None => doc! {},
        };
        let mut cursor = self
            .entries
            .find(filter)
            .sort(doc! { "recorded_at": -1 })
            // Bounded in the query rather than after it: the core clamps what a
            // caller may ask for, and this is where that clamp has to reach for
            // it to mean anything to the database.
            .limit(query.limit as i64)
            .await
            .map_err(|e| StoreError::Unavailable(format!("read activity: {e}")))?;
        let mut out = Vec::new();
        while let Some(d) = cursor
            .try_next()
            .await
            .map_err(|e| StoreError::Unavailable(format!("activity cursor: {e}")))?
        {
            out.push(from_doc(d)?);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use layer2_core::{ActivityAction, ActivityOutcome, ActivityTarget, DetailKey, SessionId};

    fn record() -> ActivityRecord {
        ActivityRecord::new(ActivityAction::SessionTerminate, ActivityOutcome::Succeeded)
            .by(SubjectId("operator".into()))
            .on(ActivityTarget::Session(SessionId("sesn_1".into())))
            .with(DetailKey::Runtime, "example-runtime")
    }

    #[test]
    fn an_entry_round_trips_through_its_stored_shape() {
        let original = record();
        let back = from_doc(to_doc(&original).expect("converts")).expect("reads back");

        assert_eq!(back.id, original.id);
        assert_eq!(back.at, original.at);
        assert_eq!(back.actor, original.actor);
        assert_eq!(back.action, original.action);
        assert_eq!(back.outcome, original.outcome);
        assert_eq!(back.target, original.target);
        assert_eq!(back.detail, original.detail);
    }

    #[test]
    fn an_unattributed_entry_stores_and_reads_back_unattributed() {
        // A refused caller is exactly what this collection exists to hold, and
        // they have no subject to be attributed to.
        let original = ActivityRecord::new(ActivityAction::SessionList, ActivityOutcome::Denied);
        let back = from_doc(to_doc(&original).expect("converts")).expect("reads back");
        assert!(back.actor.is_none());
        assert!(back.target.is_none());
    }

    #[test]
    fn the_actor_is_stored_as_a_queryable_field_not_only_inside_the_document() {
        // Narrowing to one subject filters on it, so it has to be a top-level
        // field rather than something only reachable by reading every entry.
        let entry = record();
        let doc = to_doc(&entry).expect("converts");
        let json = serde_json::to_value(&doc).expect("serialises");
        assert_eq!(json["actor"], "operator");
        assert_eq!(
            json["_id"], entry.id.0,
            "the entry's own identifier is what the database enforces uniqueness on"
        );
    }

    #[test]
    fn a_stored_parameter_outside_the_allowlist_does_not_survive_being_read() {
        // The allowlist holds on the way out too: an entry written by an older
        // build cannot reintroduce a key nobody reviewed.
        let mut doc = to_doc(&record()).expect("converts");
        doc.detail
            .insert("authorization".into(), "must never reappear".into());
        let back = from_doc(doc).expect("reads back");
        let rendered = serde_json::to_string(&back).expect("serialises");
        assert!(!rendered.contains("must never reappear"), "got {rendered}");
    }

    #[test]
    fn an_entry_carries_a_date_the_database_can_expire_it_by() {
        // A TTL index only works on a date. Storing the timestamp as text
        // alone would leave the collection growing forever with nothing
        // reporting that it was.
        let doc = to_doc(&record()).expect("converts");
        assert!(doc.at.ends_with('Z'), "got {}", doc.at);
        assert!(doc.recorded_at.timestamp_millis() > 0);
    }
}

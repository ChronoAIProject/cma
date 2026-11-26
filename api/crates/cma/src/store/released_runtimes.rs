//! One majority-acknowledged released-runtime record per deployment and lane.
//!
//! Every write is a compare-and-swap on the record revision. The admission
//! fence is a conditional write in the caller's transaction, so it conflicts
//! with a concurrent promotion instead of reading a stale release.
use layer2_core::released_runtime::{ReleasedLane, ReleasedLaneRecord, ReleasedTransition};
use layer2_core::sandbox::store::SandboxStoreError;
use mongodb::{bson::doc, ClientSession, Collection, Database};
use serde::{Deserialize, Serialize};

use super::durability::majority_collection;

mod legacy_dispatch;
mod source;
mod writers;
pub(super) use legacy_dispatch::{
    pin_first_dispatch, pin_replacement_dispatch, reserve_initial_runtime_image,
};
pub(crate) use source::StoreReleasedRuntime;
pub(crate) use writers::{BootWriter, ResolverLease};

pub(crate) const COLLECTION: &str = "released_runtimes";
pub(crate) const TRANSITIONS_COLLECTION: &str = "released_runtime_transitions";
const SCHEMA_VERSION: i64 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LaneDocument {
    #[serde(rename = "_id")]
    id: String,
    deployment: String,
    document_schema_version: i64,
    revision: i64,
    record: ReleasedLaneRecord,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    decision_fence: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TransitionDocument {
    #[serde(rename = "_id")]
    id: String,
    deployment: String,
    document_schema_version: i64,
    transition: ReleasedTransition,
}

#[derive(Clone)]
pub(crate) struct MongoReleasedRuntimes {
    database: Database,
    lanes: Collection<LaneDocument>,
    transitions: Collection<TransitionDocument>,
    deployment: String,
}

fn lane_key(lane: ReleasedLane) -> &'static str {
    match lane {
        ReleasedLane::Agent => "agent",
        ReleasedLane::Machine => "machine",
    }
}

fn revision(value: u64) -> Result<i64, SandboxStoreError> {
    i64::try_from(value)
        .map_err(|_| SandboxStoreError::Corrupt("released revision overflow".into()))
}

fn unavailable(error: mongodb::error::Error) -> SandboxStoreError {
    SandboxStoreError::Unavailable(error.to_string())
}

impl MongoReleasedRuntimes {
    pub(crate) fn open(database: &Database, deployment: &str) -> Self {
        Self {
            database: database.clone(),
            lanes: majority_collection(database, COLLECTION),
            transitions: majority_collection(database, TRANSITIONS_COLLECTION),
            deployment: deployment.into(),
        }
    }

    pub(crate) async fn initialize(&self) -> Result<(), layer2_core::StoreError> {
        super::schema::initialize_collections(&self.database, &[COLLECTION, TRANSITIONS_COLLECTION])
            .await
    }

    pub(crate) async fn session(&self) -> Result<ClientSession, SandboxStoreError> {
        let mut session = self
            .database
            .client()
            .start_session()
            .await
            .map_err(unavailable)?;
        super::sandbox_mongo::start_transaction(&mut session)
            .await
            .map_err(unavailable)?;
        Ok(session)
    }

    fn id(&self, lane: ReleasedLane) -> String {
        format!("{}:{}", self.deployment, lane_key(lane))
    }

    fn verify(
        &self,
        lane: ReleasedLane,
        document: LaneDocument,
    ) -> Result<LaneDocument, SandboxStoreError> {
        if document.id != self.id(lane)
            || document.deployment != self.deployment
            || document.document_schema_version != SCHEMA_VERSION
            || document.record.lane != lane
            || revision(document.record.revision)? != document.revision
            || document.record.validate().is_err()
        {
            return Err(SandboxStoreError::Corrupt("released runtime record".into()));
        }
        Ok(document)
    }

    /// Majority-visible, validated release authority.
    pub(crate) async fn load(
        &self,
        lane: ReleasedLane,
    ) -> Result<Option<ReleasedLaneRecord>, SandboxStoreError> {
        let Some(document) = self
            .lanes
            .find_one(doc! {"_id": self.id(lane)})
            .await
            .map_err(unavailable)?
        else {
            return Ok(None);
        };
        let document = self.verify(lane, document)?;
        Ok(Some(document.record))
    }

    pub(super) async fn has_served_image(
        &self,
        lane: ReleasedLane,
        image: &str,
    ) -> Result<bool, SandboxStoreError> {
        let Some(record) = self.load(lane).await? else {
            return Ok(false);
        };
        if record.active.image == image {
            return Ok(true);
        }
        // Every superseded image, including the bootstrap, appears as
        // from_image in the immutable receipt that advanced the lane.
        let found = self
            .transitions
            .find_one(doc! {
                "deployment": &self.deployment,
                "document_schema_version": SCHEMA_VERSION,
                "transition.lane": lane_key(lane),
                "transition.from_image": image,
                "transition.to_revision": { "$lte": revision(record.active.revision)? },
            })
            .await
            .map_err(unavailable)?;
        Ok(found.is_some_and(|row| {
            row.id == self.transition_id(lane, row.transition.from_revision)
                && row.transition.from_revision.checked_add(1) == Some(row.transition.to_revision)
        }))
    }

    /// Create the lane from its verified first record once. A concurrent or
    /// earlier bootstrap wins, and its record is returned instead.
    #[cfg(test)]
    pub(crate) async fn bootstrap(
        &self,
        record: ReleasedLaneRecord,
    ) -> Result<ReleasedLaneRecord, SandboxStoreError> {
        let lane = record.lane;
        match self.insert_bootstrap(&record, None).await {
            Ok(()) => Ok(record),
            Err(SandboxStoreError::Conflict) => {
                self.load(lane).await?.ok_or(SandboxStoreError::Conflict)
            }
            Err(error) => Err(error),
        }
    }

    pub(super) async fn insert_bootstrap(
        &self,
        record: &ReleasedLaneRecord,
        session: Option<&mut ClientSession>,
    ) -> Result<(), SandboxStoreError> {
        record
            .validate()
            .map_err(|_| SandboxStoreError::Corrupt("released runtime bootstrap".into()))?;
        let document = LaneDocument {
            id: self.id(record.lane),
            deployment: self.deployment.clone(),
            document_schema_version: SCHEMA_VERSION,
            revision: revision(record.revision)?,
            record: record.clone(),
            decision_fence: None,
        };
        let insert = self.lanes.insert_one(document);
        match session {
            Some(session) => insert.session(session).await,
            None => insert.await,
        }
        .map(|_| ())
        .map_err(|error| {
            if super::sandbox_mongo::is_duplicate_key(&error) {
                SandboxStoreError::Conflict
            } else {
                unavailable(error)
            }
        })
    }

    /// Replace `current` with `next` only if its lane revision still matches.
    pub(crate) async fn replace(
        &self,
        current: &ReleasedLaneRecord,
        next: ReleasedLaneRecord,
        session: Option<&mut ClientSession>,
    ) -> Result<(), SandboxStoreError> {
        if next.lane != current.lane
            || next.revision <= current.revision
            || next.validate().is_err()
        {
            return Err(SandboxStoreError::Corrupt(
                "released runtime transition".into(),
            ));
        }
        let mut record = mongodb::bson::serialize_to_document(&next)
            .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
        // Admissions deliberately do not advance the lane revision. Updating
        // individual fields preserves an increment that committed since load.
        record.remove("admissions");
        let mut fields = doc! {"revision": revision(next.revision)?};
        for (key, value) in record {
            fields.insert(format!("record.{key}"), value);
        }
        let mut update = doc! {"$set": fields};
        if next.candidate.is_none() {
            update.insert("$unset", doc! {"record.candidate": ""});
        }
        let filter = doc! {
            "_id": self.id(current.lane),
            "document_schema_version": SCHEMA_VERSION,
            "revision": revision(current.revision)?,
        };
        let operation = self.lanes.update_one(filter, update);
        let result = match session {
            Some(session) => operation.session(session).await,
            None => operation.await,
        }
        .map_err(unavailable)?;
        if result.matched_count == 1 {
            Ok(())
        } else {
            Err(SandboxStoreError::Conflict)
        }
    }

    fn transition_id(&self, lane: ReleasedLane, from_revision: u64) -> String {
        format!("{}:{from_revision}", self.id(lane))
    }

    /// A promotion must commit its evidence alongside the lane and target
    /// updates. It remains readable after any number of later promotions.
    pub(crate) async fn record_promotion(
        &self,
        current: &ReleasedLaneRecord,
        next: ReleasedLaneRecord,
        transition: ReleasedTransition,
        session: &mut ClientSession,
    ) -> Result<(), SandboxStoreError> {
        if transition.lane != current.lane
            || transition.from_revision != current.active.revision
            || transition.from_image != current.active.image
            || transition.to_revision != next.active.revision
            || transition.to_image != next.active.image
            || transition.at != next.active.promoted_at
            || current
                .candidate
                .as_ref()
                .is_none_or(|candidate| candidate.observation_id != transition.observation_id)
        {
            return Err(SandboxStoreError::Corrupt(
                "released runtime promotion".into(),
            ));
        }
        self.replace(current, next, Some(&mut *session)).await?;
        self.transitions
            .insert_one(TransitionDocument {
                id: self.transition_id(transition.lane, transition.from_revision),
                deployment: self.deployment.clone(),
                document_schema_version: SCHEMA_VERSION,
                transition,
            })
            .session(session)
            .await
            .map_err(unavailable)?;
        Ok(())
    }

    pub(crate) async fn committed(
        &self,
        expected: &ReleasedTransition,
    ) -> Result<bool, SandboxStoreError> {
        let found = self
            .transitions
            .find_one(doc! {
                "_id": self.transition_id(expected.lane, expected.from_revision),
            })
            .await
            .map_err(unavailable)?;
        Ok(found.is_some_and(|row| {
            row.deployment == self.deployment
                && row.document_schema_version == SCHEMA_VERSION
                && row.transition == *expected
        }))
    }

    pub(crate) async fn finish_promotion(
        &self,
        session: &mut ClientSession,
        transition: &ReleasedTransition,
    ) -> Result<(), SandboxStoreError> {
        use super::sandbox_mongo::{commit_transaction, CommitOutcome};
        match commit_transaction(session).await {
            CommitOutcome::Committed => Ok(()),
            CommitOutcome::Retry(_) => Err(SandboxStoreError::Conflict),
            CommitOutcome::Unknown(error) | CommitOutcome::Failed(error) => {
                // A commit may be durable before the receipt becomes visible
                // to another majority read. Reconcile the same immutable
                // transition within a bounded window, including after failover.
                let visible = tokio::time::timeout(std::time::Duration::from_secs(3), async {
                    loop {
                        if self.committed(transition).await? {
                            return Ok::<_, SandboxStoreError>(());
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    }
                })
                .await;
                visible.unwrap_or_else(|_| Err(unavailable(error)))
            }
        }
    }

    /// Fence the first durable admission of new work in the caller's
    /// transaction: it commits only while the captured release is active, and
    /// it conflicts with any concurrent promotion of the lane.
    pub(crate) async fn fence_admission(
        &self,
        lane: ReleasedLane,
        image: &str,
        active_revision: u64,
        session: &mut ClientSession,
    ) -> Result<(), SandboxStoreError> {
        let result = self
            .lanes
            .update_one(
                doc! {
                    "_id": self.id(lane),
                    "document_schema_version": SCHEMA_VERSION,
                    "record.active.image": image,
                    "record.active.revision": revision(active_revision)?,
                },
                doc! {"$inc": {"record.admissions": 1_i64}},
            )
            .session(session)
            .await
            .map_err(|error| {
                if super::sandbox_mongo::is_transient(&error) {
                    SandboxStoreError::Conflict
                } else {
                    unavailable(error)
                }
            })?;
        if result.matched_count == 1 {
            Ok(())
        } else {
            Err(SandboxStoreError::Conflict)
        }
    }

    pub(crate) async fn fence_current(
        &self,
        record: &ReleasedLaneRecord,
        session: &mut ClientSession,
    ) -> Result<(), SandboxStoreError> {
        let result = self
            .lanes
            .update_one(
                doc! {
                    "_id": self.id(record.lane), "revision": revision(record.revision)?,
                    "document_schema_version": SCHEMA_VERSION, "record.frozen": false,
                },
                doc! {"$inc": {"decision_fence": 1_i64}},
            )
            .session(session)
            .await
            .map_err(unavailable)?;
        if result.matched_count == 1 {
            Ok(())
        } else {
            Err(SandboxStoreError::Conflict)
        }
    }
}

impl super::sandbox_mongo::MongoSandboxStore {
    pub(super) async fn fence_released_admission(
        &self,
        captured: &layer3_adapter::ReleasedRuntime,
        lane: ReleasedLane,
        session: &mut ClientSession,
    ) -> Result<(), SandboxStoreError> {
        let Some(lanes) = &self.released_lanes else {
            return Ok(());
        };
        lanes.fence_snapshot(captured, lane, session).await
    }
}

impl MongoReleasedRuntimes {
    pub(super) async fn fence_snapshot(
        &self,
        captured: &layer3_adapter::ReleasedRuntime,
        lane: ReleasedLane,
        session: &mut ClientSession,
    ) -> Result<(), SandboxStoreError> {
        let Some(revisions) = captured.lane_revisions else {
            // Startup must verify and bootstrap the lane before a store-backed
            // replica admits work. The unverified pin cannot race bootstrap.
            return Err(SandboxStoreError::Conflict);
        };
        let (image, revision) = match lane {
            ReleasedLane::Agent => (Some(captured.agent.as_str()), Some(revisions.agent)),
            ReleasedLane::Machine => (captured.machine.as_deref(), revisions.machine),
        };
        match (image, revision) {
            (Some(image), Some(revision)) => {
                self.fence_admission(lane, image, revision, session).await
            }
            _ => Err(SandboxStoreError::Conflict),
        }
    }
}

#[cfg(test)]
#[path = "released_runtimes_tests.rs"]
mod tests;

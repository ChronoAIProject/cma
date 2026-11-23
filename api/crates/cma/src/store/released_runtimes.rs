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

pub(crate) const COLLECTION: &str = "released_runtimes";
const SCHEMA_VERSION: i64 = 1;
/// Enough committed transitions to resolve any unknown acknowledgement.
const TRANSITION_HISTORY: usize = 16;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LaneDocument {
    #[serde(rename = "_id")]
    id: String,
    deployment: String,
    document_schema_version: i64,
    revision: i64,
    record: ReleasedLaneRecord,
    #[serde(default)]
    transitions: Vec<ReleasedTransition>,
}

#[derive(Clone)]
pub(crate) struct MongoReleasedRuntimes {
    lanes: Collection<LaneDocument>,
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
            lanes: majority_collection(database, COLLECTION),
            deployment: deployment.into(),
        }
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
            || document.transitions.len() > TRANSITION_HISTORY
        {
            return Err(SandboxStoreError::Corrupt("released runtime record".into()));
        }
        Ok(document)
    }

    /// The lane's record and its retained committed transitions.
    pub(crate) async fn load(
        &self,
        lane: ReleasedLane,
    ) -> Result<Option<(ReleasedLaneRecord, Vec<ReleasedTransition>)>, SandboxStoreError> {
        let Some(document) = self
            .lanes
            .find_one(doc! {"_id": self.id(lane)})
            .await
            .map_err(unavailable)?
        else {
            return Ok(None);
        };
        let document = self.verify(lane, document)?;
        Ok(Some((document.record, document.transitions)))
    }

    /// Create the lane from its verified first record once. A concurrent or
    /// earlier bootstrap wins, and its record is returned instead.
    pub(crate) async fn bootstrap(
        &self,
        record: ReleasedLaneRecord,
    ) -> Result<ReleasedLaneRecord, SandboxStoreError> {
        record
            .validate()
            .map_err(|_| SandboxStoreError::Corrupt("released runtime bootstrap".into()))?;
        let lane = record.lane;
        let document = LaneDocument {
            id: self.id(lane),
            deployment: self.deployment.clone(),
            document_schema_version: SCHEMA_VERSION,
            revision: revision(record.revision)?,
            record,
            transitions: Vec::new(),
        };
        match self.lanes.insert_one(&document).await {
            Ok(_) => Ok(document.record),
            Err(error) if super::sandbox_mongo::is_duplicate_key(&error) => self
                .load(lane)
                .await?
                .map(|(record, _)| record)
                .ok_or(SandboxStoreError::Conflict),
            Err(error) => Err(unavailable(error)),
        }
    }

    /// Replace `current` with `next` only if nothing changed in between. A
    /// promotion appends its transition in the same write.
    pub(crate) async fn replace(
        &self,
        current: &ReleasedLaneRecord,
        next: ReleasedLaneRecord,
        transition: Option<ReleasedTransition>,
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
        let record = mongodb::bson::serialize_to_bson(&next)
            .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
        let mut update = doc! {"$set": {"record": record, "revision": revision(next.revision)?}};
        if let Some(transition) = transition {
            let transition = mongodb::bson::serialize_to_bson(&transition)
                .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
            update.insert(
                "$push",
                doc! {"transitions": {"$each": [transition], "$slice": -(TRANSITION_HISTORY as i64)}},
            );
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
            .map_err(unavailable)?;
        if result.matched_count == 1 {
            Ok(())
        } else {
            Err(SandboxStoreError::Conflict)
        }
    }
}

#[cfg(test)]
#[path = "released_runtimes_tests.rs"]
mod tests;

//! Gateway-owned sealed record persistence. No Agent Manager collections.
use super::{RecordError, RecordHead, RecordKind, RecordValue};
use futures_util::TryStreamExt;
use mongodb::{
    bson::{doc, spec::BinarySubtype, Binary, DateTime, Document},
    options::{Acknowledgment, ReadConcern, ReturnDocument, WriteConcern},
    Collection, Database,
};
use serde::{Deserialize, Serialize};

#[path = "mongo_admission_index.rs"]
mod admission_index;
#[path = "mongo_create_fences.rs"]
mod create_fences;
#[path = "mongo_guarded_admission.rs"]
mod guarded_admission;
#[path = "mongo_replica_observation.rs"]
mod replica_observation;
#[path = "mongo_replica_retry.rs"]
mod replica_retry;
#[path = "mongo_replicas.rs"]
mod replicas;
#[path = "mongo_versions.rs"]
mod versions;
pub(crate) use create_fences::{CancelCreate, CreateConnectionClaim, CreateFenceIdentity};
#[path = "mongo_create_fences_floor.rs"]
mod create_fences_floor;
pub(crate) use create_fences_floor::inspect_migration as inspect_create_fence_floor_migration;
pub(crate) use create_fences_floor::verify as check_create_fence_floor;
pub(crate) use create_fences_floor::verify_runtime as verify_create_fence_floor;
pub use create_fences_floor::{migrate as migrate_create_fence_floor, role as create_fence_role};
pub(crate) use replicas::{
    LeaseCondition, ReplicaGuard, ReplicaKind, ReplicaProjection, ReplicaWrite,
};

pub const RECORDS: &str = "eg_channel_records";
pub const CONNECTIONS: &str = "eg_connection_records";
pub const INTERACTIONS: &str = "eg_interaction_records";
pub const THREAD_FENCES: &str = "eg_thread_fence_records";
pub const WHATSAPP_FLOWS: &str = "eg_whatsapp_flow_records";
pub const MEDIA_CONTINUATIONS: &str = "eg_media_continuation_records";
pub const MIGRATIONS: &str = "eg_schema_migrations";
pub const SESSIONS: &str = "eg_auth_sessions";
pub const COLLECTIONS: &[&str] = &[RECORDS, CONNECTIONS, INTERACTIONS, MIGRATIONS, SESSIONS];
#[derive(Clone, Debug)]
pub struct MongoRecords {
    db: Database,
    records: Collection<Record>,
    kind: RecordKind,
    worker_credentials: Option<crate::worker_credentials::WorkerCredentials>,
    #[cfg(test)]
    replica_attempt_fixture: Option<std::sync::Arc<replica_retry::deadline_tests::AttemptFixture>>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    #[serde(rename = "_id")]
    id: String,
    owner: String,
    blob: Binary,
    version: i64,
    schema_version: i32,
    #[serde(default, skip_serializing_if = "zero_guard")]
    guard_revision: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expires_at: Option<DateTime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    replica: Option<ReplicaProjection>,
}

// Unfenced records retain their wire shape under existing optional validators.
fn zero_guard(revision: &i64) -> bool {
    *revision == 0
}

impl MongoRecords {
    #[cfg(test)]
    pub(crate) fn read_failure_fixture(db: Database) -> Self {
        Self {
            records: db.collection(RECORDS),
            kind: RecordKind::Channel,
            db,
            worker_credentials: None,
            replica_attempt_fixture: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn set_worker_tracking(
        &mut self,
        tracking: crate::worker_credentials::WorkerCredentials,
    ) {
        self.worker_credentials = Some(tracking);
    }

    pub(crate) fn database(&self) -> Database {
        self.db.clone()
    }

    pub async fn open(db: Database) -> Result<Self, RecordError> {
        Self::open_with_authority(
            db,
            cma_service_auth::mongodb_authority::MongoAuthorityMode::Isolated,
        )
        .await
    }
    pub async fn open_with_authority(
        db: Database,
        authority: cma_service_auth::mongodb_authority::MongoAuthorityMode,
    ) -> Result<Self, RecordError> {
        verify_with_authority(&db, authority).await?;
        let options = mongodb::options::CollectionOptions::builder()
            .read_concern(ReadConcern::majority())
            .write_concern(WriteConcern::builder().w(Acknowledgment::Majority).build())
            .build();
        Ok(Self {
            worker_credentials: crate::worker_credentials::WorkerCredentials::from_environment(&db)
                .await?,
            records: db.collection_with_options(RECORDS, options),
            kind: RecordKind::Channel,
            db,
            #[cfg(test)]
            replica_attempt_fixture: None,
        })
    }
    pub async fn reachable(&self) -> bool {
        self.db.run_command(doc! {"ping":1}).await.is_ok()
    }
    pub fn for_kind(&self, kind: RecordKind) -> Self {
        Self {
            db: self.db.clone(),
            records: self.db.collection_with_options(
                kind.collection(),
                mongodb::options::CollectionOptions::builder()
                    .read_concern(ReadConcern::majority())
                    .write_concern(WriteConcern::builder().w(Acknowledgment::Majority).build())
                    .build(),
            ),
            kind,
            worker_credentials: self.worker_credentials.clone(),
            #[cfg(test)]
            replica_attempt_fixture: self.replica_attempt_fixture.clone(),
        }
    }
    fn live(&self, mut filter: Document) -> Document {
        if matches!(
            self.kind,
            RecordKind::Interaction | RecordKind::MediaContinuation
        ) {
            filter.insert("expires_at", doc! {"$gt":DateTime::now()});
        }
        filter
    }
    pub(crate) fn worker_credentials(
        &self,
    ) -> Option<crate::worker_credentials::WorkerCredentials> {
        self.worker_credentials.clone()
    }
    pub async fn put(
        &self,
        id: &str,
        owner: &str,
        blob: &[u8],
        expected: Option<u64>,
    ) -> Result<u64, RecordError> {
        self.put_expiring(id, owner, blob, expected, None).await
    }
    pub async fn put_expiring(
        &self,
        id: &str,
        owner: &str,
        blob: &[u8],
        expected: Option<u64>,
        expires_at: Option<i64>,
    ) -> Result<u64, RecordError> {
        self.put_with_retention(id, owner, blob, expected, expires_at, Default::default())
            .await
    }

    pub(super) async fn put_with_retention(
        &self,
        id: &str,
        owner: &str,
        blob: &[u8],
        expected: Option<u64>,
        expires_at: Option<i64>,
        retention: super::retention::InteractionRetention,
    ) -> Result<u64, RecordError> {
        if owner.is_empty() || owner.len() > 512 || blob.is_empty() || blob.len() > 65536 {
            return Err(RecordError::Rejected("invalid record".into()));
        }
        if self.kind == RecordKind::Channel {
            if let Some(tracking) = &self.worker_credentials {
                let version = expected
                    .unwrap_or(0)
                    .checked_add(1)
                    .ok_or_else(|| RecordError::Rejected("invalid record revision".into()))?;
                tracking.record_blob(id, version, blob).await?;
            }
        }
        if retention == super::retention::InteractionRetention::WhatsappReaction
            && self.kind != RecordKind::Interaction
        {
            return Err(RecordError::Rejected(
                "WhatsApp reaction retention requires Interaction records".into(),
            ));
        }
        let expiry = match (self.kind, expires_at) {
            (RecordKind::Interaction | RecordKind::MediaContinuation, Some(seconds))
                if seconds > DateTime::now().timestamp_millis() / 1000
                    && seconds <= DateTime::now().timestamp_millis() / 1000 + retention.maximum_age() =>
            {
                Some(DateTime::from_millis(seconds.checked_mul(1000).ok_or_else(|| RecordError::Rejected("invalid interaction expiry".into()))?))
            }
            (RecordKind::Channel | RecordKind::Connection | RecordKind::ThreadFence | RecordKind::WhatsappFlow, None) => None,
            _ => return Err(RecordError::Rejected("expiry exceeds the selected interaction retention class or the record may not expire".into())),
        };
        let binary = Binary {
            subtype: BinarySubtype::Generic,
            bytes: blob.to_vec(),
        };
        if let Some(version) = expected {
            let version = i64::try_from(version)
                .map_err(|_| RecordError::Rejected("invalid record revision".into()))?;
            if version < 1 || version == i64::MAX {
                return Err(RecordError::Rejected("invalid record revision".into()));
            }
            let mut filter = doc! {"_id":id,"owner":owner,"version":version};
            if let Some(expiry) = expiry {
                filter.insert("expires_at", doc! {"$gt":DateTime::now(),"$eq":expiry});
            }
            let row = self
                .records
                .find_one_and_update(
                    filter,
                    doc! {"$set":{"blob":binary},"$inc":{"version":1_i64}},
                )
                .return_document(ReturnDocument::After)
                .await
                .map_err(failure)?;
            return match row {
                Some(r) => Ok(r.version as u64),
                None => Err(self.conflict(id, expected.unwrap_or(0)).await?),
            };
        }
        let row = Record {
            id: id.into(),
            owner: owner.into(),
            blob: binary,
            version: 1,
            schema_version: 1,
            guard_revision: 0,
            replica: None,
            expires_at: expiry,
        };
        match self.records.insert_one(&row).await {
            Ok(_) => Ok(1),
            Err(error) => {
                if self
                    .records
                    .find_one(doc! {"_id":id})
                    .await
                    .map_err(failure)?
                    .is_some()
                {
                    Err(self.conflict(id, 0).await?)
                } else {
                    Err(failure(error))
                }
            }
        }
    }
    pub async fn get(&self, id: &str) -> Result<RecordValue, RecordError> {
        let row = self
            .records
            .find_one(self.live(doc! {"_id":id}))
            .await
            .map_err(failure)?
            .ok_or(RecordError::NotFound)?;
        Ok(RecordValue {
            blob: row.blob.bytes,
            version: row.version as u64,
        })
    }
    pub async fn delete(&self, id: &str, expected: u64) -> Result<bool, RecordError> {
        if self.kind == RecordKind::ThreadFence {
            return Err(RecordError::Rejected(
                "persistent thread fences cannot be deleted".into(),
            ));
        }
        let version =
            i64::try_from(expected).map_err(|_| RecordError::Rejected("invalid version".into()))?;
        if self
            .records
            .delete_one(self.live(doc! {"_id":id,"version":version}))
            .await
            .map_err(failure)?
            .deleted_count
            == 1
        {
            return Ok(true);
        }
        if self
            .records
            .find_one(self.live(doc! {"_id":id}))
            .await
            .map_err(failure)?
            .is_none()
        {
            return Ok(false);
        }
        Err(self.conflict(id, expected).await?)
    }
    pub async fn list(
        &self,
        owner: Option<&str>,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<RecordHead>, RecordError> {
        let maximum = if self.kind == RecordKind::MediaContinuation {
            32
        } else {
            1000
        };
        if !(1..=maximum).contains(&limit) {
            return Err(RecordError::Rejected("invalid page size".into()));
        }
        let mut filter = self.live(doc! {});
        if let Some(owner) = owner {
            filter.insert("owner", owner);
        }
        if let Some(after) = after {
            filter.insert("_id", doc! {"$gt":after});
        }
        if self.kind == RecordKind::MediaContinuation {
            return self
                .media_continuation_heads(filter, owner.is_some(), limit)
                .await;
        }
        let rows: Vec<Record> = self
            .records
            .find(filter)
            .sort(doc! {"_id":1})
            .limit(limit as i64)
            .await
            .map_err(failure)?
            .try_collect()
            .await
            .map_err(failure)?;
        Ok(rows
            .into_iter()
            .map(|r| RecordHead {
                record_id: r.id,
                owner: r.owner,
                version: r.version as u64,
            })
            .collect())
    }
    async fn conflict(&self, id: &str, expected: u64) -> Result<RecordError, RecordError> {
        let current = self
            .records
            .find_one(doc! {"_id":id})
            .await
            .map_err(failure)?;
        Ok(RecordError::Conflict {
            expected,
            actual: current.map(|r| r.version as u64),
        })
    }
}
fn failure(_: mongodb::error::Error) -> RecordError {
    RecordError::Unavailable("gateway database operation failed".into())
}

#[path = "mongo_schema.rs"]
mod schema;
pub use schema::{guard_migration_required, migrate, role, verify, verify_with_authority};
pub(crate) use schema::{migration_predecessors, migration_validator};
#[path = "mongo_replica_schema.rs"]
mod replica_schema;
pub use replica_schema::migrate as migrate_replicas;
pub(crate) use replica_schema::verify as verify_replicas;

#[path = "mongo_thread_fences.rs"]
mod thread_fences;
pub use thread_fences::{migrate as migrate_thread_fences, role as thread_fences_role};
#[path = "mongo_whatsapp_flows.rs"]
mod whatsapp_flows;
pub use whatsapp_flows::{migrate as migrate_whatsapp_flows, role as whatsapp_flows_role};
#[path = "mongo_media_continuations.rs"]
mod media_continuations;
pub use media_continuations::{
    migrate as migrate_media_continuations, role as media_continuations_role,
};

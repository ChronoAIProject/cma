//! Durable Agent lifecycle operation records.
//!
//! The lifecycle worker deliberately keeps its request, phase gates and
//! outcome in Mongo.  A process restart can therefore reclaim an unfinished
//! operation without replaying the HTTP request or losing the user-visible
//! progress history.

use std::time::{Duration, SystemTime};

use futures_util::TryStreamExt;
use layer1_public::rest::{
    AgentLifecycleConnectionRequired, AgentLifecycleCreateRequest, AgentLifecycleEventView,
    AgentLifecycleOperationView,
};
use layer2_core::{now_rfc3339, Capability, StoreError, SubjectId};
use mongodb::{
    bson::{doc, spec::BinarySubtype, Binary, Bson, DateTime},
    error::{Error, ErrorKind, WriteFailure},
    options::{IndexOptions, ReturnDocument},
    Collection, Database, IndexModel,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{durability::majority_collection, schema, schema::DOCUMENT_SCHEMA_VERSION};

pub(crate) const COLLECTION: &str = "agent_lifecycle_operations";
const OWNER_KEY_INDEX: &str = "agent_lifecycle_owner_key";
// Renewable claims are fenced by a fresh token on every acquisition. A
// crashed worker becomes eligible for periodic discovery after this bound.
const LEASE: Duration = Duration::from_secs(90);
pub(crate) const HEARTBEAT: Duration = Duration::from_secs(20);
pub(crate) const DISCOVERY_BATCH: usize = 16;
const MAX_EVENTS: i32 = 32;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LifecycleBehavior {
    #[default]
    Legacy,
    PromptRepository,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct LifecycleInput {
    /// Replaying an admitted operation must preserve its original profile and setup.
    #[serde(default)]
    pub behavior: LifecycleBehavior,
    pub request: AgentLifecycleCreateRequest,
    pub resolved_image: Option<String>,
    /// New admissions pin deployment capacity; legacy records used policy minima.
    #[serde(default)]
    pub capacity: Option<layer3_adapter::SandboxCapacity>,
    #[serde(default)]
    pub capabilities: Vec<Capability>,
    /// Original authenticated actor; old records retain unknown attribution.
    #[serde(default)]
    pub actor: Option<layer2_core::resource_access::ResourceActor>,
}

impl LifecycleInput {
    pub(crate) fn captured_limits(&self) -> layer2_core::sandbox::SandboxLimits {
        // Capacity was introduced with expandable inline Sandboxes. Historical
        // admissions must recreate their original immutable one-slot profile.
        let maximum = if self.capacity.is_some() { 16 } else { 1 };
        layer2_core::sandbox::SandboxLimits {
            max_workspaces: maximum,
            max_agents_per_workspace: maximum,
        }
    }

    pub(crate) fn captured_capacity(
        &self,
        bounds: layer2_core::profiles::ProfileCapacityBounds,
    ) -> layer3_adapter::SandboxCapacity {
        self.capacity.unwrap_or(layer3_adapter::SandboxCapacity {
            cpu_millis: bounds.min_cpu_millis,
            memory_mib: bounds.min_memory_mib,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct LifecycleEvent {
    pub sequence: u64,
    pub phase: String,
    pub status: String,
    pub message: String,
    pub occurred_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct LifecycleRecord {
    #[serde(rename = "_id")]
    pub id: String,
    pub owner: String,
    pub key: String,
    pub fingerprint: Binary,
    pub input: LifecycleInput,
    pub status: String,
    pub phase: String,
    pub events: Vec<LifecycleEvent>,
    #[serde(default)]
    pub sandbox_id: Option<String>,
    pub agent_id: Option<String>,
    pub error: Option<String>,
    #[serde(default)]
    pub connection: Option<AgentLifecycleConnectionRequired>,
    pub lease_expires_at: Option<DateTime>,
    #[serde(default)]
    pub lease_token: Option<String>,
    #[serde(default)]
    pub attempts: u32,
    #[serde(default)]
    pub retry_at: Option<DateTime>,
    pub created_at: DateTime,
    pub updated_at: DateTime,
    pub document_schema_version: i64,
}

#[derive(Debug, Clone)]
pub(crate) enum CreateResult {
    Created(String),
    Existing(Box<LifecycleRecord>),
}

#[derive(Debug, Clone)]
pub(crate) struct LifecycleLease {
    pub id: String,
    pub token: String,
}

#[derive(Clone)]
pub(crate) struct MongoAgentLifecycleStore {
    operations: Collection<LifecycleRecord>,
}

impl MongoAgentLifecycleStore {
    pub(crate) async fn open(database: &Database) -> Result<Self, StoreError> {
        schema::initialize_collections(database, &[COLLECTION]).await?;
        let operations = majority_collection(database, COLLECTION);
        operations
            .create_index(
                IndexModel::builder()
                    .keys(doc! { "owner": 1, "key": 1 })
                    .options(
                        IndexOptions::builder()
                            .name(OWNER_KEY_INDEX.to_owned())
                            .unique(true)
                            .build(),
                    )
                    .build(),
            )
            .await
            .map_err(|error| {
                StoreError::Unavailable(format!("lifecycle operation index: {error}"))
            })?;
        operations
            .create_index(
                IndexModel::builder()
                    .keys(doc! { "status": 1, "lease_expires_at": 1, "created_at": 1 })
                    .build(),
            )
            .await
            .map_err(driver)?;
        operations
            .create_index(
                IndexModel::builder()
                    .keys(doc! { "status": 1, "retry_at": 1, "created_at": 1 })
                    .build(),
            )
            .await
            .map_err(driver)?;
        Ok(Self { operations })
    }

    pub(crate) async fn create(
        &self,
        owner: &SubjectId,
        key: &str,
        input: LifecycleInput,
    ) -> Result<CreateResult, StoreError> {
        let bytes = serde_json::to_vec(&input.request)
            .map_err(|error| StoreError::Malformed(format!("encode lifecycle input: {error}")))?;
        let fingerprint = Binary {
            subtype: BinarySubtype::Generic,
            bytes: Sha256::digest(&bytes).to_vec(),
        };
        if let Some(existing) = self
            .operations
            .find_one(doc! { "owner": owner.as_str(), "key": key })
            .await
            .map_err(driver)?
        {
            schema::require_current(existing.document_schema_version)?;
            if !same_request(&existing.input.request, &input.request)? {
                return Err(StoreError::RevisionConflict {
                    expected: 0,
                    actual: 1,
                });
            }
            return Ok(CreateResult::Existing(Box::new(existing)));
        }
        let now = DateTime::now();
        let record = LifecycleRecord {
            id: uuid::Uuid::new_v4().simple().to_string(),
            owner: owner.to_string(),
            key: key.to_owned(),
            fingerprint,
            input,
            status: "queued".into(),
            phase: "queued".into(),
            events: Vec::new(),
            sandbox_id: None,
            agent_id: None,
            error: None,
            connection: None,
            lease_expires_at: None,
            lease_token: None,
            attempts: 0,
            retry_at: None,
            created_at: now,
            updated_at: now,
            document_schema_version: DOCUMENT_SCHEMA_VERSION,
        };
        match self.operations.insert_one(&record).await {
            Ok(_) => Ok(CreateResult::Created(record.id)),
            Err(error) if is_duplicate_key(&error) => {
                let existing = self
                    .operations
                    .find_one(doc! { "owner": owner.as_str(), "key": key })
                    .await
                    .map_err(driver)?
                    .ok_or_else(|| {
                        StoreError::Unavailable("lifecycle operation raced out of view".into())
                    })?;
                schema::require_current(existing.document_schema_version)?;
                if !same_request(&existing.input.request, &record.input.request)? {
                    return Err(StoreError::RevisionConflict {
                        expected: 0,
                        actual: 1,
                    });
                }
                Ok(CreateResult::Existing(Box::new(existing)))
            }
            Err(error) => Err(driver(error)),
        }
    }

    /// Claim queued work or an expired worker, including legacy records whose
    /// phase-ready event incorrectly overwrote the global running status.
    pub(crate) async fn claim(&self, id: &str) -> Result<Option<LifecycleRecord>, StoreError> {
        let now = SystemTime::now();
        let deadline = now
            .checked_add(LEASE)
            .ok_or_else(|| StoreError::Malformed("lifecycle lease overflowed".into()))?;
        let filter = doc! {
            "_id": id,
            "document_schema_version": DOCUMENT_SCHEMA_VERSION,
            "$or": [
                { "status": "queued", "$or": [
                    { "retry_at": Bson::Null },
                    { "retry_at": { "$lte": DateTime::from_system_time(now) } },
                ] },
                { "status": { "$in": ["running", "ready"] }, "$or": [
                    { "lease_expires_at": { "$exists": false } },
                    { "lease_expires_at": Bson::Null },
                    { "lease_expires_at": { "$lte": DateTime::from_system_time(now) } },
                ] },
            ],
        };
        let update = doc! {
            "$set": {
                "status": "running",
                "updated_at": DateTime::now(),
                "lease_expires_at": DateTime::from_system_time(deadline),
                "lease_token": uuid::Uuid::new_v4().to_string(),
                "retry_at": Bson::Null,
            },
            "$inc": { "attempts": 1 },
        };
        self.operations
            .find_one_and_update(filter, update)
            .return_document(ReturnDocument::After)
            .await
            .map_err(driver)
    }

    pub(crate) async fn pending(&self) -> Result<Vec<String>, StoreError> {
        let now = DateTime::from_system_time(SystemTime::now());
        let mut cursor = self
            .operations
            .find(doc! {
                "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                "$or": [
                    { "status": "queued", "$or": [
                        { "retry_at": Bson::Null },
                        { "retry_at": { "$lte": now } },
                    ] },
                    { "status": { "$in": ["running", "ready"] }, "$or": [
                        { "lease_expires_at": { "$exists": false } },
                        { "lease_expires_at": Bson::Null },
                        { "lease_expires_at": { "$lte": now } },
                    ] },
                ],
            })
            .sort(doc! { "created_at": 1, "_id": 1 })
            .limit(DISCOVERY_BATCH as i64)
            .await
            .map_err(driver)?;
        let mut ids = Vec::new();
        while let Some(record) = cursor.try_next().await.map_err(driver)? {
            ids.push(record.id);
        }
        Ok(ids)
    }

    /// Replays compare only caller input; resolved latest image and captured
    /// capabilities are immutable admission data, not a second request.
    pub(crate) async fn replay(
        &self,
        owner: &SubjectId,
        key: &str,
        request: &AgentLifecycleCreateRequest,
    ) -> Result<Option<LifecycleRecord>, StoreError> {
        let existing = self
            .operations
            .find_one(doc! { "owner": owner.as_str(), "key": key })
            .await
            .map_err(driver)?;
        if let Some(record) = &existing {
            schema::require_current(record.document_schema_version)?;
            if !same_request(&record.input.request, request)? {
                return Err(StoreError::RevisionConflict {
                    expected: 0,
                    actual: 1,
                });
            }
        }
        Ok(existing)
    }

    pub(crate) async fn read_for_owner(
        &self,
        owner: &SubjectId,
        id: &str,
    ) -> Result<Option<LifecycleRecord>, StoreError> {
        self.operations
            .find_one(doc! { "_id": id })
            .await
            .map_err(driver)
            .and_then(|record| {
                record
                    .map(|record| {
                        schema::require_current(record.document_schema_version)?;
                        Ok(record)
                    })
                    .transpose()
            })
            .map(|record| record.filter(|record| record.owner == owner.as_str()))
    }
}

#[path = "agent_lifecycle_writes.rs"]
mod writes;

#[cfg(test)]
#[path = "agent_lifecycle_progress_tests.rs"]
mod progress_tests;

fn same_request(
    left: &AgentLifecycleCreateRequest,
    right: &AgentLifecycleCreateRequest,
) -> Result<bool, StoreError> {
    let encode = |value| {
        serde_json::to_value(value)
            .map_err(|error| StoreError::Malformed(format!("encode lifecycle input: {error}")))
    };
    Ok(encode(left)? == encode(right)?)
}

impl LifecycleRecord {
    pub(crate) fn lease(&self) -> Result<LifecycleLease, StoreError> {
        Ok(LifecycleLease {
            id: self.id.clone(),
            token: self.lease_token.clone().ok_or_else(|| {
                StoreError::Malformed("claimed lifecycle has no lease token".into())
            })?,
        })
    }

    pub(crate) fn view(&self) -> AgentLifecycleOperationView {
        AgentLifecycleOperationView {
            id: self.id.clone(),
            status: self.status.clone(),
            phase: self.phase.clone(),
            agent_id: self.agent_id.clone(),
            error: self.error.clone(),
            connection: self.connection.clone(),
            events: self
                .events
                .iter()
                .map(|event| AgentLifecycleEventView {
                    sequence: event.sequence,
                    phase: event.phase.clone(),
                    status: event.status.clone(),
                    message: event.message.clone(),
                    occurred_at: event.occurred_at.clone(),
                })
                .collect(),
        }
    }
}

fn driver(error: Error) -> StoreError {
    StoreError::Unavailable(format!("Agent lifecycle operation store: {error}"))
}

fn is_duplicate_key(error: &Error) -> bool {
    matches!(
        error.kind.as_ref(),
        ErrorKind::Write(WriteFailure::WriteError(write)) if write.code == 11_000
    )
}

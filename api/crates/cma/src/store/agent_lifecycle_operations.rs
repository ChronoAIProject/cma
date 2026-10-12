//! Durable Agent lifecycle operation records.
//!
//! The lifecycle worker deliberately keeps its request, phase gates and
//! outcome in Mongo.  A process restart can therefore reclaim an unfinished
//! operation without replaying the HTTP request or losing the user-visible
//! progress history.

use std::time::{Duration, SystemTime};

use futures_util::TryStreamExt;
use layer1_public::rest::{
    AgentLifecycleCreateRequest, AgentLifecycleEventView, AgentLifecycleOperationView,
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
// The worker waits up to three minutes for a runtime workspace. Keep the
// lease comfortably beyond that bound so a healthy slow worker is never
// duplicated by another replica; a crashed process is reclaimed on restart.
const LEASE: Duration = Duration::from_secs(600);
const MAX_EVENTS: i32 = 32;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct LifecycleInput {
    pub request: AgentLifecycleCreateRequest,
    pub resolved_image: Option<String>,
    #[serde(default)]
    pub capabilities: Vec<Capability>,
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
    pub lease_expires_at: Option<DateTime>,
    pub created_at: DateTime,
    pub updated_at: DateTime,
    pub document_schema_version: i64,
}

#[derive(Debug, Clone)]
pub(crate) enum CreateResult {
    Created(String),
    Existing(Box<LifecycleRecord>),
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
        Ok(Self { operations })
    }

    pub(crate) async fn create(
        &self,
        owner: &SubjectId,
        key: &str,
        input: LifecycleInput,
    ) -> Result<CreateResult, StoreError> {
        let bytes = serde_json::to_vec(&input)
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
            if existing.fingerprint.bytes != fingerprint.bytes {
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
            lease_expires_at: None,
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
                if existing.fingerprint.bytes != record.fingerprint.bytes {
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

    /// Claim a queued operation, or reclaim one whose worker lease expired.
    pub(crate) async fn claim(&self, id: &str) -> Result<Option<LifecycleRecord>, StoreError> {
        let now = SystemTime::now();
        let deadline = now
            .checked_add(LEASE)
            .ok_or_else(|| StoreError::Malformed("lifecycle lease overflowed".into()))?;
        let filter = doc! {
            "_id": id,
            "document_schema_version": DOCUMENT_SCHEMA_VERSION,
            "$or": [
                { "status": "queued" },
                { "status": "running", "$or": [
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
            }
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
                    { "status": "queued" },
                    { "status": "running", "$or": [
                        { "lease_expires_at": { "$exists": false } },
                        { "lease_expires_at": Bson::Null },
                        { "lease_expires_at": { "$lte": now } },
                    ] },
                ],
            })
            .await
            .map_err(driver)?;
        let mut ids = Vec::new();
        while let Some(record) = cursor.try_next().await.map_err(driver)? {
            ids.push(record.id);
        }
        Ok(ids)
    }

    pub(crate) async fn append_event(
        &self,
        id: &str,
        sequence: u64,
        phase: &str,
        status: &str,
        message: &str,
    ) -> Result<(), StoreError> {
        let event = LifecycleEvent {
            sequence,
            phase: phase.to_owned(),
            status: status.to_owned(),
            message: message.to_owned(),
            occurred_at: now_rfc3339(),
        };
        self.operations
            .update_one(
                doc! { "_id": id, "document_schema_version": DOCUMENT_SCHEMA_VERSION },
                doc! {
                    "$set": {
                        "phase": phase,
                        "status": status,
                        "updated_at": DateTime::now(),
                    },
                    "$push": {
                        "events": { "$each": [mongodb::bson::serialize_to_bson(&event).map_err(|error| StoreError::Malformed(error.to_string()))?], "$slice": -MAX_EVENTS },
                    },
                },
            )
            .await
            .map_err(driver)?;
        Ok(())
    }

    pub(crate) async fn succeed(&self, id: &str, agent_id: &str) -> Result<(), StoreError> {
        self.operations
            .update_one(
                doc! { "_id": id, "document_schema_version": DOCUMENT_SCHEMA_VERSION },
                doc! {
                    "$set": {
                        "status": "succeeded",
                        "phase": "key_scope_ready",
                        "agent_id": agent_id,
                        "lease_expires_at": Bson::Null,
                        "updated_at": DateTime::now(),
                    }
                },
            )
            .await
            .map_err(driver)?;
        Ok(())
    }

    pub(crate) async fn mark_sandbox(&self, id: &str, sandbox_id: &str) -> Result<(), StoreError> {
        self.operations
            .update_one(
                doc! { "_id": id, "document_schema_version": DOCUMENT_SCHEMA_VERSION },
                doc! { "$set": { "sandbox_id": sandbox_id, "updated_at": DateTime::now() } },
            )
            .await
            .map_err(driver)?;
        Ok(())
    }

    pub(crate) async fn mark_agent(&self, id: &str, agent_id: &str) -> Result<(), StoreError> {
        self.operations
            .update_one(
                doc! { "_id": id, "document_schema_version": DOCUMENT_SCHEMA_VERSION },
                doc! { "$set": { "agent_id": agent_id, "updated_at": DateTime::now() } },
            )
            .await
            .map_err(driver)?;
        Ok(())
    }

    pub(crate) async fn fail(&self, id: &str, error: &str) -> Result<(), StoreError> {
        self.operations
            .update_one(
                doc! { "_id": id, "document_schema_version": DOCUMENT_SCHEMA_VERSION },
                doc! {
                    "$set": {
                        "status": "failed",
                        "phase": "failed",
                        "error": error,
                        "lease_expires_at": Bson::Null,
                        "updated_at": DateTime::now(),
                    }
                },
            )
            .await
            .map_err(driver)?;
        Ok(())
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

impl LifecycleRecord {
    pub(crate) fn view(&self) -> AgentLifecycleOperationView {
        AgentLifecycleOperationView {
            id: self.id.clone(),
            status: self.status.clone(),
            phase: self.phase.clone(),
            agent_id: self.agent_id.clone(),
            error: self.error.clone(),
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

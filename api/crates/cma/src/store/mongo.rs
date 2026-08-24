//! Session records and event history, in MongoDB.
//!
//! Sessions, events, and whole-write receipts live in three collections.
//! Events carry a compound unique index on `(session_id, seq)`, which is what
//! makes the per-session sequence a guarantee rather than a hope: a duplicate
//! sequence is rejected by the database, not merely unlikely.

use std::{
    collections::{BTreeMap, HashSet},
    future::Future,
    time::Duration,
};

use futures_util::{FutureExt, TryStreamExt};
use layer2_core::{
    store::{SessionRecord, SessionStore, StoreError},
    Event, Rate, RateCardId, Reserved, SessionId, SessionPricing, SessionStatus, StoredEvent,
    SubjectId,
};
use mongodb::bson::{doc, Bson, Document};
use mongodb::options::{IndexOptions, ReadConcern, ReplaceOptions, WriteConcern};
use mongodb::results::{CollectionSpecification, CollectionType};
use mongodb::{Client, Collection, Database, IndexModel};
use serde::{Deserialize, Serialize};

use super::event_doc;

const LEGACY_SESSIONS: &str = "sessions";
const LEGACY_EVENTS: &str = "session_events";
const SESSIONS: &str = "sessions_v2";
const EVENTS: &str = "session_events_v2";
const STORE_PING_TIMEOUT: Duration = Duration::from_secs(2);
const STORE_OPEN_TIMEOUT: Duration = Duration::from_secs(5);
const WRITES: &str = "session_writes";
const TOMBSTONES: &str = "session_tombstones";
pub(super) const WRITE_PROTOCOL: i32 = 1;
const WRITE_PROTOCOL_FIELD: &str = "_cma_write_protocol";

/// A granted skill as it is stored.
///
/// Two shapes, because records written before versions were carried hold a
/// bare string. Reading accepts either; writing always produces the object, so
/// the old shape disappears as records are rewritten rather than needing a
/// migration nobody would run.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
enum StoredSkill {
    Versioned {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<String>,
    },
    /// Written before this service recorded versions.
    Bare(String),
}

/// A pinned session price in BSON-compatible signed integers.
///
/// BSON has no unsigned 64-bit integer, so this mirror is explicit and every
/// conversion is checked. A price that cannot be represented is refused
/// rather than clamped into a different charge.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PricingDoc {
    rate_card: String,
    cpu_micros_per_cpu_hour: i64,
    memory_micros_per_gib_hour: i64,
    cpu_millis: i64,
    memory_mib: i64,
    micros_per_hour: i64,
    quoted_at: String,
}

/// The one document produced by the deployment-reservation aggregation.
///
/// MongoDB sums into signed BSON integers. Conversion to the core's unsigned
/// ledger is checked below rather than letting malformed negative data wrap.
#[derive(Debug, Deserialize)]
struct ReservedCapacityDoc {
    sessions: i64,
    unsized_sessions: i64,
    cpu_millis: i64,
    memory_mib: i64,
}

#[derive(Debug, Deserialize)]
struct SessionCountDoc {
    count: i64,
}

impl StoredSkill {
    fn into_granted(self) -> layer2_core::GrantedSkill {
        match self {
            Self::Versioned { id, version } => layer2_core::GrantedSkill {
                id: layer2_core::SkillId::new(id),
                version,
            },
            // Not "version unknown to the registry" but "this record predates
            // us keeping it". Both render the same way, and neither invents one.
            Self::Bare(id) => layer2_core::GrantedSkill {
                id: layer2_core::SkillId::new(id),
                version: None,
            },
        }
    }
}

/// How a session record is shaped on the wire.
///
/// A distinct type from the core's record on purpose: the core owns its model,
/// and this owns how it is written. `_id` carries the session id so the
/// database enforces uniqueness rather than the application.

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionDoc {
    #[serde(rename = "_id")]
    id: String,
    /// Absent on documents from before the versioned writer fence.
    #[serde(rename = "_cma_write_protocol", default)]
    write_protocol: i32,
    owner: String,
    kernel: String,
    status: String,
    recovery_state: String,
    stop_reason: Option<String>,
    columns: u32,
    rows: u32,
    last_seq: i64,
    created_at: String,
    /// The agent configuration, stored as the core serialises it.
    ///
    /// Optional because records written before options were persisted have no
    /// such field; those load on defaults rather than failing to load at all.
    #[serde(default)]
    options: Option<mongodb::bson::Bson>,
    /// The environment bundle the session was started with, if any.
    #[serde(default)]
    environment_profile: Option<String>,
    runtime_id: Option<String>,
    context_ref: Option<String>,
    /// Runtime version, rotation count and snapshot time.
    ///
    /// All optional so documents written before rotation existed still load. A
    /// session with no recorded version is treated as being on an older one,
    /// which is what brings it onto the current image.
    #[serde(default)]
    runtime_version: Option<String>,
    #[serde(default)]
    rotations: Option<i64>,
    #[serde(default)]
    rotated_at: Option<String>,
    /// What the session was granted when it was created.
    ///
    /// Optional because records written before capabilities existed have no
    /// such field; those load claiming nothing, which is settled the next time
    /// the session is placed.
    #[serde(default)]
    capabilities: Option<mongodb::bson::Bson>,
    /// What the session's credential may reach, decided when it was created.
    ///
    /// Optional because records written before the grant was recorded have no
    /// such field, and the absence is a different fact from an empty list: one
    /// says the grant was never written down and falls back to the profile,
    /// the other says the session may reach nothing.
    #[serde(default)]
    granted_services: Option<Vec<String>>,
    /// The skills the session was given. Same distinction as above: absent
    /// means the grant predates the field, empty means it was given none.
    /// Each granted skill and the version that arrived.
    ///
    /// Records written before versions were carried hold bare strings, so the
    /// read accepts either shape and treats a bare string as a skill whose
    /// version was never recorded. Absent still means the grant itself was
    /// never recorded, which is a third thing again.
    #[serde(default)]
    granted_skills: Option<Vec<StoredSkill>>,
    /// What this session was priced at. Absent means it was never priced,
    /// which is a different fact from a price of nothing.
    #[serde(default)]
    pricing: Option<PricingDoc>,
    /// Optimistic store revision. Documents from before revisions read as zero.
    #[serde(default)]
    revision: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionWriteDoc {
    #[serde(rename = "_id")]
    id: String,
    #[serde(rename = "_cma_write_protocol", default)]
    write_protocol: i32,
    session_id: String,
    revision: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionTombstoneDoc {
    #[serde(rename = "_id")]
    id: String,
    #[serde(rename = "_cma_write_protocol")]
    write_protocol: i32,
}

/// A MongoDB-backed session store.
///
/// Events are held as raw documents rather than as a typed collection: the
/// driver would otherwise decide for itself what an unreadable document means,
/// and telling a newer vocabulary apart from real corruption is exactly the
/// decision `event_doc` exists to make.
#[derive(Clone)]
pub struct MongoSessionStore {
    client: Client,
    database: Database,
    sessions: Collection<SessionDoc>,
    events: Collection<Document>,
    writes: Collection<SessionWriteDoc>,
    tombstones: Collection<SessionTombstoneDoc>,
}

struct EventTransaction {
    events_collection: Collection<Document>,
    sessions_collection: Collection<SessionDoc>,
    writes_collection: Collection<SessionWriteDoc>,
    tombstones_collection: Collection<SessionTombstoneDoc>,
    id: SessionId,
    write_id: String,
    create: bool,
    events: Vec<Event>,
    documents: Vec<Document>,
    record: SessionDoc,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum NamespaceKind {
    Collection,
    View {
        source: Option<String>,
        pipeline_is_empty: bool,
    },
    Unsupported,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct NamespacePair {
    legacy: Option<NamespaceKind>,
    authoritative: Option<NamespaceKind>,
}

#[derive(Debug, Default)]
struct StoredEventSequence {
    session_id: Option<String>,
    next: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NamespaceAction {
    CreateAuthoritative,
    RenameLegacy,
    CreateLegacyView,
    Ready,
}

impl MongoSessionStore {
    /// Connect and ensure the indexes the guarantees depend on exist.
    /// Open the store over an existing connection, ensuring its indexes exist.
    ///
    /// Takes a client rather than a URI so one connection serves every store
    /// this deployment keeps in the same database. See `super::connect`.
    pub(super) async fn open(client: &Client, database: &str) -> Result<Self, StoreError> {
        let db = client.database(database);
        let store = Self {
            client: client.clone(),
            sessions: db.collection(SESSIONS),
            events: db.collection(EVENTS),
            writes: db.collection(WRITES),
            tombstones: db.collection(TOMBSTONES),
            database: db,
        };
        store
            .validate_transaction_topology(STORE_OPEN_TIMEOUT)
            .await?;
        // Fence the old session writer first. Its write and delete paths both
        // touch the session collection before they touch event history, so a
        // read-only session view stops a new old operation at its first step.
        store
            .ensure_versioned_namespace(LEGACY_SESSIONS, SESSIONS)
            .await?;
        store
            .ensure_versioned_namespace(LEGACY_EVENTS, EVENTS)
            .await?;
        store.ensure_indexes().await?;
        store.ensure_tombstone_collection().await?;
        store.install_protocol_validators().await?;
        store.validate_post_fence_state().await?;
        tracing::info!(database, "session store ready");
        Ok(store)
    }

    async fn validate_transaction_topology(&self, timeout: Duration) -> Result<(), StoreError> {
        let database = self.database.clone();
        let hello = bounded_mongo_command(
            "MongoDB topology hello",
            async move { database.run_command(doc! { "hello": 1_i32 }).await },
            timeout,
        )
        .await?;
        let reply = match hello {
            Ok(reply) => reply,
            Err(error) if command_error_code(&error) == Some(59) => {
                // `hello` replaced `isMaster` after transactions already
                // existed. Keep older transaction-capable replica sets usable.
                let database = self.database.clone();
                bounded_mongo_command(
                    "MongoDB topology isMaster",
                    async move { database.run_command(doc! { "isMaster": 1_i32 }).await },
                    timeout,
                )
                .await?
                .map_err(|error| {
                    StoreError::Unavailable(format!("MongoDB topology isMaster: {error}"))
                })?
            }
            Err(error) => {
                return Err(StoreError::Unavailable(format!(
                    "MongoDB topology hello: {error}"
                )))
            }
        };
        validate_transaction_topology_reply(&reply)
    }

    async fn verify_versioned_namespaces(&self) -> Result<(), StoreError> {
        for (legacy, authoritative) in [(LEGACY_SESSIONS, SESSIONS), (LEGACY_EVENTS, EVENTS)] {
            let pair = self.namespace_pair(legacy, authoritative).await?;
            if next_namespace_action(&pair, legacy, authoritative)? != NamespaceAction::Ready {
                return Err(StoreError::Unavailable(format!(
                    "MongoDB legacy namespace {legacy} is no longer a read-only view of {authoritative}"
                )));
            }
        }
        Ok(())
    }

    async fn ensure_versioned_namespace(
        &self,
        legacy: &str,
        authoritative: &str,
    ) -> Result<(), StoreError> {
        // Another replica may be performing the same cutover, and a process may
        // have stopped after the rename but before creating the view. Re-read
        // after every action; never infer that an ambiguous NamespaceExists
        // means the two namespaces contain the same data.
        for _ in 0..12 {
            let before = self.namespace_pair(legacy, authoritative).await?;
            let action = next_namespace_action(&before, legacy, authoritative)?;
            if action == NamespaceAction::Ready {
                return Ok(());
            }

            if let Err(error) = self
                .execute_namespace_action(action, legacy, authoritative)
                .await
            {
                let after = self.namespace_pair(legacy, authoritative).await?;
                let after_action = next_namespace_action(&after, legacy, authoritative)?;
                if after != before || after_action == NamespaceAction::Ready {
                    continue;
                }
                return Err(StoreError::Unavailable(format!(
                    "MongoDB namespace cutover {legacy} -> {authoritative} requires createCollection, renameCollectionSameDB, listCollections, find, and insert privileges: {error}"
                )));
            }
        }
        Err(StoreError::Unavailable(format!(
            "MongoDB namespace cutover {legacy} -> {authoritative} did not reach a stable state"
        )))
    }

    async fn namespace_pair(
        &self,
        legacy: &str,
        authoritative: &str,
    ) -> Result<NamespacePair, StoreError> {
        let mut cursor = self
            .database
            .list_collections()
            .filter(doc! { "name": { "$in": [legacy, authoritative] } })
            .await
            .map_err(|error| {
                StoreError::Unavailable(format!("list MongoDB session namespaces: {error}"))
            })?;
        let mut pair = NamespacePair::default();
        while let Some(specification) = cursor.try_next().await.map_err(|error| {
            StoreError::Unavailable(format!("MongoDB namespace cursor: {error}"))
        })? {
            let kind = namespace_kind(&specification);
            if specification.name == legacy {
                pair.legacy = Some(kind);
            } else if specification.name == authoritative {
                pair.authoritative = Some(kind);
            }
        }
        Ok(pair)
    }

    async fn execute_namespace_action(
        &self,
        action: NamespaceAction,
        legacy: &str,
        authoritative: &str,
    ) -> mongodb::error::Result<()> {
        match action {
            NamespaceAction::CreateAuthoritative => {
                self.database.create_collection(authoritative).await
            }
            NamespaceAction::RenameLegacy => {
                let source = format!("{}.{}", self.database.name(), legacy);
                let target = format!("{}.{}", self.database.name(), authoritative);
                self.client
                    .database("admin")
                    .run_command(doc! {
                        "renameCollection": source,
                        "to": target,
                        "dropTarget": false,
                    })
                    .await
                    .map(|_| ())
            }
            NamespaceAction::CreateLegacyView => {
                self.database
                    .create_collection(legacy)
                    .view_on(authoritative)
                    .pipeline(Vec::<Document>::new())
                    .await
            }
            NamespaceAction::Ready => Ok(()),
        }
    }

    async fn ensure_indexes(&self) -> Result<(), StoreError> {
        // Unique per (session, seq): the database is what makes the per-session
        // monotonic sequence a guarantee. Without it a racing append could
        // duplicate a cursor position and a resuming client would silently
        // skip or repeat history.
        let seq = IndexModel::builder()
            .keys(doc! { "session_id": 1, "seq": 1 })
            .options(IndexOptions::builder().unique(true).build())
            .build();
        self.events
            .create_index(seq)
            .await
            .map_err(|e| StoreError::Unavailable(format!("event index: {e}")))?;

        // A receipt is inserted atomically with every record/event unit. Its
        // `_id` is already unique; this index makes deleting one session's
        // receipts bounded when the owner deletes the session.
        self.writes
            .create_index(IndexModel::builder().keys(doc! { "session_id": 1 }).build())
            .await
            .map_err(|e| StoreError::Unavailable(format!("session write index: {e}")))?;

        let owner = IndexModel::builder()
            .keys(doc! { "owner": 1, "created_at": -1 })
            .build();
        self.sessions
            .create_index(owner)
            .await
            .map_err(|e| StoreError::Unavailable(format!("session index: {e}")))?;

        // Session creation matches this exact desired-state field before
        // grouping reservations, so historical idle records never need to be
        // scanned to answer whether another request fits.
        self.sessions
            .create_index(reservation_status_index())
            .await
            .map_err(|e| StoreError::Unavailable(format!("session status index: {e}")))?;
        Ok(())
    }

    async fn ensure_tombstone_collection(&self) -> Result<(), StoreError> {
        match self.database.create_collection(TOMBSTONES).await {
            Ok(()) => Ok(()),
            Err(error) => {
                let found = self
                    .namespace_pair(TOMBSTONES, "_cma_unused_tombstone_target")
                    .await?;
                if found.legacy == Some(NamespaceKind::Collection) {
                    Ok(())
                } else {
                    Err(StoreError::Unavailable(format!(
                        "create permanent session tombstone collection: {error}"
                    )))
                }
            }
        }
    }

    async fn install_protocol_validators(&self) -> Result<(), StoreError> {
        for (collection, validator) in [
            (SESSIONS, session_protocol_validator()),
            (EVENTS, event_protocol_validator()),
            (WRITES, write_protocol_validator()),
            (TOMBSTONES, tombstone_protocol_validator()),
        ] {
            self.database
                .run_command(doc! {
                    "collMod": collection,
                    "validator": validator,
                    "validationLevel": "strict",
                    "validationAction": "error",
                })
                .await
                .map_err(|error| {
                    StoreError::Unavailable(format!(
                        "install MongoDB write-protocol validator on {collection}; the database role requires collMod: {error}"
                    ))
                })?;
        }
        Ok(())
    }

    async fn validate_post_fence_state(&self) -> Result<(), StoreError> {
        let tombstones = self.validated_tombstone_ids().await?;
        let sessions = self.validated_session_tails().await?;
        let event_tails = self.validated_event_tails().await?;

        for (id, event_tail) in &event_tails {
            if sessions.contains_key(id) || tombstones.contains(id) {
                continue;
            }
            // A new replica may have committed this session after the session
            // scan began. Re-read only the apparent orphan after all old names
            // are fenced, so an old writer cannot make the answer oscillate.
            if self
                .sessions
                .find_one(doc! { "_id": id })
                .await
                .map_err(|error| {
                    StoreError::Unavailable(format!("recheck event-history owner {id}: {error}"))
                })?
                .is_none()
                && !self.is_tombstoned(&SessionId(id.clone())).await?
            {
                return Err(StoreError::Malformed(format!(
                    "event history for session {id} ends at {event_tail} but has no session record or deletion tombstone after the writer fence"
                )));
            }
        }

        for (id, recorded_tail) in sessions {
            if tombstones.contains(&id) {
                continue;
            }
            let physical_tail = event_tails.get(&id).copied().unwrap_or(0);
            if recorded_tail <= physical_tail {
                continue;
            }
            self.recheck_record_tail(&id).await?;
        }
        Ok(())
    }

    async fn validated_tombstone_ids(&self) -> Result<HashSet<String>, StoreError> {
        let mut cursor = self
            .tombstones
            .clone_with_type::<Document>()
            .find(doc! {})
            .await
            .map_err(|error| {
                StoreError::Unavailable(format!("validate session tombstones: {error}"))
            })?;
        let mut ids = HashSet::new();
        while let Some(document) = cursor.try_next().await.map_err(|error| {
            StoreError::Unavailable(format!("session tombstone validation cursor: {error}"))
        })? {
            let id = document.get_str("_id").map(str::to_owned).map_err(|_| {
                StoreError::Malformed("session tombstone _id must be a string".into())
            })?;
            if id.is_empty() {
                return Err(StoreError::Malformed(
                    "session tombstone _id must not be empty".into(),
                ));
            }
            if !matches!(
                document.get_i32(WRITE_PROTOCOL_FIELD),
                Ok(protocol) if protocol == WRITE_PROTOCOL
            ) {
                return Err(StoreError::Malformed(format!(
                    "session tombstone {id} does not carry write protocol {WRITE_PROTOCOL}"
                )));
            }
            ids.insert(id);
        }
        Ok(ids)
    }

    async fn validated_session_tails(&self) -> Result<BTreeMap<String, u64>, StoreError> {
        let mut cursor = self
            .sessions
            .clone_with_type::<Document>()
            .find(doc! {})
            .sort(doc! { "_id": 1 })
            .await
            .map_err(|error| {
                StoreError::Unavailable(format!("validate session records: {error}"))
            })?;
        let mut tails = BTreeMap::new();
        while let Some(document) = cursor.try_next().await.map_err(|error| {
            StoreError::Unavailable(format!("session validation cursor: {error}"))
        })? {
            let (id, tail) = validate_stored_session_document(&document)?;
            tails.insert(id, tail);
        }
        Ok(tails)
    }

    async fn validated_event_tails(&self) -> Result<BTreeMap<String, u64>, StoreError> {
        let mut cursor = self
            .events
            .find(doc! {})
            .sort(doc! { "session_id": 1, "seq": 1 })
            .await
            .map_err(|error| {
                StoreError::Unavailable(format!("validate session event history: {error}"))
            })?;
        let mut sequence = StoredEventSequence::default();
        let mut tails = BTreeMap::new();
        while let Some(document) = cursor
            .try_next()
            .await
            .map_err(|error| StoreError::Unavailable(format!("event validation cursor: {error}")))?
        {
            event_doc::from_document(document.clone())?;
            let (id, seq) = sequence.observe(&document)?;
            tails.insert(id, seq);
        }
        Ok(tails)
    }

    async fn recheck_record_tail(&self, id: &str) -> Result<(), StoreError> {
        if self.is_tombstoned(&SessionId(id.to_owned())).await? {
            return Ok(());
        }
        let Some(record) = self
            .sessions
            .clone_with_type::<Document>()
            .find_one(doc! { "_id": id })
            .await
            .map_err(|error| {
                StoreError::Unavailable(format!("recheck session tail for {id}: {error}"))
            })?
        else {
            return Ok(());
        };
        let (_, recorded_tail) = validate_stored_session_document(&record)?;
        let physical_tail = self
            .last_event_sequence(&SessionId(id.to_owned()))
            .await?
            .unwrap_or(0);
        if recorded_tail > physical_tail {
            return Err(StoreError::Malformed(format!(
                "session {id} advertises event {recorded_tail}, but contiguous physical history ends at {physical_tail} after the old writer was fenced"
            )));
        }
        Ok(())
    }
}

async fn bounded_ping(
    ping: impl Future<Output = mongodb::error::Result<Document>> + Send + 'static,
    timeout: Duration,
) -> Result<(), StoreError> {
    match bounded_mongo_command("ping", ping, timeout).await? {
        Ok(_) => Ok(()),
        Err(error) => Err(StoreError::Unavailable(format!("ping: {error}"))),
    }
}

async fn bounded_mongo_command(
    operation: &'static str,
    command: impl Future<Output = mongodb::error::Result<Document>> + Send + 'static,
    timeout: Duration,
) -> Result<mongodb::error::Result<Document>, StoreError> {
    // MongoDB driver futures must be polled to completion. Timing out the
    // JoinHandle detaches the task instead of cancelling the driver operation.
    let command = tokio::spawn(command);
    match tokio::time::timeout(timeout, command).await {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(error)) => Err(StoreError::Unavailable(format!(
            "{operation} task failed: {error}"
        ))),
        Err(_) => Err(StoreError::Unavailable(format!(
            "{operation} timed out after {} ms",
            timeout.as_millis()
        ))),
    }
}

async fn bounded_store_check(
    operation: &'static str,
    check: impl Future<Output = Result<(), StoreError>> + Send + 'static,
    timeout: Duration,
) -> Result<(), StoreError> {
    let check = tokio::spawn(check);
    match tokio::time::timeout(timeout, check).await {
        Ok(Ok(result)) => result,
        Ok(Err(error)) => Err(StoreError::Unavailable(format!(
            "{operation} task failed: {error}"
        ))),
        Err(_) => Err(StoreError::Unavailable(format!(
            "{operation} timed out after {} ms",
            timeout.as_millis()
        ))),
    }
}

fn command_error_code(error: &mongodb::error::Error) -> Option<i32> {
    match error.kind.as_ref() {
        mongodb::error::ErrorKind::Command(command) => Some(command.code),
        _ => None,
    }
}

fn validate_transaction_topology_reply(reply: &Document) -> Result<(), StoreError> {
    let replica_set = reply.get_str("setName").is_ok();
    let mongos = reply
        .get_str("msg")
        .is_ok_and(|message| message == "isdbgrid");
    if replica_set || mongos {
        return Ok(());
    }
    Err(StoreError::Unavailable(
        "MongoDB session persistence requires a replica set or mongos; standalone deployments do not support its transactions"
            .into(),
    ))
}

fn namespace_kind(specification: &CollectionSpecification) -> NamespaceKind {
    match specification.collection_type {
        CollectionType::Collection => NamespaceKind::Collection,
        CollectionType::View => NamespaceKind::View {
            source: specification.options.view_on.clone(),
            pipeline_is_empty: specification
                .options
                .pipeline
                .as_ref()
                .is_none_or(Vec::is_empty),
        },
        _ => NamespaceKind::Unsupported,
    }
}

fn next_namespace_action(
    pair: &NamespacePair,
    legacy: &str,
    authoritative: &str,
) -> Result<NamespaceAction, StoreError> {
    match (&pair.legacy, &pair.authoritative) {
        (None, None) => Ok(NamespaceAction::CreateAuthoritative),
        (Some(NamespaceKind::Collection), None) => Ok(NamespaceAction::RenameLegacy),
        (None, Some(NamespaceKind::Collection)) => Ok(NamespaceAction::CreateLegacyView),
        (
            Some(NamespaceKind::View {
                source: Some(source),
                pipeline_is_empty: true,
            }),
            Some(NamespaceKind::Collection),
        ) if source == authoritative => Ok(NamespaceAction::Ready),
        _ => Err(StoreError::Unavailable(format!(
            "MongoDB namespace cutover is ambiguous for {legacy} -> {authoritative}: {pair:?}; refusing to merge, replace, or drop data"
        ))),
    }
}

fn protocol_validator(mut required: Vec<&str>, long_fields: &[&str]) -> Document {
    required.insert(0, WRITE_PROTOCOL_FIELD);
    let mut properties = Document::new();
    properties.insert(
        WRITE_PROTOCOL_FIELD,
        Bson::Document(doc! {
            "bsonType": "int",
            "enum": [WRITE_PROTOCOL],
        }),
    );
    for field in long_fields {
        properties.insert(*field, Bson::Document(doc! { "bsonType": "long" }));
    }
    doc! {
        "$jsonSchema": {
            "bsonType": "object",
            "required": required,
            "properties": properties,
        }
    }
}

fn session_protocol_validator() -> Document {
    protocol_validator(vec!["revision"], &["revision"])
}

fn event_protocol_validator() -> Document {
    protocol_validator(Vec::new(), &[])
}

fn write_protocol_validator() -> Document {
    protocol_validator(Vec::new(), &[])
}

fn tombstone_protocol_validator() -> Document {
    protocol_validator(Vec::new(), &[])
}

fn appended_event_session_update(record: &Document, tail: i64) -> Document {
    let mut set = Document::new();
    set.insert(WRITE_PROTOCOL_FIELD, WRITE_PROTOCOL);
    if !record.contains_key("revision") {
        // The first write touching a pre-revision record gives its absent
        // revision the same explicit zero used by the legacy CAS filter.
        set.insert("revision", 0_i64);
    }
    doc! {
        "$set": set,
        "$max": { "last_seq": tail },
    }
}

impl StoredEventSequence {
    fn observe(&mut self, document: &Document) -> Result<(String, u64), StoreError> {
        let id = document
            .get_str("session_id")
            .map(str::to_owned)
            .map_err(|_| StoreError::Malformed("event session_id must be a string".into()))?;
        if id.is_empty() {
            return Err(StoreError::Malformed(
                "event session_id must not be empty".into(),
            ));
        }
        let stored = document
            .get_i64("seq")
            .map_err(|_| StoreError::Malformed("event seq must be a 64-bit integer".into()))?;
        let seq = u64::try_from(stored)
            .map_err(|_| StoreError::Malformed("event seq must not be negative".into()))?;
        let expected = if self.session_id.as_deref() == Some(id.as_str()) {
            self.next
        } else {
            1
        };
        if seq != expected {
            return Err(StoreError::Malformed(format!(
                "event history for session {id} is not contiguous: expected {expected}, found {seq}"
            )));
        }
        self.session_id = Some(id.clone());
        self.next = seq
            .checked_add(1)
            .ok_or_else(|| StoreError::Malformed(format!("event sequence overflow for {id}")))?;
        Ok((id, seq))
    }
}

fn validate_stored_session_document(document: &Document) -> Result<(String, u64), StoreError> {
    let id = document
        .get_str("_id")
        .map(str::to_owned)
        .map_err(|_| StoreError::Malformed("session _id must be a string".into()))?;
    if id.is_empty() {
        return Err(StoreError::Malformed(
            "session _id must not be empty".into(),
        ));
    }
    let tail = document
        .get_i64("last_seq")
        .map_err(|_| {
            StoreError::Malformed(format!("session {id} last_seq must be a 64-bit integer"))
        })
        .and_then(|tail| {
            u64::try_from(tail).map_err(|_| {
                StoreError::Malformed(format!("session {id} last_seq must not be negative"))
            })
        })?;
    match document.get("revision") {
        None => {}
        Some(Bson::Int64(revision)) if *revision >= 0 => {}
        Some(Bson::Int64(_)) => {
            return Err(StoreError::Malformed(format!(
                "session {id} revision must not be negative"
            )))
        }
        Some(_) => {
            return Err(StoreError::Malformed(format!(
                "session {id} revision must be a 64-bit integer"
            )))
        }
    }
    if let Some(protocol) = document.get(WRITE_PROTOCOL_FIELD) {
        if protocol != &Bson::Int32(WRITE_PROTOCOL) {
            return Err(StoreError::Malformed(format!(
                "session {id} carries an unknown write protocol"
            )));
        }
    }
    let stored: SessionDoc = mongodb::bson::deserialize_from_document(document.clone())
        .map_err(|error| StoreError::Malformed(format!("session {id}: {error}")))?;
    from_doc(stored)?;
    Ok((id, tail))
}

fn reservation_status_index() -> IndexModel {
    IndexModel::builder()
        .keys(doc! { "status": 1 })
        .options(IndexOptions::builder().name("status_1".to_string()).build())
        .build()
}

/// The duplicate-key code, which is the unique index on `(session_id, seq)`
/// doing its job.
const DUPLICATE_KEY: i32 = 11000;

/// Read a failed insert as a sequence collision, if that is what it was.
///
/// False for anything else, so a store that is merely unreachable is still
/// reported as unreachable. A sequence race is resolved from the durable tail
/// by the core; whole-write idempotency is resolved separately by receipts.
///
/// Event inserts run inside a transaction, so a duplicate aborts the entire
/// batch. This helper only identifies the server error; the durable tail is
/// read after the abort to construct the typed collision.
fn sequence_taken(error: &mongodb::error::Error) -> bool {
    let mongodb::error::ErrorKind::InsertMany(ref failed) = *error.kind else {
        return false;
    };
    failed
        .write_errors
        .as_ref()
        .is_some_and(|errors| errors.iter().any(|error| error.code == DUPLICATE_KEY))
}

/// Refuse any batch that would make a higher sequence readable across a hole.
///
/// Checked as a batch before insertion so the refusal writes none of it. A
/// lower sequence is `SequenceTaken`: it belongs behind the durable tail and
/// the caller must rebase it. `NonContiguousAppend` is reserved for a forward
/// gap whose missing lower writer may still be in flight.
fn validate_contiguous_append(events: &[Event], last: Option<u64>) -> Result<(), StoreError> {
    let Some(first) = events.first() else {
        return Ok(());
    };
    let id = &first.session_id;
    let mut expected = last
        .unwrap_or(0)
        .checked_add(1)
        .ok_or_else(|| StoreError::Malformed(format!("event sequence overflow for {id}")))?;

    for event in events {
        if event.session_id != *id {
            return Err(StoreError::Malformed(
                "one event append batch named more than one session".into(),
            ));
        }
        if event.seq < expected {
            tracing::error!(
                session_id = %id,
                expected,
                got = event.seq,
                "refused an event sequence already below the durable tail"
            );
            return Err(StoreError::SequenceTaken {
                expected,
                got: event.seq,
            });
        }
        if event.seq > expected {
            tracing::error!(
                session_id = %id,
                expected,
                got = event.seq,
                "refused a non-contiguous event append"
            );
            return Err(StoreError::NonContiguousAppend {
                expected,
                got: event.seq,
            });
        }
        expected = expected
            .checked_add(1)
            .ok_or_else(|| StoreError::Malformed(format!("event sequence overflow for {id}")))?;
    }
    Ok(())
}

fn stored_event_tail(found: Option<Document>) -> Result<Option<u64>, StoreError> {
    let Some(found) = found else {
        return Ok(None);
    };
    let seq = found
        .get_i64("seq")
        .map_err(|_| StoreError::Malformed("event tail seq must be a 64-bit integer".into()))?;
    u64::try_from(seq)
        .map(Some)
        .map_err(|_| StoreError::Malformed("event tail seq must not be negative".into()))
}

/// The next atomic writer starts after both facts an older deployment may have
/// exposed: events that exist, and cursor positions its session record already
/// advertised. The old two-step write could leave either one ahead of the
/// other; taking the maximum preserves those cursors without allowing a new
/// transaction to create another hole.
fn durable_sequence_floor(
    event_tail: Option<u64>,
    recorded_tail: Option<i64>,
) -> Result<u64, StoreError> {
    let recorded_tail = recorded_tail
        .map(|tail| {
            u64::try_from(tail).map_err(|_| {
                StoreError::Malformed("session event tail must not be negative".into())
            })
        })
        .transpose()?
        .unwrap_or(0);
    Ok(event_tail.unwrap_or(0).max(recorded_tail))
}

fn resolved_transaction_tail(
    id: &SessionId,
    sequence_floor: u64,
    requested_tail: u64,
    events: &[Event],
) -> Result<u64, StoreError> {
    let durable_tail = events.last().map_or(sequence_floor, |event| event.seq);
    let tail_matches = if events.is_empty() {
        requested_tail <= sequence_floor
    } else {
        requested_tail == durable_tail
    };
    if !tail_matches {
        return Err(StoreError::Malformed(format!(
            "session {id} record tail {requested_tail} does not match durable tail {durable_tail}"
        )));
    }
    Ok(durable_tail)
}

fn validate_transaction_input(
    write_id: &str,
    record: &SessionRecord,
    events: &[Event],
    create: bool,
) -> Result<(), StoreError> {
    if write_id.is_empty() {
        return Err(StoreError::Malformed(
            "session write ID must not be empty".into(),
        ));
    }
    if record.session.id.0.is_empty() {
        return Err(StoreError::Malformed("session id must not be empty".into()));
    }
    if events
        .iter()
        .any(|event| event.session_id != record.session.id)
    {
        return Err(StoreError::Malformed(
            "one durable session write named more than one session".into(),
        ));
    }
    if create && record.revision != 0 {
        return Err(StoreError::RevisionConflict {
            expected: 0,
            actual: record.revision,
        });
    }
    Ok(())
}

fn receipt_revision(
    receipt: &SessionWriteDoc,
    write_id: &str,
    id: &SessionId,
) -> Result<u64, StoreError> {
    if receipt.session_id != id.0 {
        return Err(StoreError::Malformed(format!(
            "session write ID {write_id} belongs to another session"
        )));
    }
    u64::try_from(receipt.revision)
        .map_err(|_| StoreError::Malformed("session write receipt revision is negative".into()))
}

fn next_stored_revision(actual: u64) -> Result<(u64, i64), StoreError> {
    let revision = actual
        .checked_add(1)
        .ok_or_else(|| StoreError::Malformed("session revision overflow".into()))?;
    let stored = i64::try_from(revision)
        .map_err(|_| StoreError::Malformed("session revision cannot be stored".into()))?;
    Ok((revision, stored))
}

fn session_update_options() -> ReplaceOptions {
    ReplaceOptions::builder().upsert(false).build()
}

fn session_revision_filter(id: &str, expected: i64) -> Document {
    if expected == 0 {
        // Records written before optimistic revisions have no `revision`
        // field. Deserialization deliberately reads those as revision zero,
        // so their first CAS must give the absent field the same meaning.
        doc! {
            "_id": id,
            "$or": [
                { "revision": 0_i64 },
                { "revision": { "$exists": false } },
            ],
        }
    } else {
        doc! { "_id": id, "revision": expected }
    }
}

fn stored_event_cursor(after: u64) -> Option<i64> {
    i64::try_from(after).ok()
}

fn to_doc(r: &SessionRecord) -> Result<SessionDoc, StoreError> {
    let s = &r.session;
    Ok(SessionDoc {
        id: s.id.0.clone(),
        write_protocol: WRITE_PROTOCOL,
        owner: s.owner.0.clone(),
        kernel: s.kernel.0.clone(),
        status: serde_plain_string(&s.status)?,
        recovery_state: serde_plain_string(&s.recovery_state)?,
        stop_reason: match &s.stop_reason {
            Some(v) => Some(serde_plain_string(v)?),
            None => None,
        },
        columns: s.geometry.columns() as u32,
        rows: s.geometry.rows() as u32,
        last_seq: i64::try_from(s.last_seq).map_err(|_| {
            StoreError::Malformed(format!(
                "session event tail {} cannot be stored",
                s.last_seq
            ))
        })?,
        created_at: s.created_at.clone(),
        options: Some(
            mongodb::bson::serialize_to_bson(&s.options)
                .map_err(|e| StoreError::Malformed(e.to_string()))?,
        ),
        environment_profile: s.environment_profile.as_ref().map(|p| p.0.clone()),
        runtime_id: r.runtime_id.clone(),
        context_ref: r.context_ref.clone(),
        runtime_version: s.runtime_version.clone(),
        rotations: Some(i64::from(s.rotations)),
        rotated_at: s.rotated_at.clone(),
        capabilities: Some(
            mongodb::bson::serialize_to_bson(&s.capabilities)
                .map_err(|e| StoreError::Malformed(e.to_string()))?,
        ),
        granted_services: s
            .granted_services
            .as_ref()
            .map(|services| services.iter().map(|id| id.0.clone()).collect()),
        granted_skills: s.granted_skills.as_ref().map(|skills| {
            skills
                .iter()
                .map(|granted| StoredSkill::Versioned {
                    id: granted.id.0.clone(),
                    version: granted.version.clone(),
                })
                .collect()
        }),
        pricing: s.pricing.as_ref().map(pricing_to_doc).transpose()?,
        revision: i64::try_from(r.revision).map_err(|_| {
            StoreError::Malformed(format!("session revision {} cannot be stored", r.revision))
        })?,
    })
}

fn pricing_to_doc(pricing: &SessionPricing) -> Result<PricingDoc, StoreError> {
    Ok(PricingDoc {
        rate_card: pricing.rate_card.0.clone(),
        cpu_micros_per_cpu_hour: price_to_i64(pricing.rate.cpu_micros_per_cpu_hour)?,
        memory_micros_per_gib_hour: price_to_i64(pricing.rate.memory_micros_per_gib_hour)?,
        cpu_millis: capacity_to_i64(pricing.cpu_millis, "CPU millicores")?,
        memory_mib: capacity_to_i64(pricing.memory_mib, "memory MiB")?,
        micros_per_hour: price_to_i64(pricing.micros_per_hour)?,
        quoted_at: pricing.quoted_at.clone(),
    })
}

fn price_to_i64(value: u64) -> Result<i64, StoreError> {
    i64::try_from(value).map_err(|_| {
        StoreError::Malformed(format!("a price of {value} micro-USD cannot be stored"))
    })
}

fn capacity_to_i64(value: u32, unit: &str) -> Result<i64, StoreError> {
    i64::try_from(u64::from(value)).map_err(|_| {
        StoreError::Malformed(format!(
            "a session capacity of {value} {unit} cannot be stored"
        ))
    })
}

fn pricing_from_doc(pricing: PricingDoc) -> Result<SessionPricing, StoreError> {
    Ok(SessionPricing {
        rate_card: RateCardId(pricing.rate_card),
        rate: Rate {
            cpu_micros_per_cpu_hour: stored_price(pricing.cpu_micros_per_cpu_hour)?,
            memory_micros_per_gib_hour: stored_price(pricing.memory_micros_per_gib_hour)?,
        },
        cpu_millis: stored_capacity(pricing.cpu_millis, "CPU millicores")?,
        memory_mib: stored_capacity(pricing.memory_mib, "memory MiB")?,
        micros_per_hour: stored_price(pricing.micros_per_hour)?,
        quoted_at: pricing.quoted_at,
    })
}

fn stored_price(value: i64) -> Result<u64, StoreError> {
    u64::try_from(value).map_err(|_| {
        StoreError::Malformed(format!("a stored price of {value} micro-USD is invalid"))
    })
}

fn stored_capacity(value: i64, unit: &str) -> Result<u32, StoreError> {
    u32::try_from(value).map_err(|_| {
        StoreError::Malformed(format!(
            "a stored session capacity of {value} {unit} is invalid"
        ))
    })
}

/// Render a core enum using its own serde representation.
///
/// Going through serde keeps the stored strings identical to the ones the
/// interfaces emit, so a value never means two different things depending on
/// which layer wrote it.
fn serde_plain_string<T: Serialize>(value: &T) -> Result<String, StoreError> {
    match serde_json::to_value(value).map_err(|e| StoreError::Malformed(e.to_string()))? {
        serde_json::Value::String(s) => Ok(s),
        other => Err(StoreError::Malformed(format!(
            "expected a string representation, got {other}"
        ))),
    }
}

fn parse_plain<T: for<'de> Deserialize<'de>>(s: &str) -> Result<T, StoreError> {
    serde_json::from_value(serde_json::Value::String(s.to_string()))
        .map_err(|e| StoreError::Malformed(format!("{s:?}: {e}")))
}

fn validate_status_transition(id: &SessionId, current: &str, next: &str) -> Result<(), StoreError> {
    let current: SessionStatus = parse_plain(current)?;
    let next: SessionStatus = parse_plain(next)?;
    if current.is_terminal() && !next.is_terminal() {
        return Err(StoreError::SessionTerminated { id: id.clone() });
    }
    Ok(())
}

fn tombstone_lookup_stage() -> Document {
    doc! {
        "$lookup": {
            "from": TOMBSTONES,
            "localField": "_id",
            "foreignField": "_id",
            "as": "_cma_tombstones",
        }
    }
}

fn visible_sessions_pipeline(filter: Document) -> Vec<Document> {
    vec![
        doc! { "$match": filter },
        tombstone_lookup_stage(),
        doc! { "$match": { "_cma_tombstones.0": { "$exists": false } } },
        doc! { "$project": { "_cma_tombstones": 0 } },
        doc! { "$sort": { "created_at": -1 } },
    ]
}

fn visible_count_pipeline(filter: Document) -> Vec<Document> {
    vec![
        doc! { "$match": filter },
        tombstone_lookup_stage(),
        doc! { "$match": { "_cma_tombstones.0": { "$exists": false } } },
        doc! { "$group": { "_id": null, "count": { "$sum": 1_i64 } } },
    ]
}

/// A bounded server-side sum of every durable promise that wants a runtime.
fn reserved_capacity_pipeline() -> Result<Vec<Document>, StoreError> {
    let statuses = SessionStatus::WANTING_RUNTIME
        .iter()
        .map(serde_plain_string)
        .collect::<Result<Vec<_>, _>>()?;

    Ok(vec![
        doc! { "$match": { "status": { "$in": statuses } } },
        tombstone_lookup_stage(),
        doc! { "$match": { "_cma_tombstones.0": { "$exists": false } } },
        doc! {
            "$group": {
                "_id": null,
                "sessions": { "$sum": 1_i64 },
                "unsized_sessions": {
                    "$sum": {
                        "$cond": [
                            { "$eq": [{ "$ifNull": ["$options.size", null] }, null] },
                            1_i64,
                            0_i64,
                        ]
                    }
                },
                "cpu_millis": {
                    "$sum": { "$ifNull": ["$options.size.cpu_millis", 0_i64] }
                },
                "memory_mib": {
                    "$sum": { "$ifNull": ["$options.size.memory_mib", 0_i64] }
                },
            }
        },
    ])
}

fn reserved_from_doc(doc: ReservedCapacityDoc) -> Result<Reserved, StoreError> {
    fn unsigned(value: i64, field: &str) -> Result<u64, StoreError> {
        u64::try_from(value).map_err(|_| {
            StoreError::Malformed(format!(
                "reserved-capacity aggregation returned negative {field}: {value}"
            ))
        })
    }

    Ok(Reserved {
        sessions: unsigned(doc.sessions, "session count")?,
        unsized_sessions: unsigned(doc.unsized_sessions, "unsized session count")?,
        cpu_millis: unsigned(doc.cpu_millis, "CPU total")?,
        memory_mib: unsigned(doc.memory_mib, "memory total")?,
    })
}

fn reserved_from_optional_doc(doc: Option<ReservedCapacityDoc>) -> Result<Reserved, StoreError> {
    doc.map_or_else(|| Ok(Reserved::default()), reserved_from_doc)
}

fn from_doc(d: SessionDoc) -> Result<SessionRecord, StoreError> {
    use layer2_core::{Session, TerminalGeometry};
    let geometry = TerminalGeometry::new(d.columns as u16, d.rows as u16)
        .map_err(|e| StoreError::Malformed(e.to_string()))?;
    Ok(SessionRecord {
        session: Session {
            id: SessionId(d.id),
            owner: SubjectId(d.owner),
            kernel: layer2_core::KernelId::new(d.kernel),
            status: parse_plain(&d.status)?,
            recovery_state: parse_plain(&d.recovery_state)?,
            stop_reason: match d.stop_reason {
                Some(v) => Some(parse_plain(&v)?),
                None => None,
            },
            runtime_version: d.runtime_version,
            rotations: d.rotations.unwrap_or(0).clamp(0, i64::from(u32::MAX)) as u32,
            rotated_at: d.rotated_at,
            capabilities: match d.capabilities {
                Some(v) => mongodb::bson::deserialize_from_bson(v)
                    .map_err(|e| StoreError::Malformed(e.to_string()))?,
                None => Default::default(),
            },
            environment_profile: d.environment_profile.map(layer2_core::EnvironmentProfileId),
            granted_services: d
                .granted_services
                .map(|services| services.into_iter().map(layer2_core::ServiceId).collect()),
            granted_skills: d
                .granted_skills
                .map(|skills| skills.into_iter().map(StoredSkill::into_granted).collect()),
            pricing: d.pricing.map(pricing_from_doc).transpose()?,
            options: match d.options {
                Some(v) => mongodb::bson::deserialize_from_bson(v)
                    .map_err(|e| StoreError::Malformed(e.to_string()))?,
                None => Default::default(),
            },
            geometry,
            last_seq: d.last_seq.max(0) as u64,
            created_at: d.created_at,
        },
        runtime_id: d.runtime_id,
        context_ref: d.context_ref,
        revision: u64::try_from(d.revision)
            .map_err(|_| StoreError::Malformed("session revision must not be negative".into()))?,
    })
}

#[async_trait::async_trait]
impl SessionStore for MongoSessionStore {
    async fn ping(&self) -> Result<(), StoreError> {
        let database = self.database.clone();
        bounded_ping(
            async move { database.run_command(doc! { "ping": 1_i32 }).await },
            STORE_PING_TIMEOUT,
        )
        .await?;
        self.validate_transaction_topology(STORE_PING_TIMEOUT)
            .await?;
        let store = self.clone();
        bounded_store_check(
            "MongoDB namespace readiness",
            async move { store.verify_versioned_namespaces().await },
            STORE_PING_TIMEOUT,
        )
        .await
    }

    async fn upsert(&self, record: &SessionRecord) -> Result<(), StoreError> {
        self.transactional_upsert(record).await
    }

    async fn get(&self, id: &SessionId) -> Result<Option<SessionRecord>, StoreError> {
        Ok(self.query(doc! { "_id": &id.0 }).await?.into_iter().next())
    }

    async fn list_for(&self, owner: &SubjectId) -> Result<Vec<SessionRecord>, StoreError> {
        self.query(doc! { "owner": &owner.0 }).await
    }

    async fn count_live_for(&self, owner: &SubjectId) -> Result<u32, StoreError> {
        let statuses = SessionStatus::WANTING_RUNTIME
            .iter()
            .map(serde_plain_string)
            .collect::<Result<Vec<_>, _>>()?;
        let mut cursor = self
            .sessions
            .aggregate(visible_count_pipeline(doc! {
                "owner": &owner.0,
                "status": { "$in": statuses },
            }))
            .await
            .map_err(|e| StoreError::Unavailable(format!("count live sessions: {e}")))?;
        let count = cursor
            .try_next()
            .await
            .map_err(|e| StoreError::Unavailable(format!("count live sessions cursor: {e}")))?
            .map(mongodb::bson::deserialize_from_document::<SessionCountDoc>)
            .transpose()
            .map_err(|e| StoreError::Malformed(format!("live session count: {e}")))?
            .map_or(0, |count| count.count);
        u32::try_from(count)
            .map_err(|_| StoreError::Malformed("live session count exceeds u32".into()))
    }

    async fn reserved_capacity(&self) -> Result<Reserved, StoreError> {
        let mut cursor = self
            .sessions
            .aggregate(reserved_capacity_pipeline()?)
            .await
            .map_err(|e| StoreError::Unavailable(format!("sum reserved capacity: {e}")))?;
        let document = cursor
            .try_next()
            .await
            .map_err(|e| StoreError::Unavailable(format!("reserved capacity cursor: {e}")))?;
        let aggregate = document
            .map(mongodb::bson::deserialize_from_document)
            .transpose()
            .map_err(|e| StoreError::Malformed(format!("reserved capacity: {e}")))?;
        reserved_from_optional_doc(aggregate)
    }

    async fn list_all(&self) -> Result<Vec<SessionRecord>, StoreError> {
        self.query(doc! {}).await
    }

    async fn delete(&self, id: &SessionId) -> Result<(), StoreError> {
        self.transactional_delete(id).await
    }

    async fn append_events(&self, events: &[Event]) -> Result<(), StoreError> {
        self.transactional_events(events).await
    }

    async fn commit_session_idempotent(
        &self,
        write_id: &str,
        record: &SessionRecord,
        events: &[Event],
    ) -> Result<u64, StoreError> {
        self.transactional_commit(write_id, record, events, false)
            .await
    }

    async fn create_session_idempotent(
        &self,
        write_id: &str,
        record: &SessionRecord,
        events: &[Event],
    ) -> Result<u64, StoreError> {
        self.transactional_commit(write_id, record, events, true)
            .await
    }

    async fn event_tail(&self, id: &SessionId) -> Result<u64, StoreError> {
        if self.is_tombstoned(id).await? {
            return Ok(0);
        }
        Ok(self.last_event_sequence(id).await?.unwrap_or(0))
    }

    async fn events_since(
        &self,
        id: &SessionId,
        after: u64,
    ) -> Result<Vec<StoredEvent>, StoreError> {
        if self.is_tombstoned(id).await? {
            return Ok(Vec::new());
        }
        let Some(after) = stored_event_cursor(after) else {
            // Mongo cannot contain an event above i64::MAX, so a cursor beyond
            // that point is already past every representable event.
            return Ok(Vec::new());
        };
        let mut cursor = self
            .events
            .find(doc! { "session_id": &id.0, "seq": { "$gt": after } })
            .sort(doc! { "seq": 1 })
            .await
            .map_err(|e| StoreError::Unavailable(format!("events: {e}")))?;
        let mut out = Vec::new();
        while let Some(d) = cursor
            .try_next()
            .await
            .map_err(|e| StoreError::Unavailable(format!("events cursor: {e}")))?
        {
            out.push(event_doc::from_document(d)?);
        }
        Ok(out)
    }
}

impl MongoSessionStore {
    async fn transactional_upsert(&self, record: &SessionRecord) -> Result<(), StoreError> {
        let id = record.session.id.clone();
        let document = to_doc(record)?;
        let context = (
            self.sessions.clone(),
            self.events.clone(),
            self.tombstones.clone(),
            id.clone(),
            document,
        );
        let mut session = self.client.start_session().await.map_err(|error| {
            StoreError::Unavailable(format!("start session upsert transaction: {error}"))
        })?;
        let outcome = session
            .start_transaction()
            .read_concern(ReadConcern::snapshot())
            .write_concern(WriteConcern::majority())
            .and_run(
                context,
                |session, (sessions, events, tombstones, id, document)| {
                    async move {
                        if tombstones
                            .find_one(doc! { "_id": &id.0 })
                            .session(&mut *session)
                            .await?
                            .is_some()
                        {
                            return Err(mongodb::error::Error::custom(
                                StoreError::SessionDeleted { id: (*id).clone() },
                            ));
                        }
                        let existing = sessions
                            .find_one(doc! { "_id": &id.0 })
                            .session(&mut *session)
                            .await?;
                        if let Some(existing) = existing.as_ref() {
                            validate_status_transition(id, &existing.status, &document.status)
                                .map_err(mongodb::error::Error::custom)?;
                        }
                        let found = events
                            .find_one(doc! { "session_id": &id.0 })
                            .sort(doc! { "seq": -1 })
                            .projection(doc! { "_id": 0, "seq": 1 })
                            .session(&mut *session)
                            .await?;
                        let event_tail =
                            stored_event_tail(found).map_err(mongodb::error::Error::custom)?;
                        let sequence_floor = durable_sequence_floor(
                            event_tail,
                            existing.as_ref().map(|record| record.last_seq),
                        )
                        .map_err(mongodb::error::Error::custom)?;
                        let requested_tail = u64::try_from(document.last_seq).map_err(|_| {
                            mongodb::error::Error::custom(StoreError::Malformed(
                                "session event tail must not be negative".into(),
                            ))
                        })?;
                        let durable_tail =
                            resolved_transaction_tail(id, sequence_floor, requested_tail, &[])
                                .map_err(mongodb::error::Error::custom)?;
                        document.last_seq = i64::try_from(durable_tail).map_err(|_| {
                            mongodb::error::Error::custom(StoreError::Malformed(format!(
                                "session event tail {durable_tail} cannot be stored"
                            )))
                        })?;
                        sessions
                            .replace_one(doc! { "_id": &document.id }, &*document)
                            .with_options(ReplaceOptions::builder().upsert(true).build())
                            .session(&mut *session)
                            .await?;
                        Ok(())
                    }
                    .boxed()
                },
            )
            .await;
        match outcome {
            Ok(()) => Ok(()),
            Err(error) => {
                if let Some(error) = error.get_custom::<StoreError>() {
                    return Err(error.clone());
                }
                Err(StoreError::Unavailable(format!(
                    "upsert session transaction: {error}"
                )))
            }
        }
    }

    /// Atomically commit one exact writer-owned record/event unit.
    ///
    /// The receipt is part of the transaction. The driver's convenient runner
    /// reruns the callback for `TransientTransactionError` and retries commit
    /// resolution for `UnknownTransactionCommitResult`; either path therefore
    /// finds the same receipt rather than repeating events or an old record.
    async fn transactional_commit(
        &self,
        write_id: &str,
        record: &SessionRecord,
        events: &[Event],
        create: bool,
    ) -> Result<u64, StoreError> {
        validate_transaction_input(write_id, record, events, create)?;

        let id = record.session.id.clone();
        let documents = events
            .iter()
            .map(event_doc::to_document)
            .collect::<Result<Vec<_>, _>>()?;
        let record = to_doc(record)?;
        let mut session = self.client.start_session().await.map_err(|error| {
            StoreError::Unavailable(format!("start session write transaction: {error}"))
        })?;
        let transaction = EventTransaction {
            events_collection: self.events.clone(),
            sessions_collection: self.sessions.clone(),
            writes_collection: self.writes.clone(),
            tombstones_collection: self.tombstones.clone(),
            id: id.clone(),
            write_id: write_id.to_string(),
            create,
            events: events.to_vec(),
            documents,
            record,
        };

        let outcome = session
            .start_transaction()
            .read_concern(ReadConcern::snapshot())
            .write_concern(WriteConcern::majority())
            .and_run(transaction, |session, transaction| {
                async move {
                    if transaction
                        .tombstones_collection
                        .find_one(doc! { "_id": &transaction.id.0 })
                        .session(&mut *session)
                        .await?
                        .is_some()
                    {
                        return Err(mongodb::error::Error::custom(StoreError::SessionDeleted {
                            id: transaction.id.clone(),
                        }));
                    }
                    if let Some(receipt) = transaction
                        .writes_collection
                        .find_one(doc! { "_id": &transaction.write_id })
                        .session(&mut *session)
                        .await?
                    {
                        return receipt_revision(&receipt, &transaction.write_id, &transaction.id)
                            .map_err(mongodb::error::Error::custom);
                    }

                    let existing = transaction
                        .sessions_collection
                        .find_one(doc! { "_id": &transaction.id.0 })
                        .session(&mut *session)
                        .await?;
                    let actual = existing
                        .as_ref()
                        .map(|record| {
                            u64::try_from(record.revision).map_err(|_| {
                                mongodb::error::Error::custom(StoreError::Malformed(
                                    "session revision must not be negative".into(),
                                ))
                            })
                        })
                        .transpose()?
                        .unwrap_or(0);
                    let expected = u64::try_from(transaction.record.revision).map_err(|_| {
                        mongodb::error::Error::custom(StoreError::Malformed(
                            "session revision must not be negative".into(),
                        ))
                    })?;
                    if transaction.create {
                        if existing.is_some() {
                            return Err(mongodb::error::Error::custom(
                                StoreError::RevisionConflict {
                                    expected: 0,
                                    actual,
                                },
                            ));
                        }
                    } else if existing.is_none() {
                        return Err(mongodb::error::Error::custom(StoreError::SessionDeleted {
                            id: transaction.id.clone(),
                        }));
                    } else if actual != expected {
                        return Err(mongodb::error::Error::custom(
                            StoreError::RevisionConflict { expected, actual },
                        ));
                    }
                    if let Some(existing) = existing.as_ref() {
                        validate_status_transition(
                            &transaction.id,
                            &existing.status,
                            &transaction.record.status,
                        )
                        .map_err(mongodb::error::Error::custom)?;
                    }

                    let found = transaction
                        .events_collection
                        .find_one(doc! { "session_id": &transaction.id.0 })
                        .sort(doc! { "seq": -1 })
                        .projection(doc! { "_id": 0, "seq": 1 })
                        .session(&mut *session)
                        .await?;
                    let event_tail =
                        stored_event_tail(found).map_err(mongodb::error::Error::custom)?;
                    let sequence_floor = durable_sequence_floor(
                        event_tail,
                        existing.as_ref().map(|record| record.last_seq),
                    )
                    .map_err(mongodb::error::Error::custom)?;
                    validate_contiguous_append(&transaction.events, Some(sequence_floor))
                        .map_err(mongodb::error::Error::custom)?;
                    let durable_tail = resolved_transaction_tail(
                        &transaction.id,
                        sequence_floor,
                        transaction.record.last_seq as u64,
                        &transaction.events,
                    )
                    .map_err(mongodb::error::Error::custom)?;

                    let (revision, revision_i64) =
                        next_stored_revision(actual).map_err(mongodb::error::Error::custom)?;
                    let mut durable = transaction.record.clone();
                    durable.revision = revision_i64;
                    durable.last_seq = i64::try_from(durable_tail).map_err(|_| {
                        mongodb::error::Error::custom(StoreError::Malformed(format!(
                            "session event tail {durable_tail} cannot be stored"
                        )))
                    })?;

                    if !transaction.documents.is_empty() {
                        transaction
                            .events_collection
                            .insert_many(transaction.documents.iter())
                            .session(&mut *session)
                            .await?;
                    }
                    if transaction.create {
                        transaction
                            .sessions_collection
                            .insert_one(&durable)
                            .session(&mut *session)
                            .await?;
                    } else {
                        let replaced = transaction
                            .sessions_collection
                            .replace_one(
                                session_revision_filter(&durable.id, transaction.record.revision),
                                &durable,
                            )
                            .with_options(session_update_options())
                            .session(&mut *session)
                            .await?;
                        if replaced.modified_count != 1 {
                            return Err(mongodb::error::Error::custom(
                                StoreError::RevisionConflict { expected, actual },
                            ));
                        }
                    }
                    transaction
                        .writes_collection
                        .insert_one(SessionWriteDoc {
                            id: transaction.write_id.clone(),
                            write_protocol: WRITE_PROTOCOL,
                            session_id: transaction.id.0.clone(),
                            revision: revision_i64,
                        })
                        .session(&mut *session)
                        .await?;
                    Ok(revision)
                }
                .boxed()
            })
            .await;

        match outcome {
            Ok(revision) => Ok(revision),
            Err(error) => {
                if let Some(error) = error.get_custom::<StoreError>() {
                    return Err(error.clone());
                }
                if sequence_taken(&error) {
                    if let Some(revision) = self.write_receipt(write_id, &id).await? {
                        return Ok(revision);
                    }
                    if let Some(first) = events.first() {
                        let expected = self
                            .last_event_sequence(&id)
                            .await?
                            .unwrap_or(0)
                            .checked_add(1)
                            .ok_or_else(|| {
                                StoreError::Malformed(format!("event sequence overflow for {id}"))
                            })?;
                        return Err(StoreError::SequenceTaken {
                            expected,
                            got: first.seq,
                        });
                    }
                }
                Err(StoreError::Unavailable(format!(
                    "commit session write transaction: {error}"
                )))
            }
        }
    }

    async fn transactional_events(&self, events: &[Event]) -> Result<(), StoreError> {
        let Some(first) = events.first() else {
            return Ok(());
        };
        let id = first.session_id.clone();
        let documents = events
            .iter()
            .map(event_doc::to_document)
            .collect::<Result<Vec<_>, _>>()?;
        let context = (
            self.events.clone(),
            self.sessions.clone_with_type::<Document>(),
            self.tombstones.clone(),
            id.clone(),
            events.to_vec(),
            documents,
        );
        let mut session = self.client.start_session().await.map_err(|error| {
            StoreError::Unavailable(format!("start event transaction: {error}"))
        })?;
        let outcome = session
            .start_transaction()
            .read_concern(ReadConcern::snapshot())
            .write_concern(WriteConcern::majority())
            .and_run(
                context,
                |session, (collection, sessions, tombstones, id, events, documents)| {
                    async move {
                        if tombstones
                            .find_one(doc! { "_id": &id.0 })
                            .session(&mut *session)
                            .await?
                            .is_some()
                        {
                            return Err(mongodb::error::Error::custom(
                                StoreError::SessionDeleted { id: (*id).clone() },
                            ));
                        }
                        let found = collection
                            .find_one(doc! { "session_id": &id.0 })
                            .sort(doc! { "seq": -1 })
                            .projection(doc! { "_id": 0, "seq": 1 })
                            .session(&mut *session)
                            .await?;
                        let event_tail =
                            stored_event_tail(found).map_err(mongodb::error::Error::custom)?;
                        let recorded = sessions
                            .find_one(doc! { "_id": &id.0 })
                            .projection(doc! { "_id": 0, "last_seq": 1, "revision": 1 })
                            .session(&mut *session)
                            .await?;
                        let recorded_tail = recorded
                            .as_ref()
                            .map(|record| {
                                record.get_i64("last_seq").map_err(|_| {
                                    mongodb::error::Error::custom(StoreError::Malformed(
                                        "session event tail must be a 64-bit integer".into(),
                                    ))
                                })
                            })
                            .transpose()?;
                        if recorded.is_none() {
                            return Err(mongodb::error::Error::custom(
                                StoreError::SessionDeleted { id: (*id).clone() },
                            ));
                        }
                        let sequence_floor = durable_sequence_floor(event_tail, recorded_tail)
                            .map_err(mongodb::error::Error::custom)?;
                        validate_contiguous_append(events, Some(sequence_floor))
                            .map_err(mongodb::error::Error::custom)?;
                        collection
                            .insert_many(documents.iter())
                            .session(&mut *session)
                            .await?;
                        let appended_tail = i64::try_from(
                            events
                                .last()
                                .expect("a raw append transaction is non-empty")
                                .seq,
                        )
                        .map_err(|_| {
                            mongodb::error::Error::custom(StoreError::Malformed(
                                "event tail cannot be stored".into(),
                            ))
                        })?;
                        let updated = sessions
                            .update_one(
                                doc! { "_id": &id.0 },
                                appended_event_session_update(
                                    recorded.as_ref().expect("presence checked above"),
                                    appended_tail,
                                ),
                            )
                            .session(&mut *session)
                            .await?;
                        if updated.matched_count != 1 {
                            return Err(mongodb::error::Error::custom(
                                StoreError::SessionDeleted { id: (*id).clone() },
                            ));
                        }
                        Ok(())
                    }
                    .boxed()
                },
            )
            .await;
        match outcome {
            Ok(()) => Ok(()),
            Err(error) => {
                if let Some(error) = error.get_custom::<StoreError>() {
                    return Err(error.clone());
                }
                if sequence_taken(&error) {
                    let expected = self
                        .last_event_sequence(&id)
                        .await?
                        .unwrap_or(0)
                        .checked_add(1)
                        .ok_or_else(|| {
                            StoreError::Malformed(format!("event sequence overflow for {id}"))
                        })?;
                    return Err(StoreError::SequenceTaken {
                        expected,
                        got: first.seq,
                    });
                }
                Err(StoreError::Unavailable(format!(
                    "commit event transaction: {error}"
                )))
            }
        }
    }

    async fn transactional_delete(&self, id: &SessionId) -> Result<(), StoreError> {
        let context = (
            self.sessions.clone(),
            self.events.clone(),
            self.writes.clone(),
            self.tombstones.clone(),
            id.clone(),
        );
        let mut session = self.client.start_session().await.map_err(|error| {
            StoreError::Unavailable(format!("start session deletion transaction: {error}"))
        })?;
        session
            .start_transaction()
            .read_concern(ReadConcern::snapshot())
            .write_concern(WriteConcern::majority())
            .and_run(
                context,
                |session, (sessions, events, writes, tombstones, id)| {
                    async move {
                        tombstones
                            .replace_one(
                                doc! { "_id": &id.0 },
                                SessionTombstoneDoc {
                                    id: id.0.clone(),
                                    write_protocol: WRITE_PROTOCOL,
                                },
                            )
                            .with_options(ReplaceOptions::builder().upsert(true).build())
                            .session(&mut *session)
                            .await?;
                        sessions
                            .delete_one(doc! { "_id": &id.0 })
                            .session(&mut *session)
                            .await?;
                        events
                            .delete_many(doc! { "session_id": &id.0 })
                            .session(&mut *session)
                            .await?;
                        writes
                            .delete_many(doc! { "session_id": &id.0 })
                            .session(&mut *session)
                            .await?;
                        Ok(())
                    }
                    .boxed()
                },
            )
            .await
            .map_err(|error| {
                StoreError::Unavailable(format!("delete session transaction: {error}"))
            })
    }

    async fn write_receipt(
        &self,
        write_id: &str,
        id: &SessionId,
    ) -> Result<Option<u64>, StoreError> {
        if self.is_tombstoned(id).await? {
            return Err(StoreError::SessionDeleted { id: id.clone() });
        }
        let found = self
            .writes
            .find_one(doc! { "_id": write_id })
            .await
            .map_err(|error| {
                StoreError::Unavailable(format!("read session write receipt: {error}"))
            })?;
        let Some(receipt) = found else {
            return Ok(None);
        };
        receipt_revision(&receipt, write_id, id).map(Some)
    }

    /// The highest event sequence currently readable for one session.
    async fn last_event_sequence(&self, id: &SessionId) -> Result<Option<u64>, StoreError> {
        let found = self
            .events
            .find_one(doc! { "session_id": &id.0 })
            .sort(doc! { "seq": -1 })
            .projection(doc! { "_id": 0, "seq": 1 })
            .await
            .map_err(|e| StoreError::Unavailable(format!("read event tail: {e}")))?;
        stored_event_tail(found)
    }

    async fn query(&self, filter: Document) -> Result<Vec<SessionRecord>, StoreError> {
        let mut cursor = self
            .sessions
            .aggregate(visible_sessions_pipeline(filter))
            .await
            .map_err(|e| StoreError::Unavailable(format!("list: {e}")))?;
        let mut out = Vec::new();
        while let Some(d) = cursor
            .try_next()
            .await
            .map_err(|e| StoreError::Unavailable(format!("list cursor: {e}")))?
        {
            let stored = mongodb::bson::deserialize_from_document(d)
                .map_err(|e| StoreError::Malformed(format!("session document: {e}")))?;
            out.push(from_doc(stored)?);
        }
        Ok(out)
    }

    async fn is_tombstoned(&self, id: &SessionId) -> Result<bool, StoreError> {
        self.tombstones
            .find_one(doc! { "_id": &id.0 })
            .await
            .map(|found| found.is_some())
            .map_err(|error| {
                StoreError::Unavailable(format!("read session deletion fence: {error}"))
            })
    }
}

#[cfg(test)]
#[path = "mongo_tests.rs"]
mod tests;

//! Session records and event history, in MongoDB.
//!
//! Sessions, events, and whole-write receipts live in three collections.
//! Events carry a compound unique index on `(session_id, seq)`, which is what
//! makes the per-session sequence a guarantee rather than a hope: a duplicate
//! sequence is rejected by the database, not merely unlikely.

use std::{
    collections::{BTreeMap, HashSet},
    future::Future,
    sync::Arc,
    time::{Duration, Instant},
};

use futures_util::{FutureExt, TryStreamExt};
use layer2_core::{
    store::{
        Page, PageCursor, PendingHandover, SessionListQuery, SessionRecord, SessionStore,
        SkillPack, SkillPackDigest, SkillPackState, StoreError,
    },
    ChatRuntimeTransition, ConversationLink, ConversationOpeningLease,
    ConversationOpeningLeaseRequest, ConversationWriteAuthority, Event, EventKind, Rate,
    RateCardId, Reserved, RuntimeAuthority, RuntimeAuthorityRetirement, RuntimeAuthoritySwitch,
    RuntimeCandidate, RuntimeCleanup, RuntimeReservation, SessionBoundary, SessionId,
    SessionInteractionLeaseRequest, SessionLease, SessionLeaseRequest, SessionPricing,
    SessionStatus, StopReason, StoredEvent, SubjectId,
};
use mongodb::bson::{doc, Binary, Bson, Document};
use mongodb::options::{
    Hint, IndexOptions, InsertOneOptions, ReadConcern, ReplaceOptions, UpdateOptions,
    ValidationAction, ValidationLevel, WriteConcern,
};
use mongodb::results::{CollectionSpecification, CollectionType};
use mongodb::{Client, Collection, Database, IndexModel};
use serde::{Deserialize, Serialize};

use super::event_doc;

mod coordination;

use coordination::{
    all_session_receipts_filter, apply_chat_runtime_transition, coordination_clock_update,
    has_session_lease, require_conversation_write_authority, runtime_authority_from_doc,
    runtime_authority_from_projection, validate_coordination_state,
    validate_session_coordination_receipts, CoordinationCommand, CoordinationDoc,
    CoordinationOutcome, CoordinationReceiptDoc, RuntimeAuthorityProjectionDoc,
    COORDINATION_RECEIPTS,
};

const LEGACY_SESSIONS: &str = "sessions";
const LEGACY_EVENTS: &str = "session_events";
const PRE_COORDINATION_SESSIONS: &str = "sessions_v2";
const SESSIONS: &str = "sessions_v3";
const EVENTS: &str = "session_events_v2";
const STORE_PING_TIMEOUT: Duration = Duration::from_secs(2);
const STORE_OPEN_TIMEOUT: Duration = Duration::from_secs(5);
const HISTORY_AUDIT_INTERVAL: Duration = Duration::from_secs(30);
const WRITES: &str = "session_writes";
const TOMBSTONES: &str = "session_tombstones";
const LEGACY_CUTOVER_SOURCES: [&str; 3] = [LEGACY_SESSIONS, LEGACY_EVENTS, TOMBSTONES];
const LEGACY_CUTOVER_COLLECTIONS: [&str; 3] = [PRE_COORDINATION_SESSIONS, EVENTS, TOMBSTONES];
const AUTHORITATIVE_COLLECTIONS: [&str; 5] =
    [SESSIONS, EVENTS, WRITES, TOMBSTONES, COORDINATION_RECEIPTS];
const TRANSACTION_PROBE: &str = "_cma_transaction_probe";
const NAMESPACE_CUTOVERS: &str = "_cma_session_namespace_cutovers";
const NAMESPACE_CUTOVER_ID: &str = "sessions-v2";
const NAMESPACE_CUTOVER_PROTOCOL: i32 = 1;
const NAMESPACE_CUTOVER_OBSERVED_ORIGIN: &str = "observed-complete";
// Keep the marker identity stable so older binaries fail closed when its
// protocol advances instead of treating a newer activation as another database.
const COORDINATION_ACTIVATION_ID: &str = "session-coordination-v1";
const INITIAL_COORDINATION_ACTIVATION_PROTOCOL: i32 = 1;
const LEGACY_COORDINATION_ACTIVATION_PROTOCOL: i32 = 2;
const COORDINATION_ACTIVATION_PROTOCOL: i32 = 3;
const COORDINATION_RECEIPTS_PROVENANCE_ID: &str = "session-coordination-receipts-v1";
const COORDINATION_RECEIPTS_PROVENANCE_PROTOCOL: i32 = 1;
pub(super) const WRITE_PROTOCOL: i32 = 1;
const WRITE_PROTOCOL_FIELD: &str = "_cma_write_protocol";
/// Session-document writer that preserves explicit skill grant and custody ownership.
pub(super) const SESSION_WRITE_PROTOCOL: i32 = 2;
// This marker fences stale document writers after collMod, but Mongo validators
// cannot fence deletes or provider calls. Enabling coordination still requires
// an offline drain of every older coordination writer and its in-flight work.
#[cfg(test)]
pub(super) const LEGACY_SESSION_COORDINATION_PROTOCOL: i32 = 2;
pub(super) const SESSION_COORDINATION_PROTOCOL: i32 = 3;
#[cfg(test)]
pub(super) const LEGACY_COORDINATION_RECEIPT_PROTOCOL: i32 = 1;
pub(super) const COORDINATION_RECEIPT_PROTOCOL: i32 = 2;
const COORDINATION_PROTOCOL_FIELD: &str = "_cma_coordination_protocol";
/// Maximum BSON document containing temporary resolved skill bytes.
///
/// This is deliberately one quarter of MongoDB's 16-MiB document ceiling. The
/// staged state is a recovery bridge, not a second blob store, and the margin
/// covers the rest of the session plus BSON framing without relying on an
/// estimate of how binary values are encoded.
const MAX_STAGED_SESSION_DOCUMENT_BYTES: usize = 4 * 1024 * 1024;
const OWNER_SESSION_PAGE_INDEX: &str = "owner_1_created_at_-1__id_-1";
const OWNER_STATUS_SESSION_PAGE_INDEX: &str = "owner_1_status_1_created_at_-1__id_-1";
const ALL_SESSION_PAGE_INDEX: &str = "created_at_-1__id_-1";
const STATUS_SESSION_PAGE_INDEX: &str = "status_1_created_at_-1__id_-1";

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

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum SkillPackStateDoc {
    Staged {
        packs: Vec<SkillPackDoc>,
    },
    LegacyHandover {
        source_runtime_id: String,
        source_generation: i64,
        packs: Vec<SkillPackDoc>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stored_digest: Option<Binary>,
    },
    /// Listing-only representation; authoritative documents never store it.
    StagedSummary,
    Stored {
        digest: Binary,
    },
    CleanupPending,
    Cleaned,
}

impl std::fmt::Debug for SkillPackStateDoc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Staged { packs } => {
                let files = packs.iter().map(|pack| pack.files.len()).sum::<usize>();
                let bytes = packs
                    .iter()
                    .flat_map(|pack| &pack.files)
                    .map(|file| file.body.bytes.len())
                    .sum::<usize>();
                f.debug_struct("Staged")
                    .field("packs", &packs.len())
                    .field("files", &files)
                    .field("bytes", &bytes)
                    .finish()
            }
            Self::LegacyHandover {
                source_runtime_id,
                source_generation,
                packs,
                stored_digest,
            } => {
                let files = packs.iter().map(|pack| pack.files.len()).sum::<usize>();
                let bytes = packs
                    .iter()
                    .flat_map(|pack| &pack.files)
                    .map(|file| file.body.bytes.len())
                    .sum::<usize>();
                f.debug_struct("LegacyHandover")
                    .field("source_runtime_id", source_runtime_id)
                    .field("source_generation", source_generation)
                    .field("packs", &packs.len())
                    .field("files", &files)
                    .field("bytes", &bytes)
                    .field("stored_digest", &stored_digest.is_some())
                    .finish()
            }
            Self::StagedSummary => f.write_str("StagedSummary"),
            Self::Stored { .. } => f.write_str("Stored"),
            Self::CleanupPending => f.write_str("CleanupPending"),
            Self::Cleaned => f.write_str("Cleaned"),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct SkillPackDoc {
    name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    files: Vec<SkillPackFileDoc>,
}

#[derive(Clone, Serialize, Deserialize)]
struct SkillPackFileDoc {
    path: String,
    body: Binary,
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
    /// Absent until durable runtime coordination writers were fenced in.
    #[serde(rename = "_cma_coordination_protocol", default)]
    coordination_protocol: i32,
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
    /// Internal Trigger provenance. Absent for every ordinary/legacy session.
    #[serde(default)]
    trigger_origin: Option<layer2_core::TriggerSessionOrigin>,
    /// Internal shared-host provenance. Public chat facts are derived on read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    chat_origin: Option<layer2_core::ChatOrigin>,
    /// Internal initial-prompt outbox. Absent on ordinary and legacy sessions.
    #[serde(default)]
    initial_prompt: Option<layer2_core::InitialPromptDispatch>,
    /// Absent on records written before durable handover claims existed.
    #[serde(default)]
    rotation_claim: Option<String>,
    /// Recoverable snapshot and target for an interrupted runtime handover.
    ///
    /// Compatibility-only metadata; it does not change Mongo's coordinated
    /// writer protocol, whose handovers remain fenced by runtime authority.
    ///
    /// Optional for documents written before durable snapshot recovery. It is
    /// intentionally independent of `rotation_claim`: a reversible stop may
    /// retain the snapshot for a later revive after its live claim is cleared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pending_handover: Option<PendingHandover>,
    /// Exact resolved skill bytes and their object-store handoff state.
    ///
    /// Absent means the session predates durable custody. An explicit empty
    /// staged list remains distinct: it is the exact grant of no skills.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    skill_packs: Option<SkillPackStateDoc>,
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
    /// Runtime authority and the one active provider-mutation lease.
    ///
    /// Absent records predate durable coordination and begin at generation zero.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    coordination: Option<CoordinationDoc>,
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
    receipts: Collection<CoordinationReceiptDoc>,
    collection_ids: Arc<BTreeMap<String, Binary>>,
    integrity: Arc<tokio::sync::Mutex<IntegrityAudit>>,
    readiness_probe: Arc<tokio::sync::Semaphore>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct HistorySnapshot {
    tombstones: HashSet<String>,
    session_tails: BTreeMap<String, u64>,
    event_tails: BTreeMap<String, u64>,
}

#[derive(Debug, Default)]
struct IntegrityAudit {
    checked_at: Option<Instant>,
    last_error: Option<StoreError>,
    snapshot: Option<HistorySnapshot>,
}

#[derive(Debug, Clone, PartialEq)]
struct CollectionContract {
    kind: NamespaceKind,
    validator: Option<Document>,
    validation_level: Option<ValidationLevel>,
    validation_action: Option<ValidationAction>,
    uuid: Option<Binary>,
}

#[derive(Debug, Clone)]
struct RequiredIndex {
    collection: &'static str,
    name: &'static str,
    keys: Document,
    unique: bool,
}

/// Proof that the driver completed and aborted a real transaction.
struct TransactionQualified;

impl TransactionQualified {
    fn authorize(&self, action: NamespaceAction) -> NamespaceAction {
        action
    }
}

/// Proof that this namespace state is either new, complete, or explicitly
/// authorized after every legacy writer was stopped and drained.
#[derive(Debug)]
struct NamespaceCutoverAuthorized;

trait NamespaceMutationAuthorized {
    fn authorize(&self, action: NamespaceAction) -> NamespaceAction {
        action
    }
}

impl NamespaceMutationAuthorized for NamespaceCutoverAuthorized {}

/// Proof that the irreversible coordination activation was durably recorded
/// before the pre-coordination session namespace was changed.
#[derive(Debug)]
struct CoordinationActivationAuthorized;

impl NamespaceMutationAuthorized for CoordinationActivationAuthorized {}

#[derive(Debug, Clone, PartialEq, Eq)]
enum NamespaceCutoverAuthorization {
    Complete,
    Proven,
    RecordNewDatabase,
    RecordOperatorDrain,
    RecordCompletion {
        origin: String,
        collection_ids: BTreeMap<String, Binary>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum NamespaceCutoverMarker {
    InProgress {
        origin: String,
        source_ids: BTreeMap<String, Option<Binary>>,
        tombstone_id: Option<Binary>,
    },
    Complete {
        origin: String,
        collection_ids: BTreeMap<String, Binary>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum LegacyCutoverStageSource {
    Migrate,
    Complete(BTreeMap<String, Binary>),
}

// The receipt UUID is anchored in NAMESPACE_CUTOVERS. Administrative deletion
// of both that provenance namespace and the receipt collection is outside the
// recoverable fault model, just as it is for the existing session cutover proof.
#[derive(Debug, Clone, PartialEq, Eq)]
enum CoordinationReceiptsProvenance {
    InProgress,
    Complete { collection_id: Binary },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CoordinationActivationMarker {
    LegacyInProgress {
        protocol: i32,
        origin: String,
    },
    LegacyComplete {
        protocol: i32,
        origin: String,
        collection_ids: BTreeMap<String, Binary>,
    },
    LegacyUpgradeInProgress {
        protocol: i32,
        origin: String,
        collection_ids: BTreeMap<String, Binary>,
    },
    InProgress {
        origin: String,
    },
    UpgradeInProgress {
        origin: String,
        collection_ids: BTreeMap<String, Binary>,
    },
    Complete {
        origin: String,
        collection_ids: BTreeMap<String, Binary>,
    },
}

fn authorize_coordination_protocol_upgrade(
    marker: CoordinationActivationMarker,
    _coordination_cutover_confirmed: bool,
) -> Result<CoordinationActivationMarker, StoreError> {
    match marker {
        CoordinationActivationMarker::LegacyInProgress { protocol, .. }
        | CoordinationActivationMarker::LegacyComplete { protocol, .. }
        | CoordinationActivationMarker::LegacyUpgradeInProgress { protocol, .. } => {
            Err(StoreError::Unavailable(format!(
                "MongoDB coordination protocol-{protocol} activation cannot be upgraded to protocol-{COORDINATION_ACTIVATION_PROTOCOL}: protocols 1 and 2 could switch runtime authority without recording the exact event boundary, so their markerless runtime-switch boundaries and historical usage cannot be reconstructed safely; CHRONO_MONGODB_COORDINATION_CUTOVER=drained only proves writers stopped and cannot restore that accounting boundary; reset or restore this unreleased experimental database to a fresh or pre-coordination state"
            )))
        }
        CoordinationActivationMarker::UpgradeInProgress { .. } => {
            Err(StoreError::Unavailable(format!(
                "MongoDB coordination protocol-{COORDINATION_ACTIVATION_PROTOCOL} upgrade-in-progress provenance cannot be resumed: this marker can be residue from the unreleased protocol-1 or protocol-2 upgrade path and does not prove a reconstructable runtime-switch boundary; refusing to mutate its marker, validators, or documents; reset or restore the experimental database to a fresh or pre-coordination state"
            )))
        }
        current @ (CoordinationActivationMarker::InProgress { .. }
        | CoordinationActivationMarker::Complete { .. }) => Ok(current),
    }
}

struct EventTransaction {
    events_collection: Collection<Document>,
    sessions_collection: Collection<SessionDoc>,
    writes_collection: Collection<SessionWriteDoc>,
    tombstones_collection: Collection<SessionTombstoneDoc>,
    id: SessionId,
    write_id: String,
    create: bool,
    conversation_authority: Option<ConversationWriteAuthority>,
    chat_runtime_transition: Option<ChatRuntimeTransition>,
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
    legacy_marked: bool,
    authoritative_marked: bool,
    legacy_fenced: bool,
    authoritative_fenced: bool,
    legacy_uuid: Option<Binary>,
    authoritative_uuid: Option<Binary>,
}

#[derive(Debug, Default)]
struct StoredEventSequence {
    session_id: Option<String>,
    next: u64,
}

#[derive(Debug)]
struct CutoverEvent {
    session_id: String,
    seq: u64,
    position: Document,
    document: Document,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NamespaceAction {
    CreateAuthoritative,
    FenceLegacy,
    RenameLegacy,
    FenceAuthoritative,
    CreateLegacyView,
    FenceLateLegacy,
    MergeLateLegacy,
    Ready,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionWriteContract {
    Current,
    Previous,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CutoverOwnerLookup {
    Retry,
    Present,
    Absent,
}

impl MongoSessionStore {
    fn new(client: &Client, database: &str) -> Self {
        let database = client.database(database);
        Self {
            client: client.clone(),
            sessions: database.collection(SESSIONS),
            events: database.collection(EVENTS),
            writes: database.collection(WRITES),
            tombstones: database.collection(TOMBSTONES),
            receipts: database.collection(COORDINATION_RECEIPTS),
            database,
            collection_ids: Arc::new(BTreeMap::new()),
            integrity: Arc::new(tokio::sync::Mutex::new(IntegrityAudit::default())),
            readiness_probe: Arc::new(tokio::sync::Semaphore::new(1)),
        }
    }

    /// Open the store over an existing connection, ensuring its indexes exist.
    ///
    /// Takes a client rather than a URI so one connection serves every store
    /// this deployment keeps in the same database. See `super::connect`.
    pub(super) async fn open(
        client: &Client,
        database: &str,
        legacy_cutover_confirmed: bool,
        coordination_cutover_confirmed: bool,
    ) -> Result<Self, StoreError> {
        let mut store = Self::new(client, database);
        // This observation must precede every collection or provenance write.
        // An empty session namespace inside an otherwise established database
        // is not enough to prove that no pre-coordination process can return.
        let database_was_pristine = store.coordination_database_is_pristine().await?;
        store
            .validate_transaction_topology(STORE_OPEN_TIMEOUT)
            .await?;
        let qualification = store.qualify_transaction(STORE_OPEN_TIMEOUT).await?;
        let initial_activation = store.coordination_activation_marker().await?;
        let cutover_collection_ids = if initial_activation.is_some() {
            // An activation proof means the v1-to-v2 cutover already completed.
            // Its session collection may since have been renamed to v3, so the
            // old mutation pipeline must not try to reconstruct the v2 shape.
            store.completed_legacy_cutover_ids_from_marker().await?
        } else {
            store
                .migrate_legacy_namespaces(&qualification, legacy_cutover_confirmed)
                .await?
        };

        let activation = match initial_activation {
            Some(marker) => {
                authorize_coordination_protocol_upgrade(marker, coordination_cutover_confirmed)?
            }
            None => {
                store
                    .authorize_coordination_activation(
                        database_was_pristine,
                        coordination_cutover_confirmed,
                    )
                    .await?
            }
        };
        let namespace_activation_in_progress =
            matches!(activation, CoordinationActivationMarker::InProgress { .. });
        if namespace_activation_in_progress {
            store
                .verify_coordination_migration_source(&cutover_collection_ids)
                .await?;
            let activation = CoordinationActivationAuthorized;
            store
                .ensure_coordination_namespace(&qualification, &activation)
                .await?;
            let coordination_receipts_id = store.prepare_coordination_receipts_provenance().await?;
            store.ensure_indexes().await?;
            store.install_protocol_validators().await?;
            store.migrate_session_coordination_protocol().await?;
            store
                .complete_coordination_receipts_provenance(&coordination_receipts_id)
                .await?;
        }
        let collection_ids = store.authoritative_collection_ids().await?;
        validate_migrated_namespace_ids(&cutover_collection_ids, &collection_ids)?;
        if !namespace_activation_in_progress {
            // A completed activation marker is the authority for the protected
            // UUIDs. Verify it before the session validator can be changed.
            store
                .verify_coordination_activation(&collection_ids)
                .await?;
        }
        store.verify_coordinated_namespaces().await?;
        store
            .verify_coordination_receipts_provenance_ids(&collection_ids)
            .await?;
        store
            .verify_non_session_collection_contracts(Some(&collection_ids))
            .await?;
        store.verify_required_indexes().await?;
        store
            .ensure_session_write_protocol(
                coordination_cutover_confirmed,
                required_activation_uuid(&collection_ids, SESSIONS)?,
            )
            .await?;
        store
            .verify_collection_contracts(Some(&collection_ids))
            .await?;
        store.validate_coordination_receipts().await?;
        let snapshot = store.validate_post_fence_state().await?;
        if namespace_activation_in_progress {
            store
                .complete_coordination_activation(&collection_ids)
                .await?;
        }
        store
            .verify_coordination_activation(&collection_ids)
            .await?;
        {
            let mut integrity = store.integrity.lock().await;
            integrity.checked_at = Some(Instant::now());
            integrity.snapshot = Some(snapshot);
        }
        store.collection_ids = Arc::new(collection_ids);
        tracing::info!(database, "session store ready");
        Ok(store)
    }

    /// Migrate only the legacy namespace and stop at the pre-coordination boundary.
    pub(super) async fn stage_legacy_cutover(
        client: &Client,
        database: &str,
    ) -> Result<(), StoreError> {
        let store = Self::new(client, database);
        store
            .validate_transaction_topology(STORE_OPEN_TIMEOUT)
            .await?;
        // Reject protected state before even the read-only transaction probe;
        // the stage should not enter its migration path for an inadmissible DB.
        if let LegacyCutoverStageSource::Complete(collection_ids) =
            store.verify_legacy_cutover_stage_source().await?
        {
            store.verify_staged_legacy_cutover(&collection_ids).await?;
            tracing::info!(
                database,
                "legacy MongoDB namespace cutover already verified"
            );
            return Ok(());
        }
        let qualification = store.qualify_transaction(STORE_OPEN_TIMEOUT).await?;
        // Re-read after qualification so the migration still starts from the
        // exact protected state that was accepted before the transaction probe.
        if let LegacyCutoverStageSource::Complete(collection_ids) =
            store.verify_legacy_cutover_stage_source().await?
        {
            store.verify_staged_legacy_cutover(&collection_ids).await?;
            tracing::info!(
                database,
                "legacy MongoDB namespace cutover already verified"
            );
            return Ok(());
        }
        // Make the stage's first durable write its exact operator-drained
        // provenance, then re-read it before the shared migration can touch a
        // protected namespace. A competing marker with another origin fails.
        store
            .record_namespace_cutover_marker("operator-drained")
            .await?;
        if let LegacyCutoverStageSource::Complete(collection_ids) =
            store.verify_legacy_cutover_stage_source().await?
        {
            store.verify_staged_legacy_cutover(&collection_ids).await?;
            tracing::info!(
                database,
                "legacy MongoDB namespace cutover already verified"
            );
            return Ok(());
        }
        let collection_ids = store
            .migrate_legacy_namespaces(&qualification, true)
            .await?;
        store.verify_staged_legacy_cutover(&collection_ids).await?;
        tracing::info!(database, "legacy MongoDB namespace cutover verified");
        Ok(())
    }

    async fn migrate_legacy_namespaces(
        &self,
        qualification: &TransactionQualified,
        legacy_cutover_confirmed: bool,
    ) -> Result<BTreeMap<String, Binary>, StoreError> {
        let cutover = self
            .authorize_namespace_cutover(legacy_cutover_confirmed)
            .await?;
        self.prepare_cutover_tombstone_collection().await?;
        // Legacy flushes mutate the session record before appending events,
        // and legacy deletes remove the session before removing its events.
        // Fence session mutations before moving either protected history.
        self.prepare_legacy_session_fence(qualification, &cutover)
            .await?;
        self.capture_legacy_events(qualification, &cutover).await?;
        self.verify_legacy_writers_drained().await?;
        self.ensure_versioned_namespace(qualification, &cutover, LEGACY_EVENTS, EVENTS)
            .await?;
        self.reconcile_cutover_deletes().await?;
        self.ensure_versioned_namespace(
            qualification,
            &cutover,
            LEGACY_SESSIONS,
            PRE_COORDINATION_SESSIONS,
        )
        .await?;
        // Once the view exists no old delete can cross this boundary, so a
        // second pass closes the last session-before-events delete race.
        self.reconcile_cutover_deletes().await?;
        self.finalize_namespace_cutover_marker().await
    }

    /// Authorize only an untouched raw source or this stage's own durable resume state.
    async fn verify_legacy_cutover_stage_source(
        &self,
    ) -> Result<LegacyCutoverStageSource, StoreError> {
        self.verify_stage_coordination_namespaces_absent().await?;
        let sessions = self
            .namespace_pair(LEGACY_SESSIONS, PRE_COORDINATION_SESSIONS)
            .await?;
        let events = self.namespace_pair(LEGACY_EVENTS, EVENTS).await?;
        let contracts = self.legacy_stage_namespace_contracts().await?;
        let source = match self.legacy_stage_namespace_marker().await? {
            None => {
                validate_raw_legacy_stage_source(&sessions, &events, &contracts)?;
                LegacyCutoverStageSource::Migrate
            }
            Some(NamespaceCutoverMarker::InProgress {
                origin,
                source_ids,
                tombstone_id,
            }) if origin == "operator-drained" =>
            {
                self.verify_in_progress_cutover_source(
                    &source_ids,
                    tombstone_id.as_ref(),
                    &sessions,
                    &events,
                )
                .await?;
                validate_legacy_stage_namespace_contracts(&sessions, &events, &contracts)?;
                next_namespace_action(&sessions, LEGACY_SESSIONS, PRE_COORDINATION_SESSIONS)?;
                next_namespace_action(&events, LEGACY_EVENTS, EVENTS)?;
                LegacyCutoverStageSource::Migrate
            }
            Some(NamespaceCutoverMarker::Complete {
                origin,
                collection_ids,
            }) if origin == "operator-drained" => {
                let actual = self.current_completed_namespace_ids().await?;
                validate_completed_cutover_ids(&collection_ids, &actual)?;
                LegacyCutoverStageSource::Complete(collection_ids)
            }
            Some(marker) => {
                return Err(StoreError::Unavailable(format!(
                    "legacy MongoDB cutover stage requires exact operator-drained provenance, found {marker:?}; refusing to mutate or adopt an unproved recovery boundary"
                )))
            }
        };
        if source == LegacyCutoverStageSource::Migrate {
            self.verify_legacy_stage_tombstones(true).await?;
            self.verify_stage_session_documents_are_pre_coordination()
                .await?;
        }
        Ok(source)
    }

    async fn verify_in_progress_cutover_source(
        &self,
        source_ids: &BTreeMap<String, Option<Binary>>,
        tombstone_id: Option<&Binary>,
        sessions: &NamespacePair,
        events: &NamespacePair,
    ) -> Result<(), StoreError> {
        validate_namespace_cutover_source_ids(source_ids, sessions, events)?;
        validate_in_progress_tombstone_binding(source_ids, tombstone_id)?;
        let current = self.current_cutover_tombstone_id().await?;
        match (tombstone_id, current.as_ref()) {
            (Some(expected), Some(actual)) if expected == actual => Ok(()),
            (Some(_), Some(_)) => Err(StoreError::Unavailable(format!(
                "MongoDB cutover collection {TOMBSTONES} UUID no longer matches its in-progress provenance; refusing replacement deletion state"
            ))),
            (Some(_), None) => Err(StoreError::Unavailable(format!(
                "MongoDB cutover collection {TOMBSTONES} disappeared after its UUID was durably recorded; refusing to recreate lost deletion state"
            ))),
            (None, None) => Ok(()),
            (None, Some(_)) => Err(StoreError::Unavailable(format!(
                "MongoDB cutover found {TOMBSTONES} after provenance recorded it absent but before its UUID was bound; refusing ambiguous DDL-crash or replacement state"
            ))),
        }
    }

    async fn legacy_stage_namespace_contracts(
        &self,
    ) -> Result<BTreeMap<String, CollectionContract>, StoreError> {
        let mut contracts = BTreeMap::new();
        for collection in [
            LEGACY_SESSIONS,
            PRE_COORDINATION_SESSIONS,
            LEGACY_EVENTS,
            EVENTS,
        ] {
            if let Some(contract) = self.collection_contract_for(collection).await? {
                contracts.insert(collection.to_owned(), contract);
            }
        }
        Ok(contracts)
    }

    async fn verify_stage_coordination_namespaces_absent(&self) -> Result<(), StoreError> {
        let sessions = self
            .namespace_pair(PRE_COORDINATION_SESSIONS, SESSIONS)
            .await?;
        if sessions.authoritative.is_some() {
            return Err(StoreError::Unavailable(format!(
                "legacy MongoDB cutover stage refuses an existing {SESSIONS} namespace"
            )));
        }
        let receipts = self
            .namespace_pair(COORDINATION_RECEIPTS, "_cma_unused_stage_receipt_target")
            .await?;
        if receipts.legacy.is_some() {
            return Err(StoreError::Unavailable(format!(
                "legacy MongoDB cutover stage refuses an existing {COORDINATION_RECEIPTS} namespace"
            )));
        }
        Ok(())
    }

    async fn legacy_stage_namespace_marker(
        &self,
    ) -> Result<Option<NamespaceCutoverMarker>, StoreError> {
        let Some(contract) = self.collection_contract_for(NAMESPACE_CUTOVERS).await? else {
            return Ok(None);
        };
        validate_stage_unvalidated_collection_contract(NAMESPACE_CUTOVERS, &contract)?;
        let mut cursor = self
            .database
            .collection::<Document>(NAMESPACE_CUTOVERS)
            .find(doc! {})
            .sort(doc! { "_id": 1 })
            .await
            .map_err(|error| {
                StoreError::Unavailable(format!(
                    "inspect staged MongoDB namespace-cutover provenance: {error}"
                ))
            })?;
        let Some(document) = cursor.try_next().await.map_err(|error| {
            StoreError::Unavailable(format!(
                "read staged MongoDB namespace-cutover provenance: {error}"
            ))
        })?
        else {
            return Err(StoreError::Unavailable(
                "legacy MongoDB cutover stage refuses an existing empty provenance namespace"
                    .into(),
            ));
        };
        if !matches!(document.get_str("_id"), Ok(id) if id == NAMESPACE_CUTOVER_ID) {
            return Err(StoreError::Unavailable(
                "legacy MongoDB cutover stage refuses an unproved document in its reserved provenance namespace"
                    .into(),
            ));
        }
        if cursor
            .try_next()
            .await
            .map_err(|error| {
                StoreError::Unavailable(format!(
                    "read staged MongoDB namespace-cutover provenance tail: {error}"
                ))
            })?
            .is_some()
        {
            return Err(StoreError::Unavailable(
                "legacy MongoDB cutover stage requires exactly one document in its reserved provenance namespace"
                    .into(),
            ));
        }
        validate_namespace_cutover_marker(document).map(Some)
    }

    async fn verify_legacy_stage_tombstones(&self, allow_absent: bool) -> Result<(), StoreError> {
        match self.collection_contract_for(TOMBSTONES).await? {
            None if allow_absent => return Ok(()),
            None => {
                return Err(StoreError::Unavailable(format!(
                    "legacy MongoDB cutover stage did not preserve {TOMBSTONES} as a collection"
                )))
            }
            Some(contract) => {
                validate_stage_unvalidated_collection_contract(TOMBSTONES, &contract)?;
            }
        }
        self.validated_tombstone_ids().await?;
        Ok(())
    }

    async fn verify_stage_session_documents_are_pre_coordination(&self) -> Result<(), StoreError> {
        for collection in [LEGACY_SESSIONS, PRE_COORDINATION_SESSIONS] {
            let coordinated = self
                .database
                .collection::<Document>(collection)
                .find_one(doc! { COORDINATION_PROTOCOL_FIELD: { "$exists": true } })
                .await
                .map_err(|error| {
                    StoreError::Unavailable(format!(
                        "inspect {collection} for coordination state before the staged legacy cutover: {error}"
                    ))
                })?;
            if coordinated.is_some() {
                return Err(StoreError::Unavailable(format!(
                    "legacy MongoDB cutover stage found a coordination-protocol document in {collection}; refusing to relabel coordinated data as a v2 recovery source"
                )));
            }
        }
        Ok(())
    }

    async fn verify_staged_legacy_cutover(
        &self,
        expected_ids: &BTreeMap<String, Binary>,
    ) -> Result<(), StoreError> {
        self.verify_stage_coordination_namespaces_absent().await?;
        self.verify_current_completed_namespace_ids(expected_ids)
            .await?;
        let marker = self.legacy_stage_namespace_marker().await?;
        let Some(NamespaceCutoverMarker::Complete {
            origin,
            collection_ids,
        }) = marker
        else {
            return Err(StoreError::Unavailable(
                "legacy MongoDB cutover stage did not leave complete namespace provenance".into(),
            ));
        };
        if origin != "operator-drained" {
            return Err(StoreError::Unavailable(format!(
                "legacy MongoDB cutover stage completion origin is {origin:?}, not operator-drained"
            )));
        }
        validate_completed_cutover_ids(expected_ids, &collection_ids)?;

        for collection in [PRE_COORDINATION_SESSIONS, EVENTS] {
            let contract = self
                .collection_contract_for(collection)
                .await?
                .ok_or_else(|| {
                    StoreError::Unavailable(format!(
                        "staged legacy MongoDB collection {collection} is missing"
                    ))
                })?;
            validate_collection_contract(collection, &contract, expected_ids.get(collection))?;
        }
        self.verify_legacy_stage_tombstones(false).await?;
        self.verify_stage_session_documents_are_pre_coordination()
            .await?;
        self.verify_legacy_writers_drained().await?;
        self.verify_legacy_pre_cutover_owners().await?;
        Ok(())
    }

    async fn validate_transaction_topology(&self, timeout: Duration) -> Result<(), StoreError> {
        let store = self.clone();
        bounded_store_check(
            "MongoDB topology validation",
            async move { store.validate_transaction_topology_unbounded().await },
            timeout,
        )
        .await
    }

    async fn validate_transaction_topology_unbounded(&self) -> Result<(), StoreError> {
        let hello = self.database.run_command(doc! { "hello": 1_i32 }).await;
        let reply = match hello {
            Ok(reply) => reply,
            Err(error) if command_error_code(&error) == Some(59) => {
                // `hello` replaced `isMaster` after transactions already
                // existed. Keep older transaction-capable replica sets usable.
                self.database
                    .run_command(doc! { "isMaster": 1_i32 })
                    .await
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

    async fn check_readiness(&self) -> Result<(), StoreError> {
        self.database
            .run_command(doc! { "ping": 1_i32 })
            .await
            .map_err(|error| StoreError::Unavailable(format!("ping: {error}")))?;
        self.validate_transaction_topology_unbounded().await?;
        self.verify_versioned_namespaces().await?;
        self.verify_coordination_activation(self.collection_ids.as_ref())
            .await?;
        self.verify_coordination_receipts_provenance_ids(self.collection_ids.as_ref())
            .await?;
        self.verify_collection_contracts(Some(self.collection_ids.as_ref()))
            .await?;
        self.verify_required_indexes().await?;
        self.audit_integrity_if_due().await
    }

    async fn qualify_transaction(
        &self,
        timeout: Duration,
    ) -> Result<TransactionQualified, StoreError> {
        let client = self.client.clone();
        let probe = self.database.collection::<Document>(TRANSACTION_PROBE);
        bounded_store_check(
            "MongoDB transaction qualification",
            async move {
                let mut session = client.start_session().await.map_err(|error| {
                    StoreError::Unavailable(format!(
                        "start MongoDB transaction qualification session: {error}"
                    ))
                })?;
                session
                    .start_transaction()
                    .read_concern(ReadConcern::snapshot())
                    .write_concern(WriteConcern::majority())
                    .await
                    .map_err(|error| {
                        StoreError::Unavailable(format!(
                            "start MongoDB transaction qualification: {error}"
                        ))
                    })?;

                // Starting a driver transaction is local. The first operation is
                // what proves that the selected server accepts transaction commands.
                let operation = probe.find_one(doc! {}).session(&mut session).await;
                let abort = session.abort_transaction().await;
                operation.map_err(|error| {
                    StoreError::Unavailable(format!(
                        "execute MongoDB transaction qualification: {error}"
                    ))
                })?;
                abort.map_err(|error| {
                    StoreError::Unavailable(format!(
                        "abort MongoDB transaction qualification: {error}"
                    ))
                })?;
                Ok(())
            },
            timeout,
        )
        .await?;
        Ok(TransactionQualified)
    }

    async fn coordination_database_is_pristine(&self) -> Result<bool, StoreError> {
        let mut collections = self.database.list_collections().await.map_err(|error| {
            StoreError::Unavailable(format!(
                "inspect MongoDB database before coordination activation: {error}"
            ))
        })?;
        collections
            .try_next()
            .await
            .map(|collection| collection.is_none())
            .map_err(|error| {
                StoreError::Unavailable(format!(
                    "inspect MongoDB database namespace cursor before coordination activation: {error}"
                ))
            })
    }

    async fn coordination_activation_marker(
        &self,
    ) -> Result<Option<CoordinationActivationMarker>, StoreError> {
        let marker = self
            .database
            .collection::<Document>(NAMESPACE_CUTOVERS)
            .find_one(doc! { "_id": COORDINATION_ACTIVATION_ID })
            .await
            .map_err(|error| {
                StoreError::Unavailable(format!(
                    "read MongoDB coordination-activation provenance: {error}"
                ))
            })?;
        marker
            .map(validate_coordination_activation_marker)
            .transpose()
    }

    async fn completed_legacy_cutover_ids_from_marker(
        &self,
    ) -> Result<BTreeMap<String, Binary>, StoreError> {
        match self.namespace_cutover_marker().await? {
            Some(NamespaceCutoverMarker::Complete { collection_ids, .. }) => {
                let expected = required_cutover_uuid(&collection_ids, TOMBSTONES)?;
                match self.current_cutover_tombstone_id().await? {
                    Some(actual) if &actual == expected => Ok(collection_ids),
                    Some(_) => Err(StoreError::Unavailable(format!(
                        "completed MongoDB namespace cutover {TOMBSTONES} UUID no longer matches its durable completion record; refusing replacement deletion state"
                    ))),
                    None => Err(StoreError::Unavailable(format!(
                        "completed MongoDB namespace cutover no longer has protected collection {TOMBSTONES}; refusing to recreate lost deletion state"
                    ))),
                }
            }
            Some(NamespaceCutoverMarker::InProgress { .. }) => Err(StoreError::Unavailable(
                "MongoDB coordination activation requires the legacy namespace cutover provenance to be complete"
                    .into(),
            )),
            None => Err(StoreError::Unavailable(
                "MongoDB coordination activation exists without the completed legacy namespace cutover provenance"
                    .into(),
            )),
        }
    }

    async fn authorize_coordination_activation(
        &self,
        database_was_pristine: bool,
        coordination_cutover_confirmed: bool,
    ) -> Result<CoordinationActivationMarker, StoreError> {
        self.completed_legacy_cutover_ids_from_marker().await?;
        if let Some(marker) = self.coordination_activation_marker().await? {
            return Ok(marker);
        }
        let origin =
            coordination_activation_origin(database_was_pristine, coordination_cutover_confirmed)?;

        for _ in 0..4 {
            if let Some(marker) = self.coordination_activation_marker().await? {
                return Ok(marker);
            }
            if let Err(error) = self
                .verify_markerless_coordination_activation_source()
                .await
            {
                // A concurrent, legitimate protocol-3 activation may have
                // installed its durable marker while this read-only proof was
                // running. Only that marker can supersede the source-shape
                // refusal; markerless coordinated state remains unauthorized.
                if let Some(marker) = self.coordination_activation_marker().await? {
                    return Ok(marker);
                }
                return Err(error);
            }
            if let Some(marker) = self.coordination_activation_marker().await? {
                return Ok(marker);
            }
            let marker = CoordinationActivationMarker::InProgress {
                origin: origin.to_owned(),
            };
            let document = coordination_activation_marker_document(&marker)?;
            let inserted = self
                .database
                .collection::<Document>(NAMESPACE_CUTOVERS)
                .insert_one(document)
                .with_options(
                    InsertOneOptions::builder()
                        .write_concern(WriteConcern::majority())
                        .build(),
                )
                .await;
            match inserted {
                Ok(_) => continue,
                Err(error) => {
                    if self.coordination_activation_marker().await?.is_some() {
                        continue;
                    }
                    return Err(StoreError::Unavailable(format!(
                        "record MongoDB coordination activation before changing {PRE_COORDINATION_SESSIONS}: {error}"
                    )));
                }
            }
        }
        Err(StoreError::Unavailable(
            "MongoDB coordination activation did not reach a stable authorized state".into(),
        ))
    }

    async fn verify_markerless_coordination_activation_source(&self) -> Result<(), StoreError> {
        let sessions = self
            .namespace_pair(PRE_COORDINATION_SESSIONS, SESSIONS)
            .await?;
        let receipts_provenance = self.coordination_receipts_provenance().await?;
        let receipts_id = self.current_coordination_receipts_id().await?;
        validate_markerless_coordination_activation_source(
            &sessions,
            receipts_provenance.is_some(),
            receipts_id.is_some(),
        )
    }

    async fn verify_coordination_migration_source(
        &self,
        cutover_collection_ids: &BTreeMap<String, Binary>,
    ) -> Result<(), StoreError> {
        let compatibility = self
            .namespace_pair(LEGACY_SESSIONS, PRE_COORDINATION_SESSIONS)
            .await?;
        if !namespace_is_empty_view(compatibility.legacy.as_ref(), PRE_COORDINATION_SESSIONS) {
            return Err(StoreError::Unavailable(format!(
                "MongoDB coordination activation requires {LEGACY_SESSIONS} to remain a read-only view of {PRE_COORDINATION_SESSIONS}"
            )));
        }

        let sessions = self
            .namespace_pair(PRE_COORDINATION_SESSIONS, SESSIONS)
            .await?;
        let session_id = coordination_migration_session_id(&sessions)?.clone();
        let events = self.namespace_pair(LEGACY_EVENTS, EVENTS).await?;
        if next_namespace_action(&events, LEGACY_EVENTS, EVENTS)? != NamespaceAction::Ready {
            return Err(StoreError::Unavailable(format!(
                "MongoDB coordination activation requires {LEGACY_EVENTS} to remain a read-only view of {EVENTS}"
            )));
        }
        let event_id = events.authoritative_uuid.clone().ok_or_else(|| {
            StoreError::Unavailable(format!(
                "MongoDB coordination activation cannot identify the {EVENTS} collection UUID"
            ))
        })?;
        validate_migrated_namespace_ids(
            cutover_collection_ids,
            &BTreeMap::from([
                (SESSIONS.to_owned(), session_id),
                (EVENTS.to_owned(), event_id),
            ]),
        )
    }

    async fn ensure_coordination_namespace(
        &self,
        qualification: &TransactionQualified,
        activation: &CoordinationActivationAuthorized,
    ) -> Result<(), StoreError> {
        self.ensure_versioned_namespace(
            qualification,
            activation,
            PRE_COORDINATION_SESSIONS,
            SESSIONS,
        )
        .await
    }

    async fn verify_coordinated_namespaces(&self) -> Result<(), StoreError> {
        let compatibility = self
            .namespace_pair(LEGACY_SESSIONS, PRE_COORDINATION_SESSIONS)
            .await?;
        if !namespace_is_empty_view(compatibility.legacy.as_ref(), PRE_COORDINATION_SESSIONS)
            || !namespace_is_empty_view(compatibility.authoritative.as_ref(), SESSIONS)
        {
            return Err(StoreError::Unavailable(format!(
                "MongoDB session compatibility chain must be {LEGACY_SESSIONS} -> {PRE_COORDINATION_SESSIONS} -> {SESSIONS}"
            )));
        }

        let sessions = self
            .namespace_pair(PRE_COORDINATION_SESSIONS, SESSIONS)
            .await?;
        // This check owns only the immutable view topology. The exact session
        // validator is classified separately after every protected UUID and
        // non-session contract has been verified, so a legitimate predecessor
        // reaches its explicit drain gate and an unknown contract fails closed
        // without being mistaken for an incomplete namespace migration.
        if !namespace_is_empty_view(sessions.legacy.as_ref(), SESSIONS)
            || !matches!(sessions.authoritative, Some(NamespaceKind::Collection))
        {
            return Err(StoreError::Unavailable(format!(
                "MongoDB pre-coordination namespace {PRE_COORDINATION_SESSIONS} is no longer a read-only view of {SESSIONS}"
            )));
        }
        let events = self.namespace_pair(LEGACY_EVENTS, EVENTS).await?;
        if next_namespace_action(&events, LEGACY_EVENTS, EVENTS)? != NamespaceAction::Ready {
            return Err(StoreError::Unavailable(format!(
                "MongoDB legacy namespace {LEGACY_EVENTS} is no longer a read-only view of {EVENTS}"
            )));
        }
        Ok(())
    }

    async fn current_coordination_activation_ids(
        &self,
    ) -> Result<BTreeMap<String, Binary>, StoreError> {
        let contracts = self.collection_contracts().await?;
        let mut ids = BTreeMap::new();
        for collection in [SESSIONS, COORDINATION_RECEIPTS] {
            let contract = contracts.get(collection).ok_or_else(|| {
                StoreError::Unavailable(format!(
                    "completed MongoDB coordination activation no longer has protected collection {collection}; refusing to recreate lost coordination state"
                ))
            })?;
            if contract.kind != NamespaceKind::Collection {
                return Err(StoreError::Unavailable(format!(
                    "completed MongoDB coordination activation namespace {collection} is no longer a collection"
                )));
            }
            let uuid = contract.uuid.clone().ok_or_else(|| {
                StoreError::Unavailable(format!(
                    "completed MongoDB coordination activation collection {collection} has no stable UUID"
                ))
            })?;
            validate_cutover_uuid(collection, &uuid)?;
            ids.insert(collection.to_owned(), uuid);
        }
        Ok(ids)
    }

    async fn complete_coordination_activation(
        &self,
        expected: &BTreeMap<String, Binary>,
    ) -> Result<(), StoreError> {
        let expected = coordination_activation_ids(expected)?;
        for _ in 0..4 {
            let actual = self.current_coordination_activation_ids().await?;
            validate_coordination_activation_ids(&expected, &actual)?;
            match self.coordination_activation_marker().await? {
                Some(CoordinationActivationMarker::Complete { collection_ids, .. }) => {
                    return validate_coordination_activation_ids(&expected, &collection_ids);
                }
                Some(marker @ CoordinationActivationMarker::InProgress { .. }) => {
                    let CoordinationActivationMarker::InProgress { origin } = &marker else {
                        unreachable!("only current initial activation reaches completion");
                    };
                    let complete = CoordinationActivationMarker::Complete {
                        origin: origin.clone(),
                        collection_ids: expected.clone(),
                    };
                    let replacement = coordination_activation_marker_document(&complete)?;
                    let filter = coordination_activation_marker_document(&marker)?;
                    let replaced = self
                        .database
                        .collection::<Document>(NAMESPACE_CUTOVERS)
                        .replace_one(filter, replacement)
                        .with_options(
                            ReplaceOptions::builder()
                                .upsert(false)
                                .write_concern(WriteConcern::majority())
                                .build(),
                        )
                        .await;
                    match replaced {
                        Ok(result) if result.matched_count == 1 => continue,
                        Ok(_) | Err(_) => continue,
                    }
                }
                Some(
                    CoordinationActivationMarker::LegacyInProgress { .. }
                    | CoordinationActivationMarker::LegacyComplete { .. }
                    | CoordinationActivationMarker::LegacyUpgradeInProgress { .. }
                    | CoordinationActivationMarker::UpgradeInProgress { .. },
                ) => {
                    return Err(StoreError::Unavailable(
                        format!(
                            "MongoDB coordination activation changed to an unauthorized legacy or upgrade-in-progress marker during protocol-{COORDINATION_ACTIVATION_PROTOCOL} completion"
                        ),
                    ));
                }
                None => {
                    return Err(StoreError::Unavailable(
                        "MongoDB coordination activation provenance disappeared before completion"
                            .into(),
                    ));
                }
            }
        }
        Err(StoreError::Unavailable(
            "MongoDB coordination activation did not reach a stable completed state".into(),
        ))
    }

    async fn verify_coordination_activation(
        &self,
        expected: &BTreeMap<String, Binary>,
    ) -> Result<(), StoreError> {
        let expected = coordination_activation_ids(expected)?;
        let Some(CoordinationActivationMarker::Complete { collection_ids, .. }) =
            self.coordination_activation_marker().await?
        else {
            return Err(StoreError::Unavailable(
                "MongoDB coordination activation provenance is not complete".into(),
            ));
        };
        validate_coordination_activation_ids(&expected, &collection_ids)?;
        let actual = self.current_coordination_activation_ids().await?;
        validate_coordination_activation_ids(&collection_ids, &actual)
    }

    async fn authorize_namespace_cutover(
        &self,
        legacy_cutover_confirmed: bool,
    ) -> Result<NamespaceCutoverAuthorized, StoreError> {
        let sessions = self
            .namespace_pair(LEGACY_SESSIONS, PRE_COORDINATION_SESSIONS)
            .await?;
        let events = self.namespace_pair(LEGACY_EVENTS, EVENTS).await?;
        let marker = self.namespace_cutover_marker().await?;
        self.verify_namespace_cutover_marker_source(marker.as_ref(), &sessions, &events)
            .await?;
        let authorization = match marker.as_ref() {
            // The completed marker was just checked against a fresh three-UUID
            // snapshot. Do not re-authorize it from the older namespace pairs
            // captured before that marker read.
            Some(NamespaceCutoverMarker::Complete { .. }) => {
                NamespaceCutoverAuthorization::Complete
            }
            _ => namespace_cutover_authorization(
                &sessions,
                &events,
                legacy_cutover_confirmed,
                marker.as_ref(),
            )?,
        };
        match authorization {
            NamespaceCutoverAuthorization::RecordNewDatabase => {
                self.record_namespace_cutover_marker("new-database").await?;
            }
            NamespaceCutoverAuthorization::RecordOperatorDrain => {
                self.record_namespace_cutover_marker("operator-drained")
                    .await?;
            }
            NamespaceCutoverAuthorization::RecordCompletion { .. } => {
                let collection_ids = self.current_completed_namespace_ids().await?;
                self.record_completed_namespace_cutover_marker(&collection_ids)
                    .await?;
            }
            NamespaceCutoverAuthorization::Complete | NamespaceCutoverAuthorization::Proven => {}
        }
        let sessions = self
            .namespace_pair(LEGACY_SESSIONS, PRE_COORDINATION_SESSIONS)
            .await?;
        let events = self.namespace_pair(LEGACY_EVENTS, EVENTS).await?;
        let marker = self.namespace_cutover_marker().await?.ok_or_else(|| {
            StoreError::Unavailable(
                "MongoDB namespace-cutover provenance disappeared before mutation authorization"
                    .into(),
            )
        })?;
        self.verify_namespace_cutover_marker_source(Some(&marker), &sessions, &events)
            .await?;
        Ok(NamespaceCutoverAuthorized)
    }

    async fn verify_namespace_cutover_marker_source(
        &self,
        marker: Option<&NamespaceCutoverMarker>,
        sessions: &NamespacePair,
        events: &NamespacePair,
    ) -> Result<(), StoreError> {
        match marker {
            Some(NamespaceCutoverMarker::InProgress {
                source_ids,
                tombstone_id,
                ..
            }) => {
                self.verify_in_progress_cutover_source(
                    source_ids,
                    tombstone_id.as_ref(),
                    sessions,
                    events,
                )
                .await
            }
            Some(NamespaceCutoverMarker::Complete { collection_ids, .. }) => {
                let actual = self
                    .current_completed_namespace_ids_with_incomplete_error(
                        "completed MongoDB namespace cutover is no longer complete; refusing to recreate missing namespaces from an empty state",
                    )
                    .await?;
                validate_completed_cutover_ids(collection_ids, &actual)
            }
            None => Ok(()),
        }
    }

    async fn namespace_cutover_marker(&self) -> Result<Option<NamespaceCutoverMarker>, StoreError> {
        let marker = self
            .database
            .collection::<Document>(NAMESPACE_CUTOVERS)
            .find_one(doc! { "_id": NAMESPACE_CUTOVER_ID })
            .await
            .map_err(|error| {
                StoreError::Unavailable(format!(
                    "read MongoDB namespace-cutover provenance: {error}"
                ))
            })?;
        marker.map(validate_namespace_cutover_marker).transpose()
    }

    async fn coordination_receipts_provenance(
        &self,
    ) -> Result<Option<CoordinationReceiptsProvenance>, StoreError> {
        let marker = self
            .database
            .collection::<Document>(NAMESPACE_CUTOVERS)
            .find_one(doc! { "_id": COORDINATION_RECEIPTS_PROVENANCE_ID })
            .await
            .map_err(|error| {
                StoreError::Unavailable(format!(
                    "read MongoDB coordination-receipt provenance: {error}"
                ))
            })?;
        marker
            .map(validate_coordination_receipts_provenance)
            .transpose()
    }

    async fn current_coordination_receipts_id(&self) -> Result<Option<Binary>, StoreError> {
        let contracts = self.collection_contracts().await?;
        let Some(contract) = contracts.get(COORDINATION_RECEIPTS) else {
            return Ok(None);
        };
        if contract.kind != NamespaceKind::Collection {
            return Err(StoreError::Unavailable(format!(
                "MongoDB coordination-receipt namespace {COORDINATION_RECEIPTS} is not a collection"
            )));
        }
        let id = contract.uuid.clone().ok_or_else(|| {
            StoreError::Unavailable(format!(
                "MongoDB coordination-receipt collection {COORDINATION_RECEIPTS} has no stable UUID"
            ))
        })?;
        validate_cutover_uuid(COORDINATION_RECEIPTS, &id)?;
        Ok(Some(id))
    }

    async fn prepare_coordination_receipts_provenance(&self) -> Result<Binary, StoreError> {
        for _ in 0..8 {
            let marker = self.coordination_receipts_provenance().await?;
            let actual = self.current_coordination_receipts_id().await?;
            match (marker, actual) {
                (
                    Some(CoordinationReceiptsProvenance::Complete { collection_id }),
                    Some(actual),
                ) if collection_id == actual => return Ok(actual),
                (
                    Some(CoordinationReceiptsProvenance::Complete { collection_id }),
                    Some(_),
                ) => {
                    return Err(StoreError::Unavailable(format!(
                        "MongoDB coordination-receipt collection {COORDINATION_RECEIPTS} UUID no longer matches its durable provenance; refusing replacement data (expected {collection_id:?})"
                    )))
                }
                (Some(CoordinationReceiptsProvenance::Complete { .. }), None) => {
                    return Err(StoreError::Unavailable(format!(
                        "MongoDB coordination-receipt collection {COORDINATION_RECEIPTS} is missing; refusing to recreate lost coordination state"
                    )))
                }
                (Some(CoordinationReceiptsProvenance::InProgress), Some(actual)) => {
                    return Ok(actual)
                }
                (Some(CoordinationReceiptsProvenance::InProgress), None) => {
                    let created = self
                        .database
                        .create_collection(COORDINATION_RECEIPTS)
                        .await;
                    if let Err(error) = created {
                        if self.current_coordination_receipts_id().await?.is_none() {
                            return Err(StoreError::Unavailable(format!(
                                "create MongoDB coordination-receipt collection after recording provenance intent: {error}"
                            )));
                        }
                    }
                }
                (None, Some(_)) => {
                    if self.coordination_receipts_provenance().await?.is_some() {
                        continue;
                    }
                    return Err(StoreError::Unavailable(format!(
                        "MongoDB coordination-receipt collection {COORDINATION_RECEIPTS} exists without durable provenance; refusing to adopt unknown coordination state"
                    )));
                }
                (None, None) => {
                    let marker = coordination_receipts_provenance_document(
                        &CoordinationReceiptsProvenance::InProgress,
                    )?;
                    let inserted = self
                        .database
                        .collection::<Document>(NAMESPACE_CUTOVERS)
                        .insert_one(marker)
                        .with_options(
                            InsertOneOptions::builder()
                                .write_concern(WriteConcern::majority())
                                .build(),
                        )
                        .await;
                    if let Err(error) = inserted {
                        if self.coordination_receipts_provenance().await?.is_none() {
                            return Err(StoreError::Unavailable(format!(
                                "record MongoDB coordination-receipt provenance before creating its collection: {error}"
                            )));
                        }
                    }
                }
            }
        }
        Err(StoreError::Unavailable(
            "MongoDB coordination-receipt provenance did not reach a stable prepared state".into(),
        ))
    }

    async fn complete_coordination_receipts_provenance(
        &self,
        expected: &Binary,
    ) -> Result<(), StoreError> {
        let contracts = self.collection_contracts().await?;
        let contract = contracts.get(COORDINATION_RECEIPTS).ok_or_else(|| {
            StoreError::Unavailable(format!(
                "MongoDB coordination-receipt collection {COORDINATION_RECEIPTS} disappeared before provenance completion"
            ))
        })?;
        validate_collection_contract(COORDINATION_RECEIPTS, contract, Some(expected))?;

        for _ in 0..4 {
            match self.coordination_receipts_provenance().await? {
                Some(CoordinationReceiptsProvenance::Complete { collection_id }) => {
                    return validate_exact_coordination_receipts_id(expected, &collection_id)
                }
                Some(CoordinationReceiptsProvenance::InProgress) => {
                    let completed = coordination_receipts_provenance_document(
                        &CoordinationReceiptsProvenance::Complete {
                            collection_id: expected.clone(),
                        },
                    )?;
                    let replaced = self
                        .database
                        .collection::<Document>(NAMESPACE_CUTOVERS)
                        .replace_one(
                            doc! {
                                "_id": COORDINATION_RECEIPTS_PROVENANCE_ID,
                                "protocol": COORDINATION_RECEIPTS_PROVENANCE_PROTOCOL,
                                "phase": "in-progress",
                            },
                            completed,
                        )
                        .with_options(
                            ReplaceOptions::builder()
                                .upsert(false)
                                .write_concern(WriteConcern::majority())
                                .build(),
                        )
                        .await;
                    match replaced {
                        Ok(result) if result.matched_count == 1 => continue,
                        Ok(_) | Err(_) => continue,
                    }
                }
                None => {
                    return Err(StoreError::Unavailable(
                        "MongoDB coordination-receipt provenance disappeared before completion"
                            .into(),
                    ))
                }
            }
        }
        Err(StoreError::Unavailable(
            "MongoDB coordination-receipt provenance did not reach a stable completed state".into(),
        ))
    }

    async fn verify_coordination_receipts_provenance_ids(
        &self,
        expected_ids: &BTreeMap<String, Binary>,
    ) -> Result<(), StoreError> {
        let expected = expected_ids.get(COORDINATION_RECEIPTS).ok_or_else(|| {
            StoreError::Unavailable(
                "MongoDB readiness has no coordination-receipt collection UUID".into(),
            )
        })?;
        let Some(CoordinationReceiptsProvenance::Complete { collection_id }) =
            self.coordination_receipts_provenance().await?
        else {
            return Err(StoreError::Unavailable(
                "MongoDB coordination-receipt provenance is not complete".into(),
            ));
        };
        validate_exact_coordination_receipts_id(expected, &collection_id)
    }

    async fn record_namespace_cutover_marker(&self, origin: &str) -> Result<(), StoreError> {
        if self.namespace_cutover_marker().await?.is_some() {
            return Ok(());
        }
        let sessions = self
            .namespace_pair(LEGACY_SESSIONS, PRE_COORDINATION_SESSIONS)
            .await?;
        let events = self.namespace_pair(LEGACY_EVENTS, EVENTS).await?;
        let tombstone_id = self.current_cutover_tombstone_id().await?;
        let source_ids = namespace_cutover_source_ids(&sessions, &events, tombstone_id.clone())?;
        validate_namespace_cutover_source_origin(origin, &source_ids)?;
        let marker = NamespaceCutoverMarker::InProgress {
            origin: origin.to_owned(),
            source_ids,
            tombstone_id,
        };
        let document = namespace_cutover_marker_document(&marker)?;
        let insert = self
            .database
            .collection::<Document>(NAMESPACE_CUTOVERS)
            .insert_one(document)
            .with_options(
                InsertOneOptions::builder()
                    .write_concern(WriteConcern::majority())
                    .build(),
            )
            .await;
        match insert {
            Ok(_) => Ok(()),
            Err(insert_error) => {
                if self.namespace_cutover_marker().await?.is_some() {
                    Ok(())
                } else {
                    Err(StoreError::Unavailable(format!(
                        "record MongoDB namespace-cutover provenance before mutating legacy namespaces: {insert_error}"
                    )))
                }
            }
        }
    }

    async fn current_cutover_tombstone_id(&self) -> Result<Option<Binary>, StoreError> {
        let Some(contract) = self.collection_contract_for(TOMBSTONES).await? else {
            return Ok(None);
        };
        if contract.kind != NamespaceKind::Collection {
            return Err(StoreError::Unavailable(format!(
                "MongoDB cutover requires {TOMBSTONES} to be a collection"
            )));
        }
        let uuid = contract.uuid.ok_or_else(|| {
            StoreError::Unavailable(format!(
                "MongoDB cutover collection {TOMBSTONES} has no stable UUID"
            ))
        })?;
        validate_cutover_uuid(TOMBSTONES, &uuid)?;
        Ok(Some(uuid))
    }

    async fn prepare_cutover_tombstone_collection(&self) -> Result<(), StoreError> {
        for _ in 0..8 {
            match self.namespace_cutover_marker().await? {
                Some(NamespaceCutoverMarker::Complete { collection_ids, .. }) => {
                    let actual = self.current_completed_namespace_ids().await?;
                    return validate_completed_cutover_ids(&collection_ids, &actual);
                }
                Some(NamespaceCutoverMarker::InProgress {
                    source_ids,
                    tombstone_id: Some(expected),
                    ..
                }) => {
                    validate_in_progress_tombstone_binding(&source_ids, Some(&expected))?;
                    let sessions = self
                        .namespace_pair(LEGACY_SESSIONS, PRE_COORDINATION_SESSIONS)
                        .await?;
                    let events = self.namespace_pair(LEGACY_EVENTS, EVENTS).await?;
                    validate_namespace_cutover_source_ids(&source_ids, &sessions, &events)?;
                    return match self.current_cutover_tombstone_id().await? {
                        Some(actual) if actual == expected => Ok(()),
                        Some(_) => Err(StoreError::Unavailable(format!(
                            "MongoDB cutover collection {TOMBSTONES} UUID no longer matches its in-progress provenance; refusing replacement deletion state"
                        ))),
                        None => Err(StoreError::Unavailable(format!(
                            "MongoDB cutover collection {TOMBSTONES} disappeared after its UUID was durably recorded; refusing to recreate lost deletion state"
                        ))),
                    };
                }
                Some(NamespaceCutoverMarker::InProgress {
                    origin,
                    source_ids,
                    tombstone_id: None,
                }) => {
                    validate_in_progress_tombstone_binding(&source_ids, None)?;
                    let sessions = self
                        .namespace_pair(LEGACY_SESSIONS, PRE_COORDINATION_SESSIONS)
                        .await?;
                    let events = self.namespace_pair(LEGACY_EVENTS, EVENTS).await?;
                    validate_namespace_cutover_source_ids(&source_ids, &sessions, &events)?;
                    if let Some(current) = self.current_cutover_tombstone_id().await? {
                        let expected_bound = NamespaceCutoverMarker::InProgress {
                            origin: origin.clone(),
                            source_ids: source_ids.clone(),
                            tombstone_id: Some(current),
                        };
                        match self.namespace_cutover_marker().await? {
                            Some(actual) if actual == expected_bound => continue,
                            Some(NamespaceCutoverMarker::Complete {
                                origin: completed_origin,
                                ..
                            }) if completed_origin == origin => continue,
                            _ => {}
                        }
                        return Err(StoreError::Unavailable(format!(
                            "MongoDB cutover found {TOMBSTONES} after provenance recorded it absent but before its UUID was bound; refusing ambiguous DDL-crash or replacement state"
                        )));
                    }

                    if let Err(error) = self.database.create_collection(TOMBSTONES).await {
                        if let Some(current) = self.current_cutover_tombstone_id().await? {
                            let expected_bound = NamespaceCutoverMarker::InProgress {
                                origin: origin.clone(),
                                source_ids: source_ids.clone(),
                                tombstone_id: Some(current),
                            };
                            match self.namespace_cutover_marker().await? {
                                Some(actual) if actual == expected_bound => continue,
                                Some(NamespaceCutoverMarker::Complete {
                                    origin: completed_origin,
                                    ..
                                }) if completed_origin == origin => continue,
                                _ => {}
                            }
                        }
                        return Err(StoreError::Unavailable(format!(
                            "create MongoDB cutover tombstone collection before binding its UUID: {error}"
                        )));
                    }
                    let created = self.current_cutover_tombstone_id().await?.ok_or_else(|| {
                        StoreError::Unavailable(format!(
                            "MongoDB created {TOMBSTONES} but its stable UUID is unavailable"
                        ))
                    })?;
                    let replacement_marker = NamespaceCutoverMarker::InProgress {
                        origin: origin.clone(),
                        source_ids: source_ids.clone(),
                        tombstone_id: Some(created.clone()),
                    };
                    let replacement = namespace_cutover_marker_document(&replacement_marker)?;
                    let source_document = namespace_cutover_source_document(&source_ids)?;
                    let updated = self
                        .database
                        .collection::<Document>(NAMESPACE_CUTOVERS)
                        .replace_one(
                            doc! {
                                "_id": NAMESPACE_CUTOVER_ID,
                                "protocol": NAMESPACE_CUTOVER_PROTOCOL,
                                "origin": &origin,
                                "phase": "in-progress",
                                "source_collection_uuids": source_document,
                                "tombstone_collection_uuid": Bson::Null,
                            },
                            replacement,
                        )
                        .with_options(
                            ReplaceOptions::builder()
                                .upsert(false)
                                .write_concern(WriteConcern::majority())
                                .build(),
                        )
                        .await;
                    match updated {
                        Ok(result) if result.matched_count == 1 => continue,
                        Ok(_) | Err(_) => {
                            match self.namespace_cutover_marker().await? {
                                Some(actual) if actual == replacement_marker => continue,
                                Some(NamespaceCutoverMarker::Complete {
                                    origin: completed_origin,
                                    ..
                                }) if completed_origin == origin => continue,
                                _ => {}
                            }
                            return Err(StoreError::Unavailable(format!(
                                "MongoDB cutover created {TOMBSTONES} but could not durably bind its UUID; refusing ambiguous deletion state"
                            )));
                        }
                    }
                }
                None => return Err(StoreError::Unavailable(
                    "MongoDB cutover tombstone preparation requires durable in-progress provenance"
                        .into(),
                )),
            }
        }
        Err(StoreError::Unavailable(format!(
            "MongoDB cutover collection {TOMBSTONES} did not reach a durably UUID-bound state"
        )))
    }

    async fn current_completed_namespace_ids(
        &self,
    ) -> Result<BTreeMap<String, Binary>, StoreError> {
        self.current_completed_namespace_ids_with_incomplete_error(
            "MongoDB namespace cutover cannot be marked complete until both legacy names are read-only views of their authoritative collections",
        )
        .await
    }

    async fn current_completed_namespace_ids_with_incomplete_error(
        &self,
        incomplete_error: &str,
    ) -> Result<BTreeMap<String, Binary>, StoreError> {
        let sessions = self
            .namespace_pair(LEGACY_SESSIONS, PRE_COORDINATION_SESSIONS)
            .await?;
        let events = self.namespace_pair(LEGACY_EVENTS, EVENTS).await?;
        let mut collection_ids = completed_namespace_ids(&sessions, &events)?
            .ok_or_else(|| StoreError::Unavailable(incomplete_error.to_owned()))?;
        let tombstones = self
            .collection_contract_for(TOMBSTONES)
            .await?
            .ok_or_else(|| {
                StoreError::Unavailable(format!(
                    "MongoDB namespace cutover cannot be marked complete without collection {TOMBSTONES}"
                ))
            })?;
        if tombstones.kind != NamespaceKind::Collection {
            return Err(StoreError::Unavailable(format!(
                "MongoDB namespace cutover requires {TOMBSTONES} to remain a collection"
            )));
        }
        let tombstone_id = tombstones.uuid.ok_or_else(|| {
            StoreError::Unavailable(format!(
                "MongoDB namespace cutover collection {TOMBSTONES} has no stable UUID"
            ))
        })?;
        validate_cutover_uuid(TOMBSTONES, &tombstone_id)?;
        collection_ids.insert(TOMBSTONES.to_owned(), tombstone_id);
        Ok(collection_ids)
    }

    async fn verify_current_completed_namespace_ids(
        &self,
        expected: &BTreeMap<String, Binary>,
    ) -> Result<(), StoreError> {
        let actual = self.current_completed_namespace_ids().await?;
        validate_completed_cutover_ids(expected, &actual)
    }

    async fn record_completed_namespace_cutover_marker(
        &self,
        expected: &BTreeMap<String, Binary>,
    ) -> Result<(), StoreError> {
        for _ in 0..4 {
            // Do not bless UUIDs from a stale namespace snapshot. Recheck the
            // complete pair immediately before each conditional marker write.
            self.verify_current_completed_namespace_ids(expected)
                .await?;
            match self.namespace_cutover_marker().await? {
                Some(NamespaceCutoverMarker::Complete { collection_ids, .. }) => {
                    return validate_completed_cutover_ids(expected, &collection_ids);
                }
                Some(NamespaceCutoverMarker::InProgress {
                    origin,
                    source_ids,
                    tombstone_id: Some(tombstone_id),
                }) => {
                    let sessions = self
                        .namespace_pair(LEGACY_SESSIONS, PRE_COORDINATION_SESSIONS)
                        .await?;
                    let events = self.namespace_pair(LEGACY_EVENTS, EVENTS).await?;
                    self.verify_in_progress_cutover_source(
                        &source_ids,
                        Some(&tombstone_id),
                        &sessions,
                        &events,
                    )
                    .await?;
                    let expected_tombstone = required_cutover_uuid(expected, TOMBSTONES)?;
                    if &tombstone_id != expected_tombstone {
                        return Err(StoreError::Unavailable(format!(
                            "MongoDB namespace cutover cannot complete because {TOMBSTONES} does not match its in-progress UUID binding"
                        )));
                    }
                    let marker = NamespaceCutoverMarker::Complete {
                        origin: origin.clone(),
                        collection_ids: expected.clone(),
                    };
                    self.verify_current_completed_namespace_ids(expected)
                        .await?;
                    let replacement = namespace_cutover_marker_document(&marker)?;
                    let source_document = namespace_cutover_source_document(&source_ids)?;
                    let updated = self
                        .database
                        .collection::<Document>(NAMESPACE_CUTOVERS)
                        .replace_one(
                            doc! {
                                "_id": NAMESPACE_CUTOVER_ID,
                                "protocol": NAMESPACE_CUTOVER_PROTOCOL,
                                "origin": &origin,
                                "phase": "in-progress",
                                "source_collection_uuids": source_document,
                                "tombstone_collection_uuid": tombstone_id,
                            },
                            replacement,
                        )
                        .with_options(
                            ReplaceOptions::builder()
                                .upsert(false)
                                .write_concern(WriteConcern::majority())
                                .build(),
                        )
                        .await;
                    match updated {
                        Ok(result) if result.matched_count == 1 => continue,
                        Ok(_) => continue,
                        Err(error) => {
                            if self.namespace_cutover_marker().await?.is_some() {
                                continue;
                            }
                            return Err(StoreError::Unavailable(format!(
                                "complete MongoDB namespace-cutover provenance: {error}"
                            )));
                        }
                    }
                }
                Some(NamespaceCutoverMarker::InProgress {
                    tombstone_id: None,
                    ..
                }) => {
                    return Err(StoreError::Unavailable(format!(
                        "MongoDB namespace cutover cannot complete before {TOMBSTONES} has an exact durable UUID binding"
                    )))
                }
                None => {
                    return Err(StoreError::Unavailable(
                        "MongoDB namespace cutover cannot complete without its durable in-progress provenance"
                            .into(),
                    ))
                }
            }
        }
        Err(StoreError::Unavailable(
            "MongoDB namespace-cutover provenance did not reach a stable completed state".into(),
        ))
    }

    async fn finalize_namespace_cutover_marker(
        &self,
    ) -> Result<BTreeMap<String, Binary>, StoreError> {
        let expected = self.current_completed_namespace_ids().await?;
        self.record_completed_namespace_cutover_marker(&expected)
            .await?;
        let Some(NamespaceCutoverMarker::Complete { collection_ids, .. }) =
            self.namespace_cutover_marker().await?
        else {
            return Err(StoreError::Unavailable(
                "MongoDB namespace-cutover provenance was not durably completed".into(),
            ));
        };
        validate_completed_cutover_ids(&expected, &collection_ids)?;
        self.verify_current_completed_namespace_ids(&collection_ids)
            .await?;
        Ok(collection_ids)
    }

    async fn prepare_legacy_session_fence(
        &self,
        qualification: &TransactionQualified,
        cutover: &NamespaceCutoverAuthorized,
    ) -> Result<(), StoreError> {
        for _ in 0..12 {
            let pair = self
                .namespace_pair(LEGACY_SESSIONS, PRE_COORDINATION_SESSIONS)
                .await?;
            match (&pair.legacy, &pair.authoritative) {
                (None, None) => {
                    self.verify_legacy_pre_cutover_owners().await?;
                    return Ok(());
                }
                (Some(NamespaceKind::Collection), None) if pair.legacy_fenced => return Ok(()),
                (Some(NamespaceKind::Collection), None) => {
                    self.verify_legacy_pre_cutover_owners().await?;
                    self.execute_namespace_action(
                        qualification,
                        cutover,
                        NamespaceAction::FenceLegacy,
                        LEGACY_SESSIONS,
                        PRE_COORDINATION_SESSIONS,
                    )
                    .await?;
                }
                (None, Some(NamespaceKind::Collection)) if pair.authoritative_fenced => {
                    return Ok(())
                }
                (None, Some(NamespaceKind::Collection)) => {
                    self.verify_legacy_pre_cutover_owners().await?;
                    self.execute_namespace_action(
                        qualification,
                        cutover,
                        NamespaceAction::FenceAuthoritative,
                        LEGACY_SESSIONS,
                        PRE_COORDINATION_SESSIONS,
                    )
                    .await?;
                }
                (Some(NamespaceKind::Collection), Some(NamespaceKind::Collection)) => {
                    return Err(StoreError::Unavailable(
                        "legacy MongoDB session collection reappeared after its authoritative namespace was established; this violates the drained-writer cutover precondition, so CMA will not merge or drop either collection"
                            .into(),
                    ))
                }
                (
                    Some(NamespaceKind::View {
                        source: Some(source),
                        pipeline_is_empty: true,
                    }),
                    Some(NamespaceKind::Collection),
                ) if source == PRE_COORDINATION_SESSIONS && pair.authoritative_fenced => {
                    return Ok(())
                }
                _ => {
                    return Err(StoreError::Unavailable(format!(
                    "MongoDB session namespace cannot establish the legacy-writer fence: {pair:?}"
                )))
                }
            }
        }
        Err(StoreError::Unavailable(
            "MongoDB session namespace did not reach a stable pre-cutover fence".into(),
        ))
    }

    async fn capture_legacy_events(
        &self,
        qualification: &TransactionQualified,
        cutover: &NamespaceCutoverAuthorized,
    ) -> Result<(), StoreError> {
        for _ in 0..12 {
            let pair = self.namespace_pair(LEGACY_EVENTS, EVENTS).await?;
            let action = match (&pair.legacy, &pair.authoritative) {
                (None, None) => NamespaceAction::CreateAuthoritative,
                (Some(NamespaceKind::Collection), None) if !pair.legacy_marked => {
                    self.install_namespace_marker(LEGACY_EVENTS, EVENTS).await?;
                    continue;
                }
                (Some(NamespaceKind::Collection), None) => NamespaceAction::RenameLegacy,
                (None, Some(NamespaceKind::Collection)) if !pair.authoritative_fenced => {
                    NamespaceAction::FenceAuthoritative
                }
                (None, Some(NamespaceKind::Collection)) => return Ok(()),
                (Some(NamespaceKind::Collection), Some(NamespaceKind::Collection))
                    if pair.authoritative_marked && !pair.authoritative_fenced =>
                {
                    NamespaceAction::FenceAuthoritative
                }
                (Some(NamespaceKind::Collection), Some(NamespaceKind::Collection))
                    if pair.authoritative_fenced =>
                {
                    return Ok(())
                }
                (
                    Some(NamespaceKind::View {
                        source: Some(source),
                        pipeline_is_empty: true,
                    }),
                    Some(NamespaceKind::Collection),
                ) if source == EVENTS && pair.authoritative_fenced => return Ok(()),
                _ => {
                    return Err(StoreError::Unavailable(format!(
                        "MongoDB event namespace cannot establish a protected cutover capture: {pair:?}"
                    )))
                }
            };
            self.execute_namespace_action(qualification, cutover, action, LEGACY_EVENTS, EVENTS)
                .await?;
        }
        Err(StoreError::Unavailable(
            "MongoDB event namespace did not reach a stable protected capture".into(),
        ))
    }

    async fn verify_legacy_pre_cutover_owners(&self) -> Result<(), StoreError> {
        let sessions = self.validated_cutover_session_tails().await?;
        let events = self.validated_cutover_event_tails().await?;
        let tombstones = self.validated_tombstone_ids().await?;
        validate_legacy_pre_cutover_owners(&sessions, &events, &tombstones)
    }

    async fn reconcile_cutover_deletes(&self) -> Result<(), StoreError> {
        let event_tails = self.validated_event_tails().await?;
        let tombstones = self.validated_tombstone_ids().await?;
        for id in event_tails.keys() {
            if tombstones.contains(id) {
                continue;
            }
            let (owner_collection, owner_exists) = self.stable_cutover_session_owner(id).await?;
            if cutover_delete_requires_reconciliation(tombstones.contains(id), owner_exists) {
                // The pre-cutover audit proved that every history had an owner
                // before the session mutation fence. The only legacy operation
                // still able to remove one after that fence is delete, so finish
                // its second half transactionally and leave a permanent marker.
                self.transactional_cutover_delete(owner_collection, &SessionId(id.clone()))
                    .await?;
            }
        }
        Ok(())
    }

    async fn stable_cutover_session_owner(
        &self,
        id: &str,
    ) -> Result<(&'static str, bool), StoreError> {
        for _ in 0..12 {
            let before = self
                .namespace_pair(LEGACY_SESSIONS, PRE_COORDINATION_SESSIONS)
                .await?;
            let owner_collection = cutover_session_owner_namespace(&before)?;
            let owner = match owner_collection {
                Some(collection) => Some((
                    collection,
                    self.database
                        .collection::<Document>(collection)
                        .find_one(doc! { "_id": id })
                        .await,
                )),
                None => None,
            };
            // A concurrent opener can atomically rename the selected collection
            // or recreate the compatibility view while the find is in flight.
            // Never turn a miss (or a namespace error) into a tombstone until
            // the namespace identity and shape are unchanged across the read.
            let after = self
                .namespace_pair(LEGACY_SESSIONS, PRE_COORDINATION_SESSIONS)
                .await?;
            let owner_found = owner
                .as_ref()
                .is_some_and(|(_, result)| matches!(result, Ok(Some(_))));
            match resolve_cutover_session_owner_lookup(&before, &after, owner_found)? {
                CutoverOwnerLookup::Retry => continue,
                CutoverOwnerLookup::Present | CutoverOwnerLookup::Absent => {}
            }

            let Some((collection, owner)) = owner else {
                return Err(StoreError::Unavailable(format!(
                    "MongoDB event history for session {id} has no fenced session owner namespace during cutover; refusing to infer a delete"
                )));
            };
            return owner
                .map(|owner| (collection, owner.is_some()))
                .map_err(|error| {
                    StoreError::Unavailable(format!(
                        "recheck legacy-delete owner {id} in {collection}: {error}"
                    ))
                });
        }
        Err(StoreError::Unavailable(format!(
            "MongoDB session owner namespace did not stabilize while reconciling legacy delete {id}"
        )))
    }

    async fn transactional_cutover_delete(
        &self,
        owner_collection: &'static str,
        id: &SessionId,
    ) -> Result<(), StoreError> {
        let context = (
            self.database.collection::<Document>(owner_collection),
            self.database.collection::<Document>(EVENTS),
            self.database.collection::<Document>(WRITES),
            self.database.collection::<SessionTombstoneDoc>(TOMBSTONES),
            id.clone(),
        );
        let mut session = self.client.start_session().await.map_err(|error| {
            StoreError::Unavailable(format!("start legacy cutover delete transaction: {error}"))
        })?;
        session
            .start_transaction()
            .read_concern(ReadConcern::snapshot())
            .write_concern(WriteConcern::majority())
            .and_run(
                context,
                |session, (owner, events, writes, tombstones, id)| {
                    async move {
                        if owner
                            .find_one(doc! { "_id": &id.0 })
                            .session(&mut *session)
                            .await?
                            .is_some()
                        {
                            return Ok(());
                        }
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
                StoreError::Unavailable(format!(
                    "finish legacy cutover delete transaction: {error}"
                ))
            })
    }

    async fn verify_versioned_namespaces(&self) -> Result<(), StoreError> {
        self.verify_coordinated_namespaces().await?;
        let cutover_ids = self.completed_legacy_cutover_ids_from_marker().await?;
        validate_migrated_namespace_ids(&cutover_ids, self.collection_ids.as_ref())
    }

    async fn ensure_versioned_namespace<Authorization>(
        &self,
        qualification: &TransactionQualified,
        authorization: &Authorization,
        legacy: &str,
        authoritative: &str,
    ) -> Result<(), StoreError>
    where
        Authorization: NamespaceMutationAuthorized,
    {
        // Another replica may be performing the same cutover, and a process may
        // have stopped after the rename but before creating the view. Re-read
        // after every action. A protocol-fenced authoritative collection is the
        // durable proof that another collection under the legacy name is a late
        // old-writer spill from that exact gap; an unfenced pair stays
        // ambiguous and is never guessed between.
        for _ in 0..12 {
            let before = self.namespace_pair(legacy, authoritative).await?;
            let action = next_namespace_action(&before, legacy, authoritative)?;
            if action == NamespaceAction::Ready {
                return Ok(());
            }

            if let Err(error) = self
                .execute_namespace_action(
                    qualification,
                    authorization,
                    action,
                    legacy,
                    authoritative,
                )
                .await
            {
                let after = self.namespace_pair(legacy, authoritative).await?;
                let after_action = next_namespace_action(&after, legacy, authoritative)?;
                if after != before || after_action == NamespaceAction::Ready {
                    continue;
                }
                return Err(StoreError::Unavailable(format!(
                    "MongoDB namespace cutover {legacy} -> {authoritative} requires createCollection, renameCollectionSameDB, collMod, listCollections, find, insert, createIndex, and dropCollection privileges: {error}"
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
            let marked = namespace_has_protocol_validator(&specification, authoritative);
            let fenced = namespace_is_protocol_fenced(&specification, authoritative);
            if specification.name == legacy {
                pair.legacy = Some(kind);
                pair.legacy_marked = marked;
                pair.legacy_fenced = fenced;
                pair.legacy_uuid = specification.info.uuid.clone();
            } else if specification.name == authoritative {
                pair.authoritative = Some(kind);
                pair.authoritative_marked = marked;
                pair.authoritative_fenced = fenced;
                pair.authoritative_uuid = specification.info.uuid.clone();
            }
        }
        Ok(pair)
    }

    async fn execute_namespace_action<Authorization>(
        &self,
        qualification: &TransactionQualified,
        authorization: &Authorization,
        action: NamespaceAction,
        legacy: &str,
        authoritative: &str,
    ) -> Result<(), StoreError>
    where
        Authorization: NamespaceMutationAuthorized,
    {
        match qualification.authorize(authorization.authorize(action)) {
            NamespaceAction::CreateAuthoritative => self
                .database
                .create_collection(authoritative)
                .await
                .map_err(|error| StoreError::Unavailable(error.to_string())),
            NamespaceAction::FenceLegacy => {
                self.install_namespace_fence(legacy, authoritative).await
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
                    .map_err(|error| StoreError::Unavailable(error.to_string()))
            }
            NamespaceAction::FenceAuthoritative => {
                self.install_namespace_fence(authoritative, authoritative)
                    .await
            }
            NamespaceAction::CreateLegacyView => self
                .database
                .create_collection(legacy)
                .view_on(authoritative)
                .pipeline(Vec::<Document>::new())
                .await
                .map_err(|error| StoreError::Unavailable(error.to_string())),
            NamespaceAction::FenceLateLegacy => {
                self.install_namespace_fence(legacy, authoritative).await
            }
            NamespaceAction::MergeLateLegacy => {
                self.merge_late_legacy_namespace(legacy, authoritative)
                    .await?;
                self.database
                    .collection::<Document>(legacy)
                    .drop()
                    .await
                    .map_err(|error| StoreError::Unavailable(error.to_string()))
            }
            NamespaceAction::Ready => Ok(()),
        }
    }

    async fn install_namespace_fence(
        &self,
        namespace: &str,
        authoritative: &str,
    ) -> Result<(), StoreError> {
        let validator = expected_validator(authoritative).ok_or_else(|| {
            StoreError::Unavailable(format!(
                "no MongoDB cutover fence is defined for {authoritative}"
            ))
        })?;
        self.database
            .run_command(doc! {
                "collMod": namespace,
                "validator": validator,
                "validationLevel": "strict",
                "validationAction": "error",
            })
            .await
            .map(|_| ())
            .map_err(|error| {
                StoreError::Unavailable(format!(
                    "install old-writer fence on MongoDB collection {namespace}: {error}"
                ))
            })
    }

    async fn install_namespace_marker(
        &self,
        namespace: &str,
        authoritative: &str,
    ) -> Result<(), StoreError> {
        let validator = expected_validator(authoritative).ok_or_else(|| {
            StoreError::Unavailable(format!(
                "no MongoDB cutover marker is defined for {authoritative}"
            ))
        })?;
        self.database
            .run_command(doc! {
                "collMod": namespace,
                "validator": validator,
                "validationLevel": "strict",
                // The marker must travel with the collection through rename,
                // while a legacy append already released by its session upsert
                // must still be accepted. `warn` supplies that durable identity
                // without changing write behaviour.
                "validationAction": "warn",
            })
            .await
            .map(|_| ())
            .map_err(|error| {
                StoreError::Unavailable(format!(
                    "install MongoDB cutover marker on {namespace}: {error}"
                ))
            })
    }

    async fn merge_late_legacy_namespace(
        &self,
        legacy: &str,
        authoritative: &str,
    ) -> Result<(), StoreError> {
        match (legacy, authoritative) {
            (LEGACY_EVENTS, EVENTS) => self.merge_late_legacy_events().await,
            _ => Err(StoreError::Unavailable(format!(
                "no MongoDB late-writer merge is defined for {legacy} -> {authoritative}"
            ))),
        }
    }

    async fn merge_late_legacy_events(&self) -> Result<(), StoreError> {
        let position_index = required_indexes()
            .into_iter()
            .find(|required| required.collection == EVENTS && required.unique)
            .expect("the authoritative event-position index is required");
        let target = self.database.collection::<Document>(EVENTS);
        target
            .create_index(required_index_model(&position_index))
            .await
            .map_err(|error| {
                StoreError::Unavailable(format!(
                    "create event-position index before late-writer merge: {error}"
                ))
            })?;

        let source = self.database.collection::<Document>(LEGACY_EVENTS);
        let mut cursor = source
            .find(doc! {})
            .sort(doc! { "session_id": 1, "seq": 1 })
            .await
            .map_err(|error| {
                StoreError::Unavailable(format!("read late legacy events during cutover: {error}"))
            })?;
        while let Some(document) = cursor.try_next().await.map_err(|error| {
            StoreError::Unavailable(format!("late legacy event cursor during cutover: {error}"))
        })? {
            let event = normalize_cutover_event_document(document)?;
            if let Some(existing) =
                target
                    .find_one(event.position.clone())
                    .await
                    .map_err(|error| {
                        StoreError::Unavailable(format!(
                            "check late legacy event {}:{} during cutover: {error}",
                            event.session_id, event.seq
                        ))
                    })?
            {
                validate_same_cutover_event(&existing, &event.document)?;
                continue;
            }

            if let Err(error) = target.insert_one(event.document.clone()).await {
                // A concurrent opener can win the unique-position insert. Read
                // its value and accept only the same event; never overwrite a
                // different history entry merely because it has the same cursor.
                let existing =
                    target
                        .find_one(event.position.clone())
                        .await
                        .map_err(|read_error| {
                            StoreError::Unavailable(format!(
                            "resolve concurrent late-event merge {}:{} after {error}: {read_error}",
                            event.session_id, event.seq
                        ))
                        })?;
                let Some(existing) = existing else {
                    return Err(StoreError::Unavailable(format!(
                        "merge late legacy event {}:{}: {error}",
                        event.session_id, event.seq
                    )));
                };
                validate_same_cutover_event(&existing, &event.document)?;
            }
        }
        Ok(())
    }

    async fn verify_legacy_writers_drained(&self) -> Result<(), StoreError> {
        let sessions = self.validated_cutover_session_tails().await?;
        let events = self.validated_cutover_event_tails().await?;
        validate_legacy_writer_drain(&sessions, &events)
    }

    async fn validated_cutover_session_tails(&self) -> Result<BTreeMap<String, u64>, StoreError> {
        let pair = self
            .namespace_pair(LEGACY_SESSIONS, PRE_COORDINATION_SESSIONS)
            .await?;
        let mut collections = Vec::new();
        if matches!(pair.legacy, Some(NamespaceKind::Collection)) {
            collections.push(LEGACY_SESSIONS);
        }
        if matches!(pair.authoritative, Some(NamespaceKind::Collection)) {
            collections.push(PRE_COORDINATION_SESSIONS);
        }
        if matches!(pair.legacy, Some(NamespaceKind::View { .. }))
            && !matches!(pair.authoritative, Some(NamespaceKind::Collection))
        {
            return Err(StoreError::Unavailable(
                "legacy MongoDB session view has no authoritative collection during cutover".into(),
            ));
        }

        let mut tails = BTreeMap::<String, u64>::new();
        for collection in collections {
            let mut cursor = self
                .database
                .collection::<Document>(collection)
                .find(doc! {})
                .sort(doc! { "_id": 1 })
                .await
                .map_err(|error| {
                    StoreError::Unavailable(format!(
                        "validate sessions in MongoDB cutover namespace {collection}: {error}"
                    ))
                })?;
            while let Some(document) = cursor.try_next().await.map_err(|error| {
                StoreError::Unavailable(format!(
                    "MongoDB cutover session cursor in {collection}: {error}"
                ))
            })? {
                let (id, tail) = validate_compatible_stored_session_document(&document)?;
                tails
                    .entry(id)
                    .and_modify(|existing| *existing = (*existing).max(tail))
                    .or_insert(tail);
            }
        }
        Ok(tails)
    }

    async fn validated_cutover_event_tails(&self) -> Result<BTreeMap<String, u64>, StoreError> {
        let pair = self.namespace_pair(LEGACY_EVENTS, EVENTS).await?;
        let mut collections = Vec::new();
        if matches!(pair.legacy, Some(NamespaceKind::Collection)) {
            collections.push(LEGACY_EVENTS);
        }
        if matches!(pair.authoritative, Some(NamespaceKind::Collection)) {
            collections.push(EVENTS);
        }
        if matches!(pair.legacy, Some(NamespaceKind::View { .. }))
            && !matches!(pair.authoritative, Some(NamespaceKind::Collection))
        {
            return Err(StoreError::Unavailable(
                "legacy MongoDB event view has no authoritative collection during writer drain"
                    .into(),
            ));
        }

        let mut positions = BTreeMap::<(String, u64), Document>::new();
        for collection in collections {
            let mut cursor = self
                .database
                .collection::<Document>(collection)
                .find(doc! {})
                .sort(doc! { "session_id": 1, "seq": 1 })
                .await
                .map_err(|error| {
                    StoreError::Unavailable(format!(
                        "validate events in MongoDB cutover namespace {collection}: {error}"
                    ))
                })?;
            while let Some(document) = cursor.try_next().await.map_err(|error| {
                StoreError::Unavailable(format!(
                    "MongoDB cutover event cursor in {collection}: {error}"
                ))
            })? {
                let event = normalize_cutover_event_document(document)?;
                let key = (event.session_id.clone(), event.seq);
                if let Some(existing) = positions.get(&key) {
                    validate_same_cutover_event(existing, &event.document)?;
                } else {
                    positions.insert(key, event.document);
                }
            }
        }

        let mut sequence = StoredEventSequence::default();
        let mut tails = BTreeMap::new();
        for document in positions.into_values() {
            let (id, seq) = sequence.observe(&document)?;
            tails.insert(id, seq);
        }
        Ok(tails)
    }

    async fn ensure_indexes(&self) -> Result<(), StoreError> {
        for required in required_indexes() {
            self.database
                .collection::<Document>(required.collection)
                .create_index(required_index_model(&required))
                .await
                .map_err(|error| {
                    StoreError::Unavailable(format!(
                        "create required index {} on {}: {error}",
                        required.name, required.collection
                    ))
                })?;
        }
        Ok(())
    }

    async fn install_protocol_validators(&self) -> Result<(), StoreError> {
        for (collection, validator) in [
            (SESSIONS, session_protocol_validator()),
            (EVENTS, event_protocol_validator()),
            (WRITES, write_protocol_validator()),
            (TOMBSTONES, tombstone_protocol_validator()),
            (
                COORDINATION_RECEIPTS,
                coordination_receipt_protocol_validator(),
            ),
        ] {
            self.install_protocol_validator(collection, validator)
                .await?;
        }
        Ok(())
    }

    async fn install_session_protocol_validator(&self) -> Result<(), StoreError> {
        self.install_protocol_validator(SESSIONS, session_protocol_validator())
            .await
    }

    #[cfg(test)]
    async fn install_coordination_protocol_validators(&self) -> Result<(), StoreError> {
        self.install_session_protocol_validator().await?;
        self.install_protocol_validator(
            COORDINATION_RECEIPTS,
            coordination_receipt_protocol_validator(),
        )
        .await
    }

    async fn install_protocol_validator(
        &self,
        collection: &str,
        validator: Document,
    ) -> Result<(), StoreError> {
        self.database
            .run_command(doc! {
                "collMod": collection,
                "validator": validator,
                "validationLevel": "strict",
                "validationAction": "error",
                "writeConcern": { "w": "majority" },
            })
            .await
            .map_err(|error| {
                StoreError::Unavailable(format!(
                    "install MongoDB write-protocol validator on {collection}; the database role requires collMod: {error}"
                ))
            })?;
        Ok(())
    }

    async fn migrate_session_coordination_protocol(&self) -> Result<(), StoreError> {
        let current = current_session_protocol_filter();
        self.sessions
            .clone_with_type::<Document>()
            .update_many(
                doc! { "$nor": [current.clone()] },
                session_protocol_migration_update(),
            )
            .with_options(
                UpdateOptions::builder()
                    .upsert(false)
                    .write_concern(WriteConcern::majority())
                    .build(),
            )
            .await
            .map_err(|error| {
                StoreError::Unavailable(format!(
                    "migrate MongoDB sessions to coordination protocol {SESSION_COORDINATION_PROTOCOL}: {error}"
                ))
            })?;

        let stale = self
            .sessions
            .clone_with_type::<Document>()
            .find_one(doc! { "$nor": [current] })
            .projection(doc! { "_id": 1 })
            .await
            .map_err(|error| {
                StoreError::Unavailable(format!(
                    "audit MongoDB session coordination protocol migration: {error}"
                ))
            })?;
        if let Some(stale) = stale {
            return Err(StoreError::Unavailable(format!(
                "MongoDB session {} did not reach coordination protocol {SESSION_COORDINATION_PROTOCOL}",
                stale
                    .get_str("_id")
                    .unwrap_or("<invalid session identity>")
            )));
        }
        Ok(())
    }

    async fn migrate_previous_session_write_protocol(&self) -> Result<(), StoreError> {
        self.sessions
            .clone_with_type::<Document>()
            .update_many(
                previous_session_protocol_filter(),
                session_write_protocol_migration_update(),
            )
            .with_options(
                UpdateOptions::builder()
                    .upsert(false)
                    .write_concern(WriteConcern::majority())
                    .build(),
            )
            .await
            .map_err(|error| {
                StoreError::Unavailable(format!(
                    "migrate MongoDB sessions to write protocol {SESSION_WRITE_PROTOCOL}: {error}"
                ))
            })?;

        let stale = self
            .sessions
            .clone_with_type::<Document>()
            .find_one(doc! { "$nor": [current_session_protocol_filter()] })
            .projection(doc! { "_id": 1 })
            .await
            .map_err(|error| {
                StoreError::Unavailable(format!(
                    "audit MongoDB session write protocol migration: {error}"
                ))
            })?;
        if let Some(stale) = stale {
            return Err(StoreError::Unavailable(format!(
                "MongoDB session {} did not reach write protocol {SESSION_WRITE_PROTOCOL} and coordination protocol {SESSION_COORDINATION_PROTOCOL}",
                stale
                    .get_str("_id")
                    .unwrap_or("<invalid session identity>")
            )));
        }
        Ok(())
    }

    /// Fence whole-document predecessors before promoting existing rows.
    ///
    /// Installing the final validator first makes a crash resumable and rejects
    /// protocol-1 session replacements before any row claims protocol 2. A
    /// pristine activation already installed that validator; an existing
    /// protocol-3 deployment requires the same explicit offline drain used for
    /// coordination activation because rejecting a still-running predecessor
    /// would otherwise interrupt its sessions.
    async fn ensure_session_write_protocol(
        &self,
        coordination_cutover_confirmed: bool,
        expected_uuid: &Binary,
    ) -> Result<(), StoreError> {
        let contracts = self.collection_contracts().await?;
        let session = contracts.get(SESSIONS).ok_or_else(|| {
            StoreError::Unavailable(format!(
                "authoritative MongoDB collection {SESSIONS} is missing"
            ))
        })?;
        match classify_session_write_contract(session, expected_uuid)? {
            SessionWriteContract::Current => {}
            SessionWriteContract::Previous => {
                if !coordination_cutover_confirmed {
                    return Err(StoreError::Unavailable(
                        "MongoDB session write protocol 2 requires an offline writer drain: stop every older CMA process, wait for its MongoDB and external operations to finish, then start exactly one replacement with CHRONO_MONGODB_COORDINATION_CUTOVER=drained"
                            .into(),
                    ));
                }
                self.install_session_protocol_validator().await?;
            }
        }
        self.migrate_previous_session_write_protocol().await
    }

    async fn collection_contracts(
        &self,
    ) -> Result<BTreeMap<String, CollectionContract>, StoreError> {
        let mut cursor = self
            .database
            .list_collections()
            .filter(doc! {
                "name": { "$in": AUTHORITATIVE_COLLECTIONS.to_vec() }
            })
            .await
            .map_err(|error| {
                StoreError::Unavailable(format!(
                    "list authoritative MongoDB collection contracts: {error}"
                ))
            })?;
        let mut contracts = BTreeMap::new();
        while let Some(specification) = cursor.try_next().await.map_err(|error| {
            StoreError::Unavailable(format!(
                "authoritative MongoDB collection contract cursor: {error}"
            ))
        })? {
            contracts.insert(
                specification.name.clone(),
                collection_contract(&specification),
            );
        }
        Ok(contracts)
    }

    async fn collection_contract_for(
        &self,
        collection: &str,
    ) -> Result<Option<CollectionContract>, StoreError> {
        let mut cursor = self
            .database
            .list_collections()
            .filter(doc! { "name": collection })
            .await
            .map_err(|error| {
                StoreError::Unavailable(format!(
                    "list MongoDB collection contract for {collection}: {error}"
                ))
            })?;
        let specification = cursor.try_next().await.map_err(|error| {
            StoreError::Unavailable(format!(
                "MongoDB collection contract cursor for {collection}: {error}"
            ))
        })?;
        Ok(specification.as_ref().map(collection_contract))
    }

    async fn authoritative_collection_ids(&self) -> Result<BTreeMap<String, Binary>, StoreError> {
        let contracts = self.collection_contracts().await?;
        let mut ids = BTreeMap::new();
        for collection in AUTHORITATIVE_COLLECTIONS {
            let contract = contracts.get(collection).ok_or_else(|| {
                StoreError::Unavailable(format!(
                    "authoritative MongoDB collection {collection} is missing"
                ))
            })?;
            let uuid = contract.uuid.clone().ok_or_else(|| {
                StoreError::Unavailable(format!(
                    "authoritative MongoDB collection {collection} has no stable UUID"
                ))
            })?;
            ids.insert(collection.to_owned(), uuid);
        }
        Ok(ids)
    }

    async fn verify_collection_contracts(
        &self,
        expected_ids: Option<&BTreeMap<String, Binary>>,
    ) -> Result<(), StoreError> {
        let contracts = self.collection_contracts().await?;
        for collection in AUTHORITATIVE_COLLECTIONS {
            let contract = contracts.get(collection).ok_or_else(|| {
                StoreError::Unavailable(format!(
                    "authoritative MongoDB collection {collection} is missing"
                ))
            })?;
            validate_collection_contract(
                collection,
                contract,
                expected_ids.and_then(|ids| ids.get(collection)),
            )?;
        }
        Ok(())
    }

    async fn verify_non_session_collection_contracts(
        &self,
        expected_ids: Option<&BTreeMap<String, Binary>>,
    ) -> Result<(), StoreError> {
        let contracts = self.collection_contracts().await?;
        for collection in AUTHORITATIVE_COLLECTIONS
            .into_iter()
            .filter(|collection| *collection != SESSIONS)
        {
            let contract = contracts.get(collection).ok_or_else(|| {
                StoreError::Unavailable(format!(
                    "authoritative MongoDB collection {collection} is missing"
                ))
            })?;
            validate_collection_contract(
                collection,
                contract,
                expected_ids.and_then(|ids| ids.get(collection)),
            )?;
        }
        Ok(())
    }

    async fn verify_required_indexes(&self) -> Result<(), StoreError> {
        let required = required_indexes();
        for collection in [EVENTS, WRITES, SESSIONS, COORDINATION_RECEIPTS] {
            let mut cursor = self
                .database
                .collection::<Document>(collection)
                .list_indexes()
                .await
                .map_err(|error| {
                    StoreError::Unavailable(format!(
                        "list required MongoDB indexes on {collection}: {error}"
                    ))
                })?;
            let mut indexes = Vec::new();
            while let Some(index) = cursor.try_next().await.map_err(|error| {
                StoreError::Unavailable(format!(
                    "required MongoDB index cursor on {collection}: {error}"
                ))
            })? {
                indexes.push(index);
            }
            for specification in required
                .iter()
                .filter(|required| required.collection == collection)
            {
                validate_required_index(specification, &indexes)?;
            }
        }
        Ok(())
    }

    async fn validate_post_fence_state(&self) -> Result<HistorySnapshot, StoreError> {
        let mut tombstones = self.validated_tombstone_ids().await?;
        let mut sessions = self.validated_session_tails().await?;
        let mut event_tails = self.validated_event_tails().await?;

        for (id, event_tail) in event_tails.clone() {
            if sessions.contains_key(&id) || tombstones.contains(&id) {
                continue;
            }
            // A new replica may have committed this session after the session
            // scan began. Re-read only the apparent orphan after all old names
            // are fenced, so an old writer cannot make the answer oscillate.
            let owner = self
                .sessions
                .clone_with_type::<Document>()
                .find_one(doc! { "_id": &id })
                .await
                .map_err(|error| {
                    StoreError::Unavailable(format!("recheck event-history owner {id}: {error}"))
                })?;
            if let Some(owner) = owner {
                let (owner_id, owner_tail) = validate_stored_session_document(&owner)?;
                sessions.insert(owner_id, owner_tail);
            } else if self.is_tombstoned(&SessionId(id.clone())).await? {
                tombstones.insert(id);
            } else {
                return Err(StoreError::Malformed(format!(
                    "event history for session {id} ends at {event_tail} but has no session record or deletion tombstone after the writer fence"
                )));
            }
        }

        for (id, recorded_tail) in sessions.clone() {
            if tombstones.contains(&id) {
                continue;
            }
            let physical_tail = event_tails.get(&id).copied().unwrap_or(0);
            if recorded_tail <= physical_tail {
                continue;
            }
            if self.is_tombstoned(&SessionId(id.clone())).await? {
                tombstones.insert(id);
                continue;
            }
            let Some(record) = self
                .sessions
                .clone_with_type::<Document>()
                .find_one(doc! { "_id": &id })
                .await
                .map_err(|error| {
                    StoreError::Unavailable(format!("recheck session tail for {id}: {error}"))
                })?
            else {
                if self.is_tombstoned(&SessionId(id.clone())).await? {
                    tombstones.insert(id);
                    continue;
                }
                return Err(StoreError::Malformed(format!(
                    "session {id} disappeared without a deletion tombstone during history validation"
                )));
            };
            let (_, current_recorded_tail) = validate_stored_session_document(&record)?;
            let current_physical_tail = self
                .last_event_sequence(&SessionId(id.clone()))
                .await?
                .unwrap_or(0);
            if current_recorded_tail > current_physical_tail {
                return Err(StoreError::Malformed(format!(
                    "session {id} advertises event {current_recorded_tail}, but contiguous physical history ends at {current_physical_tail} after the old writer was fenced"
                )));
            }
            sessions.insert(id.clone(), current_recorded_tail);
            if current_physical_tail == 0 {
                event_tails.remove(&id);
            } else {
                event_tails.insert(id, current_physical_tail);
            }
        }
        Ok(HistorySnapshot {
            tombstones,
            session_tails: sessions,
            event_tails,
        })
    }

    async fn audited_history_snapshot(
        &self,
        previous: Option<&HistorySnapshot>,
    ) -> Result<HistorySnapshot, StoreError> {
        let first = self.validate_post_fence_state().await;
        if let Ok(snapshot) = first {
            let progressed = previous
                .map(|previous| validate_history_progress(previous, &snapshot).is_ok())
                .unwrap_or(true);
            if progressed {
                return Ok(snapshot);
            }
        }

        // A delete transaction can commit between the independent collection
        // scans. Retry a suspicious result once so its permanent tombstone is
        // visible before treating the apparent regression as corruption.
        let snapshot = self.validate_post_fence_state().await?;
        if let Some(previous) = previous {
            validate_history_progress(previous, &snapshot)?;
        }
        Ok(snapshot)
    }

    async fn audit_integrity_if_due(&self) -> Result<(), StoreError> {
        let previous = {
            let audit = self.integrity.lock().await;
            if let Some(cached) = audit.cached_result(Instant::now()) {
                return cached;
            }
            audit.snapshot.clone()
        };

        let result = async {
            let snapshot = self.audited_history_snapshot(previous.as_ref()).await?;
            self.validate_coordination_receipts().await?;
            Ok(snapshot)
        }
        .await;
        let checked_at = Instant::now();
        let mut audit = self.integrity.lock().await;
        match result {
            Ok(snapshot) => {
                audit.record_success(checked_at, snapshot);
                Ok(())
            }
            Err(error) => {
                audit.record_failure(checked_at, error);
                Err(audit
                    .last_error
                    .clone()
                    .expect("a failed integrity audit retains its error"))
            }
        }
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
}

impl IntegrityAudit {
    fn cached_result(&self, now: Instant) -> Option<Result<(), StoreError>> {
        let checked_at = self.checked_at?;
        if now.saturating_duration_since(checked_at) >= HISTORY_AUDIT_INTERVAL {
            return None;
        }
        if let Some(error) = &self.last_error {
            return Some(Err(error.clone()));
        }
        self.snapshot.as_ref().map(|_| Ok(()))
    }

    fn record_success(&mut self, checked_at: Instant, snapshot: HistorySnapshot) {
        self.checked_at = Some(checked_at);
        self.last_error = None;
        self.snapshot = Some(snapshot);
    }

    fn record_failure(&mut self, checked_at: Instant, error: StoreError) {
        self.checked_at = Some(checked_at);
        if self.last_error.is_none() {
            self.last_error = Some(error);
        }
    }
}

fn collection_contract(specification: &CollectionSpecification) -> CollectionContract {
    CollectionContract {
        kind: namespace_kind(specification),
        validator: specification.options.validator.clone(),
        validation_level: specification.options.validation_level.clone(),
        validation_action: specification.options.validation_action.clone(),
        uuid: specification.info.uuid.clone(),
    }
}

fn namespace_has_protocol_validator(
    specification: &CollectionSpecification,
    authoritative: &str,
) -> bool {
    specification.collection_type == CollectionType::Collection
        && expected_validator(authoritative).is_some_and(|expected| {
            specification
                .options
                .validator
                .as_ref()
                .is_some_and(|actual| {
                    documents_equivalent(actual, &expected)
                        // A deployment which reached the original coordination
                        // implementation may already have the stronger v3
                        // validator on v2. Treat it as fenced without briefly
                        // weakening it before the rollback-resistant rename.
                        || (authoritative == PRE_COORDINATION_SESSIONS
                            && documents_equivalent(actual, &session_protocol_validator()))
                })
        })
        && specification.options.validation_level == Some(ValidationLevel::Strict)
}

fn namespace_is_protocol_fenced(
    specification: &CollectionSpecification,
    authoritative: &str,
) -> bool {
    namespace_has_protocol_validator(specification, authoritative)
        && specification.options.validation_action == Some(ValidationAction::Error)
}

fn expected_validator(collection: &str) -> Option<Document> {
    match collection {
        PRE_COORDINATION_SESSIONS => Some(pre_coordination_session_protocol_validator()),
        SESSIONS => Some(session_protocol_validator()),
        EVENTS => Some(event_protocol_validator()),
        WRITES => Some(write_protocol_validator()),
        TOMBSTONES => Some(tombstone_protocol_validator()),
        COORDINATION_RECEIPTS => Some(coordination_receipt_protocol_validator()),
        _ => None,
    }
}

fn validate_collection_contract(
    collection: &str,
    actual: &CollectionContract,
    expected_uuid: Option<&Binary>,
) -> Result<(), StoreError> {
    let expected_validator = expected_validator(collection).ok_or_else(|| {
        StoreError::Unavailable(format!(
            "no MongoDB collection contract is defined for {collection}"
        ))
    })?;
    validate_collection_contract_against(collection, actual, &expected_validator, expected_uuid)
}

fn validate_collection_contract_against(
    collection: &str,
    actual: &CollectionContract,
    expected_validator: &Document,
    expected_uuid: Option<&Binary>,
) -> Result<(), StoreError> {
    if actual.kind != NamespaceKind::Collection {
        return Err(StoreError::Unavailable(format!(
            "authoritative MongoDB namespace {collection} is not a collection"
        )));
    }
    if !actual
        .validator
        .as_ref()
        .is_some_and(|validator| documents_equivalent(validator, expected_validator))
    {
        return Err(StoreError::Unavailable(format!(
            "authoritative MongoDB collection {collection} has a changed write-protocol validator"
        )));
    }
    if actual.validation_level != Some(ValidationLevel::Strict) {
        return Err(StoreError::Unavailable(format!(
            "authoritative MongoDB collection {collection} does not enforce strict validation"
        )));
    }
    if actual.validation_action != Some(ValidationAction::Error) {
        return Err(StoreError::Unavailable(format!(
            "authoritative MongoDB collection {collection} does not reject invalid writes"
        )));
    }
    let actual_uuid = actual.uuid.as_ref().ok_or_else(|| {
        StoreError::Unavailable(format!(
            "authoritative MongoDB collection {collection} has no stable UUID"
        ))
    })?;
    if expected_uuid.is_some_and(|expected| expected != actual_uuid) {
        return Err(StoreError::Unavailable(format!(
            "authoritative MongoDB collection {collection} was replaced after startup"
        )));
    }
    Ok(())
}

fn classify_session_write_contract(
    actual: &CollectionContract,
    expected_uuid: &Binary,
) -> Result<SessionWriteContract, StoreError> {
    if validate_collection_contract_against(
        SESSIONS,
        actual,
        &session_protocol_validator(),
        Some(expected_uuid),
    )
    .is_ok()
    {
        return Ok(SessionWriteContract::Current);
    }
    if validate_collection_contract_against(
        SESSIONS,
        actual,
        &previous_session_protocol_validator(),
        Some(expected_uuid),
    )
    .is_ok()
    {
        return Ok(SessionWriteContract::Previous);
    }
    Err(StoreError::Unavailable(
        "authoritative MongoDB session collection is neither the exact current nor previous write-protocol contract; refusing to replace an unknown validator"
            .into(),
    ))
}

fn documents_equivalent(left: &Document, right: &Document) -> bool {
    left.len() == right.len()
        && left.iter().all(|(key, left)| {
            right
                .get(key)
                .is_some_and(|right| bson_values_equivalent(left, right))
        })
}

fn bson_values_equivalent(left: &Bson, right: &Bson) -> bool {
    match (left, right) {
        (Bson::Document(left), Bson::Document(right)) => documents_equivalent(left, right),
        (Bson::Array(left), Bson::Array(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(left, right)| bson_values_equivalent(left, right))
        }
        _ => left == right,
    }
}

fn required_indexes() -> Vec<RequiredIndex> {
    vec![
        RequiredIndex {
            collection: EVENTS,
            name: "session_id_1_seq_1",
            keys: doc! { "session_id": 1, "seq": 1 },
            unique: true,
        },
        RequiredIndex {
            collection: WRITES,
            name: "session_id_1",
            keys: doc! { "session_id": 1 },
            unique: false,
        },
        RequiredIndex {
            collection: SESSIONS,
            name: OWNER_SESSION_PAGE_INDEX,
            keys: doc! { "owner": 1, "created_at": -1, "_id": -1 },
            unique: false,
        },
        RequiredIndex {
            collection: SESSIONS,
            name: OWNER_STATUS_SESSION_PAGE_INDEX,
            keys: doc! { "owner": 1, "status": 1, "created_at": -1, "_id": -1 },
            unique: false,
        },
        RequiredIndex {
            collection: SESSIONS,
            name: ALL_SESSION_PAGE_INDEX,
            keys: doc! { "created_at": -1, "_id": -1 },
            unique: false,
        },
        RequiredIndex {
            collection: SESSIONS,
            name: STATUS_SESSION_PAGE_INDEX,
            keys: doc! { "status": 1, "created_at": -1, "_id": -1 },
            unique: false,
        },
        RequiredIndex {
            collection: SESSIONS,
            name: "status_1",
            keys: doc! { "status": 1 },
            unique: false,
        },
        RequiredIndex {
            collection: COORDINATION_RECEIPTS,
            name: "_id.session_id_1__id.kind_1_payload.type_1",
            keys: doc! {
                "_id.session_id": 1,
                "_id.kind": 1,
                "payload.type": 1,
            },
            unique: false,
        },
    ]
}

fn required_index_model(required: &RequiredIndex) -> IndexModel {
    IndexModel::builder()
        .keys(required.keys.clone())
        .options(
            IndexOptions::builder()
                .name(required.name.to_owned())
                .unique(required.unique)
                .build(),
        )
        .build()
}

fn index_keys_equal(left: &Document, right: &Document) -> bool {
    left.len() == right.len()
        && left.iter().zip(right.iter()).all(
            |((left_key, left_value), (right_key, right_value))| {
                left_key == right_key && left_value == right_value
            },
        )
}

fn validate_required_index(
    required: &RequiredIndex,
    indexes: &[IndexModel],
) -> Result<(), StoreError> {
    let actual = indexes
        .iter()
        .find(|index| {
            index
                .options
                .as_ref()
                .and_then(|options| options.name.as_deref())
                == Some(required.name)
        })
        .ok_or_else(|| {
            StoreError::Unavailable(format!(
                "required MongoDB index {} is missing from {}",
                required.name, required.collection
            ))
        })?;
    let options = actual.options.as_ref().ok_or_else(|| {
        StoreError::Unavailable(format!(
            "required MongoDB index {} on {} has no options",
            required.name, required.collection
        ))
    })?;
    let unique = options.unique.unwrap_or(false);
    let weakened = options.sparse.unwrap_or(false)
        || options.partial_filter_expression.is_some()
        || options.hidden.unwrap_or(false)
        || options.expire_after.is_some()
        || options.collation.is_some();
    if !index_keys_equal(&actual.keys, &required.keys) || unique != required.unique || weakened {
        return Err(StoreError::Unavailable(format!(
            "required MongoDB index {} on {} no longer has its exact key and constraint contract",
            required.name, required.collection
        )));
    }
    Ok(())
}

fn validate_legacy_pre_cutover_owners(
    sessions: &BTreeMap<String, u64>,
    events: &BTreeMap<String, u64>,
    tombstones: &HashSet<String>,
) -> Result<(), StoreError> {
    if let Some((id, event_tail)) = events
        .iter()
        .find(|(id, _)| !sessions.contains_key(*id) && !tombstones.contains(*id))
    {
        return Err(StoreError::Unavailable(format!(
            "legacy MongoDB history for session {id} ends at {event_tail} without its owner; retry after an in-flight legacy delete finishes"
        )));
    }
    Ok(())
}

fn validate_legacy_writer_drain(
    sessions: &BTreeMap<String, u64>,
    events: &BTreeMap<String, u64>,
) -> Result<(), StoreError> {
    if let Some((id, recorded_tail)) = sessions
        .iter()
        .find(|(id, recorded_tail)| events.get(*id).copied().unwrap_or(0) < **recorded_tail)
    {
        let event_tail = events.get(id).copied().unwrap_or(0);
        return Err(StoreError::Unavailable(format!(
            "legacy MongoDB writer drain is incomplete for session {id}: record advertises event {recorded_tail}, history currently ends at {event_tail}; retry after the in-flight legacy flush completes"
        )));
    }
    Ok(())
}

fn cutover_delete_requires_reconciliation(tombstoned: bool, owner_exists: bool) -> bool {
    !tombstoned && !owner_exists
}

fn validate_history_progress(
    previous: &HistorySnapshot,
    current: &HistorySnapshot,
) -> Result<(), StoreError> {
    for id in &previous.tombstones {
        if !current.tombstones.contains(id) {
            return Err(StoreError::Malformed(format!(
                "permanent deletion tombstone for session {id} disappeared"
            )));
        }
    }

    for (id, previous_tail) in &previous.session_tails {
        if current.tombstones.contains(id) {
            continue;
        }
        let current_tail = current.session_tails.get(id).ok_or_else(|| {
            StoreError::Malformed(format!(
                "session {id} disappeared without a deletion tombstone"
            ))
        })?;
        if current_tail < previous_tail {
            return Err(StoreError::Malformed(format!(
                "session {id} recorded tail decreased from {previous_tail} to {current_tail}"
            )));
        }
    }

    for (id, previous_tail) in &previous.event_tails {
        if current.tombstones.contains(id) {
            continue;
        }
        let current_tail = current.event_tails.get(id).ok_or_else(|| {
            StoreError::Malformed(format!(
                "event history for session {id} disappeared without a deletion tombstone"
            ))
        })?;
        if current_tail < previous_tail {
            return Err(StoreError::Malformed(format!(
                "event history for session {id} decreased from {previous_tail} to {current_tail}"
            )));
        }
    }

    for (id, recorded_tail) in &current.session_tails {
        if current.tombstones.contains(id) {
            continue;
        }
        let physical_tail = current.event_tails.get(id).copied().unwrap_or(0);
        if recorded_tail > &physical_tail {
            return Err(StoreError::Malformed(format!(
                "session {id} advertises event {recorded_tail}, but physical history ends at {physical_tail}"
            )));
        }
    }
    for (id, event_tail) in &current.event_tails {
        if !current.session_tails.contains_key(id) && !current.tombstones.contains(id) {
            return Err(StoreError::Malformed(format!(
                "event history for session {id} ends at {event_tail} without a session record or deletion tombstone"
            )));
        }
    }
    Ok(())
}

async fn bounded_readiness_probe(
    permit: Arc<tokio::sync::Semaphore>,
    probe: impl Future<Output = Result<(), StoreError>> + Send + 'static,
    timeout: Duration,
) -> Result<(), StoreError> {
    let deadline = tokio::time::Instant::now() + timeout;
    let permit = permit.try_acquire_owned().map_err(|_| {
        StoreError::Unavailable("MongoDB readiness probe is already in progress".into())
    })?;
    // MongoDB driver futures must be polled to completion. Timing out this
    // JoinHandle detaches the whole probe with its permit still held, so a
    // stalled driver operation cannot create a queue of detached audit tasks.
    let probe = tokio::spawn(async move {
        let _permit = permit;
        probe.await
    });
    match tokio::time::timeout_at(deadline, probe).await {
        Ok(Ok(result)) => result,
        Ok(Err(error)) => Err(StoreError::Unavailable(format!(
            "MongoDB readiness probe task failed: {error}"
        ))),
        Err(_) => Err(StoreError::Unavailable(format!(
            "MongoDB readiness probe timed out after {} ms",
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

fn namespace_cutover_marker_document(
    marker: &NamespaceCutoverMarker,
) -> Result<Document, StoreError> {
    let (origin, phase, source_ids, tombstone_id, collection_ids) = match marker {
        NamespaceCutoverMarker::InProgress {
            origin,
            source_ids,
            tombstone_id,
        } => (
            origin,
            "in-progress",
            Some(source_ids),
            Some(tombstone_id),
            None,
        ),
        NamespaceCutoverMarker::Complete {
            origin,
            collection_ids,
        } => (origin, "complete", None, None, Some(collection_ids)),
    };
    validate_namespace_cutover_origin(origin, phase == "complete")?;
    let mut document = doc! {
        "_id": NAMESPACE_CUTOVER_ID,
        "protocol": NAMESPACE_CUTOVER_PROTOCOL,
        "origin": origin,
        "phase": phase,
    };
    if let Some(source_ids) = source_ids {
        validate_namespace_cutover_source_origin(origin, source_ids)?;
        document.insert(
            "source_collection_uuids",
            namespace_cutover_source_document(source_ids)?,
        );
    }
    if let Some(tombstone_id) = tombstone_id {
        validate_in_progress_tombstone_binding(
            source_ids.expect("source accompanies binding"),
            tombstone_id.as_ref(),
        )?;
        document.insert(
            "tombstone_collection_uuid",
            tombstone_id
                .as_ref()
                .map_or(Bson::Null, |uuid| Bson::Binary(uuid.clone())),
        );
    }
    if let Some(collection_ids) = collection_ids {
        let sessions = required_cutover_uuid(collection_ids, PRE_COORDINATION_SESSIONS)?;
        let events = required_cutover_uuid(collection_ids, EVENTS)?;
        let tombstones = required_cutover_uuid(collection_ids, TOMBSTONES)?;
        let mut stored = Document::new();
        stored.insert(PRE_COORDINATION_SESSIONS, sessions.clone());
        stored.insert(EVENTS, events.clone());
        stored.insert(TOMBSTONES, tombstones.clone());
        document.insert("collection_uuids", stored);
    }
    Ok(document)
}

fn namespace_cutover_source_document(
    source_ids: &BTreeMap<String, Option<Binary>>,
) -> Result<Document, StoreError> {
    if source_ids.len() != LEGACY_CUTOVER_SOURCES.len() {
        return Err(StoreError::Malformed(
            "in-progress MongoDB namespace-cutover marker must name exactly the two raw sources and the tombstone source state"
                .into(),
        ));
    }
    let mut stored = Document::new();
    for collection in LEGACY_CUTOVER_SOURCES {
        let source = source_ids.get(collection).ok_or_else(|| {
            StoreError::Malformed(format!(
                "in-progress MongoDB namespace-cutover marker is missing source namespace {collection}"
            ))
        })?;
        match source {
            Some(uuid) => {
                validate_cutover_uuid(collection, uuid)?;
                stored.insert(collection, uuid.clone());
            }
            None => {
                stored.insert(collection, Bson::Null);
            }
        }
    }
    Ok(stored)
}

fn validate_in_progress_tombstone_binding(
    source_ids: &BTreeMap<String, Option<Binary>>,
    tombstone_id: Option<&Binary>,
) -> Result<(), StoreError> {
    let initial = source_ids.get(TOMBSTONES).ok_or_else(|| {
        StoreError::Malformed(format!(
            "in-progress MongoDB namespace-cutover marker is missing source namespace {TOMBSTONES}"
        ))
    })?;
    if let Some(tombstone_id) = tombstone_id {
        validate_cutover_uuid(TOMBSTONES, tombstone_id)?;
    }
    match (initial.as_ref(), tombstone_id) {
        (Some(initial), Some(bound)) if initial == bound => Ok(()),
        (Some(_), Some(_)) => Err(StoreError::Malformed(format!(
            "in-progress MongoDB namespace-cutover marker binds a different {TOMBSTONES} UUID than its initial source"
        ))),
        (Some(_), None) => Err(StoreError::Malformed(format!(
            "in-progress MongoDB namespace-cutover marker omits the UUID of pre-existing collection {TOMBSTONES}"
        ))),
        (None, _) => Ok(()),
    }
}

fn coordination_receipts_provenance_document(
    marker: &CoordinationReceiptsProvenance,
) -> Result<Document, StoreError> {
    let mut document = doc! {
        "_id": COORDINATION_RECEIPTS_PROVENANCE_ID,
        "protocol": COORDINATION_RECEIPTS_PROVENANCE_PROTOCOL,
        "phase": match marker {
            CoordinationReceiptsProvenance::InProgress => "in-progress",
            CoordinationReceiptsProvenance::Complete { .. } => "complete",
        },
    };
    if let CoordinationReceiptsProvenance::Complete { collection_id } = marker {
        validate_cutover_uuid(COORDINATION_RECEIPTS, collection_id)?;
        document.insert("collection_uuid", collection_id.clone());
    }
    Ok(document)
}

fn coordination_activation_marker_document(
    marker: &CoordinationActivationMarker,
) -> Result<Document, StoreError> {
    let (protocol, origin, phase, collection_ids) = match marker {
        CoordinationActivationMarker::LegacyInProgress { protocol, origin } => {
            (*protocol, origin, "in-progress", None)
        }
        CoordinationActivationMarker::LegacyComplete {
            protocol,
            origin,
            collection_ids,
        } => (*protocol, origin, "complete", Some(collection_ids)),
        CoordinationActivationMarker::LegacyUpgradeInProgress {
            protocol,
            origin,
            collection_ids,
        } => {
            if *protocol != LEGACY_COORDINATION_ACTIVATION_PROTOCOL {
                return Err(StoreError::Malformed(format!(
                    "MongoDB coordination-activation upgrade marker protocol {protocol} is not supported"
                )));
            }
            (
                *protocol,
                origin,
                "upgrade-in-progress",
                Some(collection_ids),
            )
        }
        CoordinationActivationMarker::InProgress { origin } => (
            COORDINATION_ACTIVATION_PROTOCOL,
            origin,
            "in-progress",
            None,
        ),
        CoordinationActivationMarker::UpgradeInProgress {
            origin,
            collection_ids,
        } => (
            COORDINATION_ACTIVATION_PROTOCOL,
            origin,
            "upgrade-in-progress",
            Some(collection_ids),
        ),
        CoordinationActivationMarker::Complete {
            origin,
            collection_ids,
        } => (
            COORDINATION_ACTIVATION_PROTOCOL,
            origin,
            "complete",
            Some(collection_ids),
        ),
    };
    if protocol != COORDINATION_ACTIVATION_PROTOCOL
        && !matches!(
            protocol,
            INITIAL_COORDINATION_ACTIVATION_PROTOCOL | LEGACY_COORDINATION_ACTIVATION_PROTOCOL
        )
    {
        return Err(StoreError::Malformed(format!(
            "MongoDB coordination-activation marker protocol {protocol} is not supported"
        )));
    }
    validate_coordination_activation_origin(origin)?;
    let mut document = doc! {
        "_id": COORDINATION_ACTIVATION_ID,
        "protocol": protocol,
        "origin": origin,
        "phase": phase,
    };
    if let Some(collection_ids) = collection_ids {
        let sessions = required_activation_uuid(collection_ids, SESSIONS)?;
        let receipts = required_activation_uuid(collection_ids, COORDINATION_RECEIPTS)?;
        let mut stored = Document::new();
        stored.insert(SESSIONS, sessions.clone());
        stored.insert(COORDINATION_RECEIPTS, receipts.clone());
        document.insert("collection_uuids", stored);
    }
    Ok(document)
}

fn validate_coordination_activation_marker(
    document: Document,
) -> Result<CoordinationActivationMarker, StoreError> {
    let id = document.get_str("_id").map_err(|_| {
        StoreError::Malformed("MongoDB coordination-activation marker _id must be a string".into())
    })?;
    let protocol = document.get_i32("protocol").map_err(|_| {
        StoreError::Malformed(
            "MongoDB coordination-activation marker protocol must be an int".into(),
        )
    })?;
    let origin = document.get_str("origin").map_err(|_| {
        StoreError::Malformed(
            "MongoDB coordination-activation marker origin must be a string".into(),
        )
    })?;
    let phase = document.get_str("phase").map_err(|_| {
        StoreError::Malformed(
            "MongoDB coordination-activation marker phase must be a string".into(),
        )
    })?;
    if id != COORDINATION_ACTIVATION_ID
        || !matches!(
            protocol,
            INITIAL_COORDINATION_ACTIVATION_PROTOCOL
                | LEGACY_COORDINATION_ACTIVATION_PROTOCOL
                | COORDINATION_ACTIVATION_PROTOCOL
        )
    {
        return Err(StoreError::Malformed(format!(
            "MongoDB coordination-activation marker is not a supported {COORDINATION_ACTIVATION_ID} protocol proof"
        )));
    }
    validate_coordination_activation_origin(origin)?;
    match (protocol, phase) {
        (
            INITIAL_COORDINATION_ACTIVATION_PROTOCOL | LEGACY_COORDINATION_ACTIVATION_PROTOCOL,
            "in-progress",
        ) if document.len() == 4 => {
            Ok(CoordinationActivationMarker::LegacyInProgress {
                protocol,
                origin: origin.to_owned(),
            })
        }
        (
            INITIAL_COORDINATION_ACTIVATION_PROTOCOL | LEGACY_COORDINATION_ACTIVATION_PROTOCOL,
            "complete",
        ) if document.len() == 5 => {
            Ok(CoordinationActivationMarker::LegacyComplete {
                protocol,
                origin: origin.to_owned(),
                collection_ids: stored_coordination_activation_ids(&document)?,
            })
        }
        (LEGACY_COORDINATION_ACTIVATION_PROTOCOL, "upgrade-in-progress")
            if document.len() == 5 =>
        {
            Ok(CoordinationActivationMarker::LegacyUpgradeInProgress {
                protocol,
                origin: origin.to_owned(),
                collection_ids: stored_coordination_activation_ids(&document)?,
            })
        }
        (COORDINATION_ACTIVATION_PROTOCOL, "in-progress") if document.len() == 4 => {
            Ok(CoordinationActivationMarker::InProgress {
                origin: origin.to_owned(),
            })
        }
        (COORDINATION_ACTIVATION_PROTOCOL, "upgrade-in-progress")
            if document.len() == 5 =>
        {
            Ok(CoordinationActivationMarker::UpgradeInProgress {
                origin: origin.to_owned(),
                collection_ids: stored_coordination_activation_ids(&document)?,
            })
        }
        (COORDINATION_ACTIVATION_PROTOCOL, "complete") if document.len() == 5 => {
            Ok(CoordinationActivationMarker::Complete {
                origin: origin.to_owned(),
                collection_ids: stored_coordination_activation_ids(&document)?,
            })
        }
        _ => Err(StoreError::Malformed(
            "MongoDB coordination-activation marker has an unsupported protocol, phase, or field set"
                .into(),
        )),
    }
}

fn stored_coordination_activation_ids(
    document: &Document,
) -> Result<BTreeMap<String, Binary>, StoreError> {
    let stored = document.get_document("collection_uuids").map_err(|_| {
        StoreError::Malformed(
            "completed MongoDB coordination-activation marker must contain collection_uuids".into(),
        )
    })?;
    if stored.len() != 2 {
        return Err(StoreError::Malformed(
            "completed MongoDB coordination-activation marker must name exactly the sessions and receipt collection UUIDs"
                .into(),
        ));
    }
    Ok(BTreeMap::from([
        (
            SESSIONS.to_owned(),
            stored_activation_uuid(stored, SESSIONS)?,
        ),
        (
            COORDINATION_RECEIPTS.to_owned(),
            stored_activation_uuid(stored, COORDINATION_RECEIPTS)?,
        ),
    ]))
}

fn validate_coordination_activation_origin(origin: &str) -> Result<(), StoreError> {
    if matches!(origin, "new-database" | "operator-drained") {
        return Ok(());
    }
    Err(StoreError::Malformed(
        "MongoDB coordination-activation marker has an unsupported origin".into(),
    ))
}

fn required_activation_uuid<'a>(
    collection_ids: &'a BTreeMap<String, Binary>,
    collection: &str,
) -> Result<&'a Binary, StoreError> {
    let uuid = collection_ids.get(collection).ok_or_else(|| {
        StoreError::Malformed(format!(
            "completed MongoDB coordination-activation marker is missing the {collection} UUID"
        ))
    })?;
    validate_cutover_uuid(collection, uuid)?;
    Ok(uuid)
}

fn stored_activation_uuid(document: &Document, collection: &str) -> Result<Binary, StoreError> {
    let uuid = match document.get(collection) {
        Some(Bson::Binary(uuid)) => uuid.clone(),
        _ => {
            return Err(StoreError::Malformed(format!(
                "completed MongoDB coordination-activation marker {collection} UUID must be binary"
            )))
        }
    };
    validate_cutover_uuid(collection, &uuid)?;
    Ok(uuid)
}

fn coordination_activation_ids(
    collection_ids: &BTreeMap<String, Binary>,
) -> Result<BTreeMap<String, Binary>, StoreError> {
    Ok(BTreeMap::from([
        (
            SESSIONS.to_owned(),
            required_activation_uuid(collection_ids, SESSIONS)?.clone(),
        ),
        (
            COORDINATION_RECEIPTS.to_owned(),
            required_activation_uuid(collection_ids, COORDINATION_RECEIPTS)?.clone(),
        ),
    ]))
}

fn coordination_activation_origin(
    database_was_pristine: bool,
    coordination_cutover_confirmed: bool,
) -> Result<&'static str, StoreError> {
    if database_was_pristine {
        return Ok("new-database");
    }
    if coordination_cutover_confirmed {
        return Ok("operator-drained");
    }
    Err(StoreError::Unavailable(
        "an existing MongoDB database requires an offline coordination activation: stop every pre-coordination CMA process, wait for all MongoDB and external provider operations to finish, then start exactly one replacement with CHRONO_MONGODB_COORDINATION_CUTOVER=drained; CMA cannot prove that drain itself"
            .into(),
    ))
}

fn validate_coordination_activation_ids(
    recorded: &BTreeMap<String, Binary>,
    actual: &BTreeMap<String, Binary>,
) -> Result<(), StoreError> {
    for collection in [SESSIONS, COORDINATION_RECEIPTS] {
        let recorded_uuid = required_activation_uuid(recorded, collection)?;
        let actual_uuid = actual.get(collection).ok_or_else(|| {
            StoreError::Unavailable(format!(
                "completed MongoDB coordination activation no longer has protected collection {collection}; refusing to recreate lost coordination state"
            ))
        })?;
        if recorded_uuid != actual_uuid {
            return Err(StoreError::Unavailable(format!(
                "completed MongoDB coordination activation {collection} UUID no longer matches its durable completion record; refusing replacement data"
            )));
        }
    }
    Ok(())
}

fn validate_coordination_receipts_provenance(
    document: Document,
) -> Result<CoordinationReceiptsProvenance, StoreError> {
    let id = document.get_str("_id").map_err(|_| {
        StoreError::Malformed("MongoDB coordination-receipt provenance _id must be a string".into())
    })?;
    let protocol = document.get_i32("protocol").map_err(|_| {
        StoreError::Malformed(
            "MongoDB coordination-receipt provenance protocol must be an int".into(),
        )
    })?;
    let phase = document.get_str("phase").map_err(|_| {
        StoreError::Malformed(
            "MongoDB coordination-receipt provenance phase must be a string".into(),
        )
    })?;
    if id != COORDINATION_RECEIPTS_PROVENANCE_ID
        || protocol != COORDINATION_RECEIPTS_PROVENANCE_PROTOCOL
    {
        return Err(StoreError::Malformed(format!(
            "MongoDB coordination-receipt provenance is not the supported {COORDINATION_RECEIPTS_PROVENANCE_ID} protocol {COORDINATION_RECEIPTS_PROVENANCE_PROTOCOL} proof"
        )));
    }
    match phase {
        "in-progress" if document.len() == 3 => Ok(CoordinationReceiptsProvenance::InProgress),
        "complete" if document.len() == 4 => {
            let collection_id = match document.get("collection_uuid") {
                Some(Bson::Binary(id)) => id.clone(),
                _ => {
                    return Err(StoreError::Malformed(
                        "completed MongoDB coordination-receipt provenance must carry a binary collection_uuid"
                            .into(),
                    ))
                }
            };
            validate_cutover_uuid(COORDINATION_RECEIPTS, &collection_id)?;
            Ok(CoordinationReceiptsProvenance::Complete { collection_id })
        }
        _ => Err(StoreError::Malformed(
            "MongoDB coordination-receipt provenance has an unsupported phase or field set".into(),
        )),
    }
}

fn validate_namespace_cutover_marker(
    document: Document,
) -> Result<NamespaceCutoverMarker, StoreError> {
    let id = document.get_str("_id").map_err(|_| {
        StoreError::Malformed("MongoDB namespace-cutover marker _id must be a string".into())
    })?;
    let protocol = document.get_i32("protocol").map_err(|_| {
        StoreError::Malformed("MongoDB namespace-cutover marker protocol must be an int".into())
    })?;
    let origin = document.get_str("origin").map_err(|_| {
        StoreError::Malformed("MongoDB namespace-cutover marker origin must be a string".into())
    })?;
    let phase = document.get_str("phase").map_err(|_| {
        StoreError::Malformed("MongoDB namespace-cutover marker phase must be a string".into())
    })?;
    if id != NAMESPACE_CUTOVER_ID || protocol != NAMESPACE_CUTOVER_PROTOCOL {
        return Err(StoreError::Malformed(format!(
            "MongoDB namespace-cutover marker is not the supported {NAMESPACE_CUTOVER_ID} protocol {NAMESPACE_CUTOVER_PROTOCOL} proof"
        )));
    }
    match phase {
        "in-progress" if document.len() == 6 => {
            validate_namespace_cutover_origin(origin, false)?;
            let source_ids = stored_namespace_cutover_source(&document)?;
            validate_namespace_cutover_source_origin(origin, &source_ids)?;
            let tombstone_id = match document.get("tombstone_collection_uuid") {
                Some(Bson::Binary(uuid)) => {
                    validate_cutover_uuid(TOMBSTONES, uuid)?;
                    Some(uuid.clone())
                }
                Some(Bson::Null) => None,
                _ => {
                    return Err(StoreError::Malformed(
                        "in-progress MongoDB namespace-cutover marker tombstone_collection_uuid must be a binary UUID or null"
                            .into(),
                    ))
                }
            };
            validate_in_progress_tombstone_binding(&source_ids, tombstone_id.as_ref())?;
            Ok(NamespaceCutoverMarker::InProgress {
                origin: origin.to_owned(),
                source_ids,
                tombstone_id,
            })
        }
        "complete" if document.len() == 5 => {
            validate_namespace_cutover_origin(origin, true)?;
            let stored = document.get_document("collection_uuids").map_err(|_| {
                StoreError::Malformed(
                    "completed MongoDB namespace-cutover marker must contain collection_uuids"
                        .into(),
                )
            })?;
            if stored.len() != LEGACY_CUTOVER_COLLECTIONS.len() {
                return Err(StoreError::Malformed(
                    "completed MongoDB namespace-cutover marker must name exactly the two authoritative collection UUIDs and the tombstone collection UUID"
                        .into(),
                ));
            }
            let collection_ids = BTreeMap::from([
                (
                    PRE_COORDINATION_SESSIONS.to_owned(),
                    stored_cutover_uuid(stored, PRE_COORDINATION_SESSIONS)?,
                ),
                (EVENTS.to_owned(), stored_cutover_uuid(stored, EVENTS)?),
                (
                    TOMBSTONES.to_owned(),
                    stored_cutover_uuid(stored, TOMBSTONES)?,
                ),
            ]);
            Ok(NamespaceCutoverMarker::Complete {
                origin: origin.to_owned(),
                collection_ids,
            })
        }
        _ => Err(StoreError::Malformed(
            "MongoDB namespace-cutover marker has an unsupported phase or field set".into(),
        )),
    }
}

fn stored_namespace_cutover_source(
    document: &Document,
) -> Result<BTreeMap<String, Option<Binary>>, StoreError> {
    let stored = document
        .get_document("source_collection_uuids")
        .map_err(|_| {
            StoreError::Malformed(
                "in-progress MongoDB namespace-cutover marker must contain source_collection_uuids"
                    .into(),
            )
        })?;
    if stored.len() != LEGACY_CUTOVER_SOURCES.len() {
        return Err(StoreError::Malformed(
            "in-progress MongoDB namespace-cutover marker must name exactly the two raw sources and the tombstone source state"
                .into(),
        ));
    }
    let mut source_ids = BTreeMap::new();
    for collection in LEGACY_CUTOVER_SOURCES {
        let source = match stored.get(collection) {
            Some(Bson::Binary(uuid)) => {
                validate_cutover_uuid(collection, uuid)?;
                Some(uuid.clone())
            }
            Some(Bson::Null) => None,
            _ => {
                return Err(StoreError::Malformed(format!(
                    "in-progress MongoDB namespace-cutover marker source {collection} must be a binary UUID or null"
                )))
            }
        };
        source_ids.insert(collection.to_owned(), source);
    }
    Ok(source_ids)
}

fn validate_namespace_cutover_source_origin(
    origin: &str,
    source_ids: &BTreeMap<String, Option<Binary>>,
) -> Result<(), StoreError> {
    let present = [LEGACY_SESSIONS, LEGACY_EVENTS]
        .into_iter()
        .filter(|collection| source_ids.get(*collection).is_some_and(Option::is_some))
        .count();
    match origin {
        "new-database" if present == 0 => Ok(()),
        "operator-drained" if present > 0 => Ok(()),
        "new-database" => Err(StoreError::Malformed(
            "new-database MongoDB namespace-cutover provenance cannot bind an existing raw source"
                .into(),
        )),
        "operator-drained" => Err(StoreError::Malformed(
            "operator-drained MongoDB namespace-cutover provenance must bind at least one raw source"
                .into(),
        )),
        _ => Err(StoreError::Malformed(
            "in-progress MongoDB namespace-cutover marker has an unsupported origin".into(),
        )),
    }
}

fn validate_namespace_cutover_origin(origin: &str, complete: bool) -> Result<(), StoreError> {
    if matches!(origin, "new-database" | "operator-drained")
        || (complete && origin == NAMESPACE_CUTOVER_OBSERVED_ORIGIN)
    {
        return Ok(());
    }
    Err(StoreError::Malformed(
        "MongoDB namespace-cutover marker has an unsupported origin".into(),
    ))
}

fn required_cutover_uuid<'a>(
    collection_ids: &'a BTreeMap<String, Binary>,
    collection: &str,
) -> Result<&'a Binary, StoreError> {
    let uuid = collection_ids.get(collection).ok_or_else(|| {
        StoreError::Malformed(format!(
            "completed MongoDB namespace-cutover marker is missing the {collection} UUID"
        ))
    })?;
    validate_cutover_uuid(collection, uuid)?;
    Ok(uuid)
}

fn stored_cutover_uuid(document: &Document, collection: &str) -> Result<Binary, StoreError> {
    let uuid = match document.get(collection) {
        Some(Bson::Binary(uuid)) => uuid.clone(),
        _ => {
            return Err(StoreError::Malformed(format!(
                "completed MongoDB namespace-cutover marker {collection} UUID must be binary"
            )))
        }
    };
    validate_cutover_uuid(collection, &uuid)?;
    Ok(uuid)
}

fn validate_cutover_uuid(collection: &str, uuid: &Binary) -> Result<(), StoreError> {
    if uuid.subtype == mongodb::bson::spec::BinarySubtype::Uuid && uuid.bytes.len() == 16 {
        return Ok(());
    }
    Err(StoreError::Malformed(format!(
        "completed MongoDB namespace-cutover marker {collection} UUID is not a standard UUID"
    )))
}

fn completed_namespace_ids(
    sessions: &NamespacePair,
    events: &NamespacePair,
) -> Result<Option<BTreeMap<String, Binary>>, StoreError> {
    let ready = matches!(
        next_namespace_action(sessions, LEGACY_SESSIONS, PRE_COORDINATION_SESSIONS,),
        Ok(NamespaceAction::Ready)
    ) && matches!(
        next_namespace_action(events, LEGACY_EVENTS, EVENTS),
        Ok(NamespaceAction::Ready)
    );
    if !ready {
        return Ok(None);
    }
    let sessions_uuid = sessions.authoritative_uuid.clone().ok_or_else(|| {
        StoreError::Unavailable(
            "completed MongoDB namespace cutover sessions_v2 collection has no stable UUID".into(),
        )
    })?;
    let events_uuid = events.authoritative_uuid.clone().ok_or_else(|| {
        StoreError::Unavailable(
            "completed MongoDB namespace cutover session_events_v2 collection has no stable UUID"
                .into(),
        )
    })?;
    Ok(Some(BTreeMap::from([
        (PRE_COORDINATION_SESSIONS.to_owned(), sessions_uuid),
        (EVENTS.to_owned(), events_uuid),
    ])))
}

fn validate_completed_namespace_ids(
    recorded: &BTreeMap<String, Binary>,
    actual: &BTreeMap<String, Binary>,
) -> Result<(), StoreError> {
    for collection in [PRE_COORDINATION_SESSIONS, EVENTS] {
        let recorded_uuid = required_cutover_uuid(recorded, collection)?;
        let actual_uuid = actual.get(collection).ok_or_else(|| {
            StoreError::Unavailable(format!(
                "completed MongoDB namespace cutover no longer has authoritative collection {collection}; refusing to recreate or replace it"
            ))
        })?;
        if actual_uuid != recorded_uuid {
            return Err(StoreError::Unavailable(format!(
                "completed MongoDB namespace cutover {collection} UUID no longer matches its durable completion record; refusing to accept replacement data"
            )));
        }
    }
    Ok(())
}

fn validate_completed_cutover_ids(
    recorded: &BTreeMap<String, Binary>,
    actual: &BTreeMap<String, Binary>,
) -> Result<(), StoreError> {
    validate_completed_namespace_ids(recorded, actual)?;
    let recorded_uuid = required_cutover_uuid(recorded, TOMBSTONES)?;
    let actual_uuid = actual.get(TOMBSTONES).ok_or_else(|| {
        StoreError::Unavailable(format!(
            "completed MongoDB namespace cutover no longer has protected collection {TOMBSTONES}; refusing to recreate lost deletion state"
        ))
    })?;
    if actual_uuid != recorded_uuid {
        return Err(StoreError::Unavailable(format!(
            "completed MongoDB namespace cutover {TOMBSTONES} UUID no longer matches its durable completion record; refusing replacement deletion state"
        )));
    }
    Ok(())
}

fn validate_migrated_namespace_ids(
    recorded: &BTreeMap<String, Binary>,
    actual: &BTreeMap<String, Binary>,
) -> Result<(), StoreError> {
    for (recorded_collection, actual_collection) in
        [(PRE_COORDINATION_SESSIONS, SESSIONS), (EVENTS, EVENTS)]
    {
        let recorded_uuid = required_cutover_uuid(recorded, recorded_collection)?;
        let actual_uuid = actual.get(actual_collection).ok_or_else(|| {
            StoreError::Unavailable(format!(
                "completed MongoDB namespace cutover no longer has migrated collection {actual_collection}; refusing to recreate or replace it"
            ))
        })?;
        if recorded_uuid != actual_uuid {
            return Err(StoreError::Unavailable(format!(
                "completed MongoDB namespace cutover {recorded_collection} UUID no longer matches migrated collection {actual_collection}; refusing replacement data"
            )));
        }
    }
    Ok(())
}

fn namespace_is_empty_view(kind: Option<&NamespaceKind>, source: &str) -> bool {
    matches!(
        kind,
        Some(NamespaceKind::View {
            source: Some(actual),
            pipeline_is_empty: true,
        }) if actual == source
    )
}

fn coordination_migration_session_id(pair: &NamespacePair) -> Result<&Binary, StoreError> {
    match (&pair.legacy, &pair.authoritative) {
        (Some(NamespaceKind::Collection), None) => pair.legacy_uuid.as_ref(),
        (None, Some(NamespaceKind::Collection)) => pair.authoritative_uuid.as_ref(),
        (
            Some(NamespaceKind::View {
                source: Some(source),
                pipeline_is_empty: true,
            }),
            Some(NamespaceKind::Collection),
        ) if source == SESSIONS => pair.authoritative_uuid.as_ref(),
        _ => None,
    }
    .ok_or_else(|| {
        StoreError::Unavailable(format!(
            "MongoDB coordination activation cannot prove one preserved session collection across {PRE_COORDINATION_SESSIONS} -> {SESSIONS}: {pair:?}"
        ))
    })
}

fn validate_markerless_coordination_activation_source(
    sessions: &NamespacePair,
    receipts_provenance_exists: bool,
    receipts_collection_exists: bool,
) -> Result<(), StoreError> {
    if !matches!(
        (&sessions.legacy, &sessions.authoritative),
        (Some(NamespaceKind::Collection), None)
    ) {
        return Err(StoreError::Unavailable(format!(
            "MongoDB coordination activation marker is missing but the session namespace is not an untouched pre-coordination source: expected {PRE_COORDINATION_SESSIONS} to be a collection and {SESSIONS} to be absent, found {sessions:?}; refusing to create replacement provenance for a possibly activated database"
        )));
    }
    if receipts_provenance_exists || receipts_collection_exists {
        return Err(StoreError::Unavailable(
            "MongoDB coordination activation marker is missing but coordination-receipt state already exists; refusing to create replacement provenance for a possibly activated database"
                .into(),
        ));
    }
    Ok(())
}

fn validate_exact_coordination_receipts_id(
    expected: &Binary,
    actual: &Binary,
) -> Result<(), StoreError> {
    if expected == actual {
        return Ok(());
    }
    Err(StoreError::Unavailable(format!(
        "MongoDB coordination-receipt collection {COORDINATION_RECEIPTS} UUID no longer matches its durable provenance; refusing replacement data"
    )))
}

fn namespace_cutover_authorization(
    sessions: &NamespacePair,
    events: &NamespacePair,
    legacy_cutover_confirmed: bool,
    marker: Option<&NamespaceCutoverMarker>,
) -> Result<NamespaceCutoverAuthorization, StoreError> {
    let new_database = [sessions, events]
        .iter()
        .all(|pair| pair.legacy.is_none() && pair.authoritative.is_none());
    let completed = completed_namespace_ids(sessions, events)?;
    match marker {
        Some(NamespaceCutoverMarker::Complete { collection_ids, .. }) => {
            let actual = completed.as_ref().ok_or_else(|| {
                StoreError::Unavailable(
                    "completed MongoDB namespace cutover is no longer complete; refusing to recreate missing namespaces from an empty state"
                        .into(),
                )
            })?;
            validate_completed_namespace_ids(collection_ids, actual)?;
            return Ok(NamespaceCutoverAuthorization::Complete);
        }
        Some(NamespaceCutoverMarker::InProgress { origin, .. }) => {
            if let Some(collection_ids) = completed {
                return Ok(NamespaceCutoverAuthorization::RecordCompletion {
                    origin: origin.clone(),
                    collection_ids,
                });
            }
            return Ok(NamespaceCutoverAuthorization::Proven);
        }
        None => {}
    }
    if completed.is_some() {
        return Err(StoreError::Unavailable(
            "completed MongoDB namespace cutover has no durable in-progress provenance; refusing to adopt markerless protected namespaces"
                .into(),
        ));
    }
    if new_database {
        return Ok(NamespaceCutoverAuthorization::RecordNewDatabase);
    }
    if legacy_cutover_confirmed {
        return Ok(NamespaceCutoverAuthorization::RecordOperatorDrain);
    }

    Err(StoreError::Unavailable(
        "legacy MongoDB session namespaces require an offline cutover: stop every pre-cutover CMA replica and wait for all of its MongoDB operations to finish, then start exactly one replacement with CHRONO_MONGODB_LEGACY_CUTOVER=drained; CMA cannot prove that external drain itself"
            .into(),
    ))
}

fn namespace_cutover_source_ids(
    sessions: &NamespacePair,
    events: &NamespacePair,
    tombstone_id: Option<Binary>,
) -> Result<BTreeMap<String, Option<Binary>>, StoreError> {
    let mut source_ids = BTreeMap::new();
    for (legacy, authoritative, pair) in [
        (LEGACY_SESSIONS, PRE_COORDINATION_SESSIONS, sessions),
        (LEGACY_EVENTS, EVENTS, events),
    ] {
        if pair.authoritative.is_some() {
            return Err(StoreError::Unavailable(format!(
                "cannot record MongoDB namespace-cutover intent after authoritative namespace {authoritative} already exists"
            )));
        }
        let source = match pair.legacy.as_ref() {
            None => None,
            Some(NamespaceKind::Collection) => {
                let uuid = pair.legacy_uuid.clone().ok_or_else(|| {
                    StoreError::Unavailable(format!(
                        "raw MongoDB namespace-cutover source {legacy} has no stable collection UUID"
                    ))
                })?;
                validate_cutover_uuid(legacy, &uuid)?;
                Some(uuid)
            }
            _ => {
                return Err(StoreError::Unavailable(format!(
                    "cannot record MongoDB namespace-cutover intent from non-raw source {legacy}: {pair:?}"
                )))
            }
        };
        source_ids.insert(legacy.to_owned(), source);
    }
    source_ids.insert(TOMBSTONES.to_owned(), tombstone_id);
    Ok(source_ids)
}

fn validate_namespace_cutover_source_ids(
    source_ids: &BTreeMap<String, Option<Binary>>,
    sessions: &NamespacePair,
    events: &NamespacePair,
) -> Result<(), StoreError> {
    if source_ids.len() != LEGACY_CUTOVER_SOURCES.len() {
        return Err(StoreError::Malformed(
            "in-progress MongoDB namespace-cutover provenance does not name exactly the two raw sources and tombstone source state"
                .into(),
        ));
    }
    if !source_ids.contains_key(TOMBSTONES) {
        return Err(StoreError::Malformed(format!(
            "in-progress MongoDB namespace-cutover provenance is missing source {TOMBSTONES}"
        )));
    }
    for (legacy, authoritative, pair) in [
        (LEGACY_SESSIONS, PRE_COORDINATION_SESSIONS, sessions),
        (LEGACY_EVENTS, EVENTS, events),
    ] {
        let expected = source_ids.get(legacy).ok_or_else(|| {
            StoreError::Malformed(format!(
                "in-progress MongoDB namespace-cutover provenance is missing source {legacy}"
            ))
        })?;
        match expected {
            Some(expected) => {
                let actual = if pair.authoritative.is_some() {
                    pair.authoritative_uuid.as_ref()
                } else if pair.legacy.is_some() {
                    pair.legacy_uuid.as_ref()
                } else {
                    None
                };
                let Some(actual) = actual else {
                    return Err(StoreError::Unavailable(format!(
                        "raw MongoDB namespace-cutover source {legacy} disappeared after its UUID was durably recorded; refusing to adopt absence or replacement data"
                    )));
                };
                if actual != expected {
                    return Err(StoreError::Unavailable(format!(
                        "raw MongoDB namespace-cutover source {legacy} UUID no longer matches its durable provenance across {legacy} -> {authoritative}; refusing replacement data"
                    )));
                }
            }
            None if matches!(pair.legacy.as_ref(), Some(NamespaceKind::Collection)) => {
                return Err(StoreError::Unavailable(format!(
                    "raw MongoDB namespace-cutover source {legacy} was durably absent but a collection appeared under that name; refusing unproved post-marker data"
                )))
            }
            None => {}
        }
    }
    Ok(())
}

fn validate_raw_legacy_stage_source(
    sessions: &NamespacePair,
    events: &NamespacePair,
    contracts: &BTreeMap<String, CollectionContract>,
) -> Result<(), StoreError> {
    validate_legacy_stage_namespace_contracts(sessions, events, contracts)?;
    let mut legacy_collections = 0;
    for (legacy, authoritative, pair) in [
        (LEGACY_SESSIONS, PRE_COORDINATION_SESSIONS, sessions),
        (LEGACY_EVENTS, EVENTS, events),
    ] {
        if pair.authoritative.is_some() {
            return Err(StoreError::Unavailable(format!(
                "legacy MongoDB cutover stage found markerless {authoritative} state; refusing to adopt a partially migrated or completed namespace"
            )));
        }
        match pair.legacy.as_ref() {
            None => {}
            Some(NamespaceKind::Collection)
                if !pair.legacy_marked && !pair.legacy_fenced =>
            {
                legacy_collections += 1;
            }
            _ => {
                return Err(StoreError::Unavailable(format!(
                    "legacy MongoDB cutover stage requires {legacy} to be an untouched collection or absent before provenance is recorded; found {pair:?}"
                )))
            }
        }
    }
    if legacy_collections == 0 {
        return Err(StoreError::Unavailable(
            "legacy MongoDB cutover stage requires at least one raw legacy session namespace; a fresh or markerless empty source is not an operator-drained recovery boundary"
                .into(),
        ));
    }
    Ok(())
}

fn validate_legacy_stage_namespace_contracts(
    sessions: &NamespacePair,
    events: &NamespacePair,
    contracts: &BTreeMap<String, CollectionContract>,
) -> Result<(), StoreError> {
    for (legacy, authoritative, pair) in [
        (LEGACY_SESSIONS, PRE_COORDINATION_SESSIONS, sessions),
        (LEGACY_EVENTS, EVENTS, events),
    ] {
        validate_legacy_stage_namespace_contract(
            legacy,
            authoritative,
            pair.legacy.as_ref(),
            pair.legacy_marked,
            pair.legacy_fenced,
            contracts.get(legacy),
        )?;
        validate_legacy_stage_namespace_contract(
            authoritative,
            authoritative,
            pair.authoritative.as_ref(),
            pair.authoritative_marked,
            pair.authoritative_fenced,
            contracts.get(authoritative),
        )?;
    }
    Ok(())
}

fn validate_legacy_stage_namespace_contract(
    collection: &str,
    authoritative: &str,
    kind: Option<&NamespaceKind>,
    marked: bool,
    fenced: bool,
    contract: Option<&CollectionContract>,
) -> Result<(), StoreError> {
    let (Some(kind), Some(contract)) = (kind, contract) else {
        if kind.is_none() && contract.is_none() {
            return Ok(());
        }
        return Err(StoreError::Unavailable(format!(
            "legacy MongoDB cutover stage observed an unstable contract for {collection}; refusing to mutate it"
        )));
    };
    if &contract.kind != kind {
        return Err(StoreError::Unavailable(format!(
            "legacy MongoDB cutover stage observed an unstable namespace kind for {collection}; refusing to mutate it"
        )));
    }

    match kind {
        NamespaceKind::Collection => {
            let uuid = contract.uuid.as_ref().ok_or_else(|| {
                StoreError::Unavailable(format!(
                    "legacy MongoDB cutover stage requires {collection} to have a stable collection UUID"
                ))
            })?;
            validate_cutover_uuid(collection, uuid)?;
            match (marked, fenced) {
                (false, false) => {
                    validate_stage_unvalidated_collection_contract(collection, contract)
                }
                (true, true) => validate_collection_contract(authoritative, contract, None),
                (true, false)
                    if authoritative == EVENTS
                        && contract.validator.as_ref().is_some_and(|validator| {
                            documents_equivalent(validator, &event_protocol_validator())
                        })
                        && contract.validation_level == Some(ValidationLevel::Strict)
                        && contract.validation_action == Some(ValidationAction::Warn) =>
                {
                    Ok(())
                }
                _ => Err(StoreError::Unavailable(format!(
                    "legacy MongoDB cutover stage found an unproved validator contract on {collection}; refusing to overwrite it"
                ))),
            }
        }
        NamespaceKind::View {
            source: Some(source),
            pipeline_is_empty: true,
        } if source == authoritative
            && !marked
            && !fenced
            && contract.validator.is_none()
            && contract.validation_level.is_none()
            && contract.validation_action.is_none()
            && contract.uuid.is_none() =>
        {
            Ok(())
        }
        _ => Err(StoreError::Unavailable(format!(
            "legacy MongoDB cutover stage found an unproved namespace contract on {collection}; refusing to replace it"
        ))),
    }
}

fn validate_stage_unvalidated_collection_contract(
    collection: &str,
    contract: &CollectionContract,
) -> Result<(), StoreError> {
    if contract.kind != NamespaceKind::Collection
        || contract.validator.is_some()
        || contract.validation_level.is_some()
        || contract.validation_action.is_some()
    {
        return Err(StoreError::Unavailable(format!(
            "legacy MongoDB cutover stage found an unproved validator contract on {collection}; refusing to overwrite it"
        )));
    }
    let uuid = contract.uuid.as_ref().ok_or_else(|| {
        StoreError::Unavailable(format!(
            "legacy MongoDB cutover stage requires {collection} to have a stable collection UUID"
        ))
    })?;
    validate_cutover_uuid(collection, uuid)
}

fn next_namespace_action(
    pair: &NamespacePair,
    legacy: &str,
    authoritative: &str,
) -> Result<NamespaceAction, StoreError> {
    match (&pair.legacy, &pair.authoritative) {
        (None, None) => Ok(NamespaceAction::CreateAuthoritative),
        (Some(NamespaceKind::Collection), None) if !pair.legacy_fenced => {
            Ok(NamespaceAction::FenceLegacy)
        }
        (Some(NamespaceKind::Collection), None) => Ok(NamespaceAction::RenameLegacy),
        (None, Some(NamespaceKind::Collection)) if !pair.authoritative_fenced => {
            Ok(NamespaceAction::FenceAuthoritative)
        }
        (None, Some(NamespaceKind::Collection)) => Ok(NamespaceAction::CreateLegacyView),
        (Some(NamespaceKind::Collection), Some(NamespaceKind::Collection))
            if legacy == LEGACY_EVENTS
                && authoritative == EVENTS
                && pair.authoritative_fenced
                && !pair.legacy_fenced =>
        {
            Ok(NamespaceAction::FenceLateLegacy)
        }
        (Some(NamespaceKind::Collection), Some(NamespaceKind::Collection))
            if legacy == LEGACY_EVENTS
                && authoritative == EVENTS
                && pair.authoritative_fenced
                && pair.legacy_fenced =>
        {
            Ok(NamespaceAction::MergeLateLegacy)
        }
        (
            Some(NamespaceKind::View {
                source: Some(source),
                pipeline_is_empty: true,
            }),
            Some(NamespaceKind::Collection),
        ) if source == authoritative && !pair.authoritative_fenced => {
            Ok(NamespaceAction::FenceAuthoritative)
        }
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

fn cutover_session_owner_namespace(
    pair: &NamespacePair,
) -> Result<Option<&'static str>, StoreError> {
    match (&pair.legacy, &pair.authoritative) {
        (None, None) => Ok(None),
        (Some(NamespaceKind::Collection), None) if pair.legacy_fenced => {
            Ok(Some(LEGACY_SESSIONS))
        }
        (None, Some(NamespaceKind::Collection)) if pair.authoritative_fenced => {
            Ok(Some(PRE_COORDINATION_SESSIONS))
        }
        (
            Some(NamespaceKind::View {
                source: Some(source),
                pipeline_is_empty: true,
            }),
            Some(NamespaceKind::Collection),
        ) if source == PRE_COORDINATION_SESSIONS && pair.authoritative_fenced => {
            Ok(Some(PRE_COORDINATION_SESSIONS))
        }
        _ => Err(StoreError::Unavailable(format!(
            "MongoDB session owner namespace is ambiguous during cutover: {pair:?}; refusing delete reconciliation"
        ))),
    }
}

fn resolve_cutover_session_owner_lookup(
    before: &NamespacePair,
    after: &NamespacePair,
    owner_found: bool,
) -> Result<CutoverOwnerLookup, StoreError> {
    let before_collection = cutover_session_owner_namespace(before)?;
    // Validate the second snapshot even when it changed. In particular, a
    // legacy collection appearing beside the authoritative collection must
    // fail closed before an apparent owner miss can create a tombstone.
    let after_collection = cutover_session_owner_namespace(after)?;
    if before != after || before_collection != after_collection {
        return Ok(CutoverOwnerLookup::Retry);
    }
    if before_collection.is_none() {
        return Err(StoreError::Unavailable(
            "MongoDB cutover has protected event history but no fenced session owner namespace; refusing to infer a delete"
                .into(),
        ));
    }
    Ok(if owner_found {
        CutoverOwnerLookup::Present
    } else {
        CutoverOwnerLookup::Absent
    })
}

fn normalize_cutover_event_document(mut document: Document) -> Result<CutoverEvent, StoreError> {
    let event = event_doc::from_document(document.clone())?;
    document.remove("_id");
    document.insert(WRITE_PROTOCOL_FIELD, WRITE_PROTOCOL);
    let position = doc! {
        "session_id": &event.session_id.0,
        "seq": i64::try_from(event.seq).map_err(|_| {
            StoreError::Malformed(format!(
                "late legacy event {}:{} exceeds MongoDB's sequence range",
                event.session_id, event.seq
            ))
        })?,
    };
    Ok(CutoverEvent {
        session_id: event.session_id.0,
        seq: event.seq,
        position,
        document,
    })
}

fn validate_same_cutover_event(left: &Document, right: &Document) -> Result<(), StoreError> {
    let left = canonical_cutover_event(left.clone())?;
    let right = canonical_cutover_event(right.clone())?;
    if documents_equivalent(&left, &right) {
        return Ok(());
    }
    let id = right
        .get_str("session_id")
        .unwrap_or("<invalid session id>");
    let seq = right.get_i64("seq").unwrap_or(-1);
    Err(StoreError::Malformed(format!(
        "late legacy event {id}:{seq} conflicts with a different authoritative event at the same cursor"
    )))
}

fn canonical_cutover_event(mut document: Document) -> Result<Document, StoreError> {
    event_doc::from_document(document.clone())?;
    document.remove("_id");
    document.remove(WRITE_PROTOCOL_FIELD);
    Ok(document)
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
    session_protocol_validator_for(SESSION_WRITE_PROTOCOL, SESSION_COORDINATION_PROTOCOL)
}

#[cfg(test)]
fn legacy_session_protocol_validator() -> Document {
    session_protocol_validator_for(WRITE_PROTOCOL, LEGACY_SESSION_COORDINATION_PROTOCOL)
}

fn previous_session_protocol_validator() -> Document {
    session_protocol_validator_for(WRITE_PROTOCOL, SESSION_COORDINATION_PROTOCOL)
}

fn session_protocol_validator_for(write_protocol: i32, coordination_protocol: i32) -> Document {
    let mut validator = coordination_protocol_validator(
        protocol_validator(vec!["revision", COORDINATION_PROTOCOL_FIELD], &["revision"]),
        &[coordination_protocol],
    );
    validator
        .get_document_mut("$jsonSchema")
        .expect("the protocol validator has a JSON schema")
        .get_document_mut("properties")
        .expect("the protocol validator has properties")
        .insert(
            WRITE_PROTOCOL_FIELD,
            Bson::Document(doc! {
                "bsonType": "int",
                "enum": [write_protocol],
            }),
        );
    validator
}

fn pre_coordination_session_protocol_validator() -> Document {
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

fn coordination_receipt_protocol_validator() -> Document {
    let mut validator =
        coordination_receipt_protocol_base_validator(&[COORDINATION_RECEIPT_PROTOCOL]);
    validator.insert(
        "$or",
        Bson::Array(vec![
            // Equality with null also matches an absent dotted path, which is
            // the unswitched reservation shape.
            Bson::Document(doc! { "payload.value.switch": Bson::Null }),
            Bson::Document(doc! {
                "payload.value.switch.runtime_started_seq": {
                    "$type": "long",
                    "$gt": 0_i64,
                },
            }),
        ]),
    );
    validator
}

fn coordination_receipt_protocol_base_validator(protocols: &[i32]) -> Document {
    let mut validator = coordination_protocol_validator(
        protocol_validator(vec![COORDINATION_PROTOCOL_FIELD, "payload"], &[]),
        protocols,
    );
    validator
        .get_document_mut("$jsonSchema")
        .expect("the protocol validator has a JSON schema")
        .get_document_mut("properties")
        .expect("the protocol validator has properties")
        .insert(
            "payload",
            Bson::Document(doc! {
                "bsonType": "object",
                "required": ["type", "value"],
                "properties": {
                    "type": { "bsonType": "string" },
                    "value": {
                        "bsonType": "object",
                        "properties": {
                            "completed_by": {
                                "bsonType": "string",
                                "minLength": 1,
                            },
                        },
                    },
                },
            }),
        );
    validator
}

#[cfg(test)]
fn legacy_coordination_receipt_protocol_validator() -> Document {
    coordination_receipt_protocol_base_validator(&[LEGACY_COORDINATION_RECEIPT_PROTOCOL])
}

fn coordination_protocol_validator(mut validator: Document, protocols: &[i32]) -> Document {
    validator
        .get_document_mut("$jsonSchema")
        .expect("the protocol validator has a JSON schema")
        .get_document_mut("properties")
        .expect("the protocol validator has properties")
        .insert(
            COORDINATION_PROTOCOL_FIELD,
            Bson::Document(doc! {
                "bsonType": "int",
                "enum": protocols.iter().copied().map(Bson::Int32).collect::<Vec<_>>(),
            }),
        );
    validator
}

fn current_session_protocol_filter() -> Document {
    doc! {
        WRITE_PROTOCOL_FIELD: SESSION_WRITE_PROTOCOL,
        COORDINATION_PROTOCOL_FIELD: SESSION_COORDINATION_PROTOCOL,
        "revision": { "$type": "long" },
    }
}

fn previous_session_protocol_filter() -> Document {
    doc! {
        WRITE_PROTOCOL_FIELD: { "$eq": WRITE_PROTOCOL, "$type": "int" },
        COORDINATION_PROTOCOL_FIELD: {
            "$eq": SESSION_COORDINATION_PROTOCOL,
            "$type": "int",
        },
        "revision": { "$type": "long" },
    }
}

fn session_write_protocol_migration_update() -> Document {
    doc! { "$set": { WRITE_PROTOCOL_FIELD: SESSION_WRITE_PROTOCOL } }
}

fn session_protocol_migration_update() -> Document {
    doc! {
        "$set": {
            WRITE_PROTOCOL_FIELD: SESSION_WRITE_PROTOCOL,
            COORDINATION_PROTOCOL_FIELD: SESSION_COORDINATION_PROTOCOL,
        },
        "$max": { "revision": 0_i64 },
    }
}

fn appended_event_session_update(record: &Document, tail: i64) -> Document {
    let mut set = Document::new();
    set.insert(WRITE_PROTOCOL_FIELD, SESSION_WRITE_PROTOCOL);
    set.insert(COORDINATION_PROTOCOL_FIELD, SESSION_COORDINATION_PROTOCOL);
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

fn validate_stored_session_identity_and_tail(
    document: &Document,
    revision_required: bool,
) -> Result<(String, u64), StoreError> {
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
        None if !revision_required => {}
        None => {
            return Err(StoreError::Malformed(format!(
                "session {id} revision is absent"
            )))
        }
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
    Ok((id, tail))
}

fn validate_compatible_stored_session_document(
    document: &Document,
) -> Result<(String, u64), StoreError> {
    let (id, tail) = validate_stored_session_identity_and_tail(document, false)?;
    match document.get(WRITE_PROTOCOL_FIELD) {
        None | Some(Bson::Int32(WRITE_PROTOCOL)) => {}
        Some(_) => {
            return Err(StoreError::Malformed(format!(
                "session {id} carries an unknown compatibility write protocol"
            )))
        }
    }
    match document.get(COORDINATION_PROTOCOL_FIELD) {
        None | Some(Bson::Int32(SESSION_COORDINATION_PROTOCOL)) => {}
        Some(_) => {
            return Err(StoreError::Malformed(format!(
                "session {id} carries an unknown compatibility coordination protocol"
            )))
        }
    }
    // Validate the exact payload that migration will retain, with only the
    // legacy protocol markers promoted to their current fenced values.
    let mut promoted = document.clone();
    promoted.insert(WRITE_PROTOCOL_FIELD, SESSION_WRITE_PROTOCOL);
    promoted.insert(COORDINATION_PROTOCOL_FIELD, SESSION_COORDINATION_PROTOCOL);
    let stored: SessionDoc = mongodb::bson::deserialize_from_document(promoted)
        .map_err(|error| StoreError::Malformed(format!("session {id}: {error}")))?;
    from_doc(stored)?;
    Ok((id, tail))
}

fn validate_stored_session_document(document: &Document) -> Result<(String, u64), StoreError> {
    let (id, tail) = validate_stored_session_identity_and_tail(document, true)?;
    if document.get(WRITE_PROTOCOL_FIELD) != Some(&Bson::Int32(SESSION_WRITE_PROTOCOL)) {
        return Err(StoreError::Malformed(format!(
            "session {id} does not carry the current write protocol"
        )));
    }
    let stored: SessionDoc = mongodb::bson::deserialize_from_document(document.clone())
        .map_err(|error| StoreError::Malformed(format!("session {id}: {error}")))?;
    from_doc(stored)?;
    Ok((id, tail))
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

fn validate_ordinary_event_batch(events: &[Event]) -> Result<(), StoreError> {
    if events
        .iter()
        .any(|event| matches!(&event.kind, EventKind::RuntimeStarted { .. }))
    {
        return Err(StoreError::Malformed(
            "RuntimeStarted events may only be written by runtime-authority coordination".into(),
        ));
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
    validate_ordinary_event_batch(events)?;
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

fn skill_pack_state_to_doc(
    state: &SkillPackState,
) -> Result<Option<SkillPackStateDoc>, StoreError> {
    Ok(match state {
        SkillPackState::Legacy => None,
        SkillPackState::Staged(packs) => Some(SkillPackStateDoc::Staged {
            packs: skill_packs_to_docs(packs),
        }),
        SkillPackState::LegacyHandover {
            source_runtime_id,
            source_generation,
            packs,
            stored_digest,
        } => {
            if source_runtime_id.is_empty() {
                return Err(StoreError::Malformed(
                    "a legacy skill handover has no exact source runtime".into(),
                ));
            }
            if stored_digest.is_some_and(|digest| SkillPackDigest::for_packs(packs) != digest) {
                return Err(StoreError::Malformed(
                    "a legacy skill handover receipt does not match its exact bytes".into(),
                ));
            }
            Some(SkillPackStateDoc::LegacyHandover {
                source_runtime_id: source_runtime_id.clone(),
                source_generation: i64::try_from(*source_generation).map_err(|_| {
                    StoreError::Malformed(format!(
                        "legacy skill handover generation {source_generation} cannot be stored"
                    ))
                })?,
                packs: skill_packs_to_docs(packs),
                stored_digest: stored_digest.map(skill_pack_digest_to_binary),
            })
        }
        SkillPackState::StagedSummary => {
            return Err(StoreError::Malformed(
                "a projected staged skill summary cannot be persisted; reload the exact session"
                    .into(),
            ))
        }
        SkillPackState::Stored { digest } => Some(SkillPackStateDoc::Stored {
            digest: skill_pack_digest_to_binary(*digest),
        }),
        SkillPackState::CleanupPending => Some(SkillPackStateDoc::CleanupPending),
        SkillPackState::Cleaned => Some(SkillPackStateDoc::Cleaned),
    })
}

fn skill_pack_state_from_doc(
    state: Option<SkillPackStateDoc>,
) -> Result<SkillPackState, StoreError> {
    match state {
        None => Ok(SkillPackState::Legacy),
        Some(SkillPackStateDoc::Staged { packs }) => {
            Ok(SkillPackState::Staged(skill_packs_from_docs(packs)?))
        }
        Some(SkillPackStateDoc::LegacyHandover {
            source_runtime_id,
            source_generation,
            packs,
            stored_digest,
        }) => {
            if source_runtime_id.is_empty() {
                return Err(StoreError::Malformed(
                    "a legacy skill handover has no exact source runtime".into(),
                ));
            }
            let source_generation = u64::try_from(source_generation).map_err(|_| {
                StoreError::Malformed(
                    "a legacy skill handover generation must not be negative".into(),
                )
            })?;
            let packs = skill_packs_from_docs(packs)?;
            let stored_digest = stored_digest
                .map(|digest| skill_pack_digest_from_binary(digest, "legacy handover"))
                .transpose()?;
            if stored_digest.is_some_and(|digest| SkillPackDigest::for_packs(&packs) != digest) {
                return Err(StoreError::Malformed(
                    "a legacy skill handover receipt does not match its exact bytes".into(),
                ));
            }
            Ok(SkillPackState::LegacyHandover {
                source_runtime_id,
                source_generation,
                packs,
                stored_digest,
            })
        }
        Some(SkillPackStateDoc::StagedSummary) => Err(StoreError::Malformed(
            "an authoritative session contains a listing-only staged skill summary".into(),
        )),
        Some(SkillPackStateDoc::Stored { digest }) => Ok(SkillPackState::Stored {
            digest: skill_pack_digest_from_binary(digest, "stored skill pack")?,
        }),
        Some(SkillPackStateDoc::CleanupPending) => Ok(SkillPackState::CleanupPending),
        Some(SkillPackStateDoc::Cleaned) => Ok(SkillPackState::Cleaned),
    }
}

fn skill_packs_to_docs(packs: &[SkillPack]) -> Vec<SkillPackDoc> {
    packs
        .iter()
        .map(|pack| SkillPackDoc {
            name: pack.name.clone(),
            version: pack.version.clone(),
            files: pack
                .files
                .iter()
                .map(|(path, body)| SkillPackFileDoc {
                    path: path.clone(),
                    body: Binary {
                        subtype: mongodb::bson::spec::BinarySubtype::Generic,
                        bytes: body.clone(),
                    },
                })
                .collect(),
        })
        .collect()
}

fn skill_packs_from_docs(packs: Vec<SkillPackDoc>) -> Result<Vec<SkillPack>, StoreError> {
    let packs = packs
        .into_iter()
        .map(|pack| {
            let files = pack
                .files
                .into_iter()
                .map(|file| {
                    if file.body.subtype != mongodb::bson::spec::BinarySubtype::Generic {
                        return Err(StoreError::Malformed(format!(
                            "skill file {} is not generic binary data",
                            file.path
                        )));
                    }
                    Ok((file.path, file.body.bytes))
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(SkillPack {
                name: pack.name,
                version: pack.version,
                files,
            })
        })
        .collect::<Result<Vec<_>, StoreError>>()?;
    layer2_core::store::validate_skill_packs(&packs).map_err(|error| {
        StoreError::Malformed(format!(
            "invalid inline skill packs in session BSON: {error}"
        ))
    })?;
    Ok(packs)
}

fn skill_pack_digest_to_binary(digest: SkillPackDigest) -> Binary {
    Binary {
        subtype: mongodb::bson::spec::BinarySubtype::Generic,
        bytes: digest.as_bytes().to_vec(),
    }
}

fn skill_pack_digest_from_binary(
    digest: Binary,
    description: &str,
) -> Result<SkillPackDigest, StoreError> {
    if digest.subtype != mongodb::bson::spec::BinarySubtype::Generic {
        return Err(StoreError::Malformed(format!(
            "{description} digest is not generic binary data"
        )));
    }
    SkillPackDigest::from_slice(&digest.bytes)
        .ok_or_else(|| StoreError::Malformed(format!("{description} digest must be 32 bytes")))
}

fn to_doc(r: &SessionRecord) -> Result<SessionDoc, StoreError> {
    let s = &r.session;
    let contains_staged_skills = matches!(
        &r.skill_packs,
        SkillPackState::Staged(_) | SkillPackState::LegacyHandover { .. }
    );
    let document = SessionDoc {
        id: s.id.0.clone(),
        write_protocol: SESSION_WRITE_PROTOCOL,
        coordination_protocol: SESSION_COORDINATION_PROTOCOL,
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
        trigger_origin: r.trigger_origin.clone(),
        chat_origin: r.chat_origin.clone(),
        initial_prompt: r.initial_prompt.clone(),
        rotation_claim: r.rotation_claim.clone(),
        pending_handover: r.pending_handover.clone(),
        skill_packs: skill_pack_state_to_doc(&r.skill_packs)?,
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
        coordination: Some(CoordinationDoc::default()),
    };
    if contains_staged_skills {
        let encoded = mongodb::bson::serialize_to_vec(&document)
            .map_err(|error| StoreError::Malformed(format!("session BSON: {error}")))?;
        if encoded.len() > MAX_STAGED_SESSION_DOCUMENT_BYTES {
            return Err(StoreError::Malformed(format!(
                "session BSON with staged skills exceeds the {MAX_STAGED_SESSION_DOCUMENT_BYTES}-byte limit"
            )));
        }
    }
    Ok(document)
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
        // Filtering a sorted stream preserves its order. Keep the sort next to
        // the base match so Mongo can serve it from the pagination index
        // instead of sorting every row after the tombstone join.
        doc! { "$sort": session_page_sort() },
        tombstone_lookup_stage(),
        doc! { "$match": { "_cma_tombstones.0": { "$exists": false } } },
        doc! { "$project": { "_cma_tombstones": 0 } },
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

/// Match every record whose complete lifecycle state entitles it to a runtime.
///
/// Most states are status-only. An approval wait deliberately shares `idle`
/// with a stopped session, so its stop reason is part of the persisted
/// predicate too.
fn runtime_entitlement_filter() -> Result<Document, StoreError> {
    let statuses = SessionStatus::WANTING_RUNTIME
        .iter()
        .map(serde_plain_string)
        .collect::<Result<Vec<_>, _>>()?;
    let idle = serde_plain_string(&SessionStatus::Idle)?;
    let requires_action = serde_plain_string(&StopReason::RequiresAction)?;

    Ok(doc! {
        "$or": [
            { "status": { "$in": statuses } },
            { "status": idle, "stop_reason": requires_action },
        ]
    })
}

/// A bounded server-side sum of every durable promise that wants a runtime.
fn reserved_capacity_pipeline() -> Result<Vec<Document>, StoreError> {
    let mut entitlement = runtime_entitlement_filter()?;
    entitlement.insert("chat_origin", doc! { "$exists": false });

    Ok(vec![
        doc! { "$match": entitlement },
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
    validate_coordination_state(&d)?;
    let geometry = TerminalGeometry::new(d.columns as u16, d.rows as u16)
        .map_err(|e| StoreError::Malformed(e.to_string()))?;
    let status: layer2_core::SessionStatus = parse_plain(&d.status)?;
    let skill_packs = skill_pack_state_from_doc(d.skill_packs)?;
    let chat = d
        .chat_origin
        .as_ref()
        .map(layer2_core::ChatOrigin::public_projection);
    if status.is_interactive()
        && matches!(
            &skill_packs,
            SkillPackState::LegacyHandover {
                source_runtime_id,
                ..
            } if d.runtime_id.as_deref() != Some(source_runtime_id.as_str())
        )
    {
        return Err(StoreError::Malformed(
            "an interactive legacy skill handover does not belong to its current runtime".into(),
        ));
    }
    Ok(SessionRecord {
        session: Session {
            chat,
            id: SessionId(d.id),
            owner: SubjectId(d.owner),
            kernel: layer2_core::KernelId::new(d.kernel),
            status,
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
        trigger_origin: d.trigger_origin,
        chat_origin: d.chat_origin,
        initial_prompt: d.initial_prompt,
        rotation_claim: d.rotation_claim,
        pending_handover: d.pending_handover,
        skill_packs,
        revision: u64::try_from(d.revision)
            .map_err(|_| StoreError::Malformed("session revision must not be negative".into()))?,
    })
}

/// Decode a bounded listing row whose staged bytes were intentionally removed.
///
/// Every other custody shape still goes through the exact decoder. In
/// particular, stored receipts retain and validate their digest. The summary
/// is restored only after the remaining authoritative document has decoded,
/// so it cannot hide a malformed session field.
fn from_list_doc(mut d: SessionDoc) -> Result<SessionRecord, StoreError> {
    let staged_summary = matches!(d.skill_packs, Some(SkillPackStateDoc::StagedSummary));
    if staged_summary {
        d.skill_packs = None;
    }
    let mut record = from_doc(d)?;
    if staged_summary {
        record.skill_packs = SkillPackState::StagedSummary;
    }
    Ok(record)
}

#[async_trait::async_trait]
impl SessionStore for MongoSessionStore {
    async fn ping(&self) -> Result<(), StoreError> {
        let store = self.clone();
        bounded_readiness_probe(
            Arc::clone(&self.readiness_probe),
            async move { store.check_readiness().await },
            STORE_PING_TIMEOUT,
        )
        .await
    }

    async fn get(&self, id: &SessionId) -> Result<Option<SessionRecord>, StoreError> {
        let mut cursor = self
            .sessions
            .aggregate(exact_session_pipeline(id))
            .await
            .map_err(|error| {
                StoreError::Unavailable(format!("read exact session record: {error}"))
            })?;
        let found = cursor.try_next().await.map_err(|error| {
            StoreError::Unavailable(format!("read exact session record cursor: {error}"))
        })?;
        found
            .map(mongodb::bson::deserialize_from_document)
            .transpose()
            .map_err(|error| StoreError::Malformed(format!("session document: {error}")))?
            .map(from_doc)
            .transpose()
    }

    async fn runtime_authority(
        &self,
        id: &SessionId,
    ) -> Result<Option<RuntimeAuthority>, StoreError> {
        let stored = self
            .sessions
            .clone_with_type::<RuntimeAuthorityProjectionDoc>()
            .find_one(doc! { "_id": &id.0 })
            .projection(runtime_authority_projection())
            .await
            .map_err(|error| {
                StoreError::Unavailable(format!("read MongoDB runtime authority: {error}"))
            })?;
        stored
            .as_ref()
            .map(runtime_authority_from_projection)
            .transpose()
    }

    async fn acquire_session_lease(
        &self,
        request: &SessionLeaseRequest,
    ) -> Result<SessionLease, StoreError> {
        match self
            .run_coordination_command(CoordinationCommand::Acquire(request.clone()))
            .await?
        {
            CoordinationOutcome::Lease(lease) => Ok(lease),
            _ => unreachable!("acquire command returns a lease"),
        }
    }

    async fn acquire_interaction_lease(
        &self,
        request: &SessionInteractionLeaseRequest,
    ) -> Result<SessionLease, StoreError> {
        match self
            .run_coordination_command(CoordinationCommand::AcquireInteraction(request.clone()))
            .await?
        {
            CoordinationOutcome::Lease(lease) => Ok(lease),
            _ => unreachable!("interaction acquisition returns a lease"),
        }
    }

    async fn renew_session_lease(
        &self,
        lease: &SessionLease,
        ttl: Duration,
    ) -> Result<(), StoreError> {
        if lease.operation == layer2_core::SessionOperation::Interact {
            return Err(StoreError::Malformed(
                "interaction leases require interaction renewal".into(),
            ));
        }
        match self
            .run_coordination_command(CoordinationCommand::Renew {
                lease: lease.clone(),
                ttl,
            })
            .await?
        {
            CoordinationOutcome::Unit => Ok(()),
            _ => unreachable!("renew command returns no value"),
        }
    }

    async fn renew_interaction_lease(
        &self,
        lease: &SessionLease,
        ttl: Duration,
    ) -> Result<(), StoreError> {
        if lease.operation != layer2_core::SessionOperation::Interact {
            return Err(StoreError::Malformed(
                "provider-operation leases require provider renewal".into(),
            ));
        }
        match self
            .run_coordination_command(CoordinationCommand::RenewInteraction {
                lease: lease.clone(),
                ttl,
            })
            .await?
        {
            CoordinationOutcome::Unit => Ok(()),
            _ => unreachable!("interaction renew command returns no value"),
        }
    }

    async fn advance_session_lease(
        &self,
        lease: &mut SessionLease,
        boundary: &SessionBoundary,
    ) -> Result<(), StoreError> {
        match self
            .run_coordination_command(CoordinationCommand::Advance {
                lease: lease.clone(),
                boundary: boundary.clone(),
            })
            .await?
        {
            CoordinationOutcome::Unit => {
                lease.boundary = boundary.clone();
                Ok(())
            }
            _ => unreachable!("advance command returns no value"),
        }
    }

    async fn release_session_lease(&self, lease: &SessionLease) -> Result<(), StoreError> {
        match self
            .run_coordination_command(CoordinationCommand::Release(lease.clone()))
            .await?
        {
            CoordinationOutcome::Unit => Ok(()),
            _ => unreachable!("release command returns no value"),
        }
    }

    async fn acquire_conversation_opening(
        &self,
        request: &ConversationOpeningLeaseRequest,
        session_lease: Option<&SessionLease>,
    ) -> Result<ConversationOpeningLease, StoreError> {
        match self
            .run_coordination_command(CoordinationCommand::AcquireConversationOpening {
                request: request.clone(),
                session_lease: session_lease.cloned(),
            })
            .await?
        {
            CoordinationOutcome::ConversationOpening(opening) => Ok(opening),
            _ => unreachable!("conversation opening acquisition returns an opening"),
        }
    }

    async fn renew_conversation_opening(
        &self,
        opening: &ConversationOpeningLease,
        session_lease: Option<&SessionLease>,
        ttl: Duration,
    ) -> Result<(), StoreError> {
        match self
            .run_coordination_command(CoordinationCommand::RenewConversationOpening {
                opening: opening.clone(),
                session_lease: session_lease.cloned(),
                ttl,
            })
            .await?
        {
            CoordinationOutcome::Unit => Ok(()),
            _ => unreachable!("conversation opening renewal returns no value"),
        }
    }

    async fn publish_conversation_link(
        &self,
        opening: &ConversationOpeningLease,
        session_lease: Option<&SessionLease>,
    ) -> Result<ConversationLink, StoreError> {
        match self
            .run_coordination_command(CoordinationCommand::PublishConversationLink {
                opening: opening.clone(),
                session_lease: session_lease.cloned(),
            })
            .await?
        {
            CoordinationOutcome::ConversationLink(link) => Ok(link),
            _ => unreachable!("conversation link publication returns a link"),
        }
    }

    async fn release_conversation_opening(
        &self,
        opening: &ConversationOpeningLease,
    ) -> Result<(), StoreError> {
        match self
            .run_coordination_command(CoordinationCommand::ReleaseConversationOpening(
                opening.clone(),
            ))
            .await?
        {
            CoordinationOutcome::Unit => Ok(()),
            _ => unreachable!("conversation opening release returns no value"),
        }
    }

    async fn validate_conversation_link(&self, link: &ConversationLink) -> Result<(), StoreError> {
        let mut stored = self
            .sessions
            .find_one(doc! { "_id": &link.session_id.0 })
            .await
            .map_err(|error| {
                StoreError::Unavailable(format!(
                    "read MongoDB conversation link authority: {error}"
                ))
            })?
            .ok_or_else(|| StoreError::SessionDeleted {
                id: link.session_id.clone(),
            })?;
        require_conversation_write_authority(
            &mut stored,
            &ConversationWriteAuthority::Active(link.clone()),
        )
    }

    async fn reserve_runtime_generation(
        &self,
        lease: &SessionLease,
        reservation_token: &str,
    ) -> Result<RuntimeReservation, StoreError> {
        match self
            .run_coordination_command(CoordinationCommand::Reserve {
                lease: lease.clone(),
                token: reservation_token.into(),
            })
            .await?
        {
            CoordinationOutcome::Reservation(reservation) => Ok(reservation),
            _ => unreachable!("reserve command returns a reservation"),
        }
    }

    async fn register_runtime_candidate(
        &self,
        lease: &SessionLease,
        reservation: &RuntimeReservation,
        runtime_id: &str,
    ) -> Result<RuntimeCandidate, StoreError> {
        match self
            .run_coordination_command(CoordinationCommand::Register {
                lease: lease.clone(),
                reservation: reservation.clone(),
                runtime_id: runtime_id.into(),
            })
            .await?
        {
            CoordinationOutcome::Candidate(candidate) => Ok(candidate),
            _ => unreachable!("register command returns a candidate"),
        }
    }

    async fn switch_runtime_authority(
        &self,
        lease: &mut SessionLease,
        candidate: &RuntimeCandidate,
    ) -> Result<RuntimeAuthoritySwitch, StoreError> {
        match self
            .run_coordination_command(CoordinationCommand::Switch {
                lease: lease.clone(),
                candidate: candidate.clone(),
            })
            .await?
        {
            CoordinationOutcome::Switch(switched) => {
                lease.boundary = switched.boundary.clone();
                Ok(switched)
            }
            _ => unreachable!("switch command returns an authority switch"),
        }
    }

    async fn retire_runtime_authority(
        &self,
        lease: &mut SessionLease,
        retirement_token: &str,
    ) -> Result<RuntimeAuthorityRetirement, StoreError> {
        match self
            .run_coordination_command(CoordinationCommand::Retire {
                lease: lease.clone(),
                token: retirement_token.into(),
            })
            .await?
        {
            CoordinationOutcome::Retirement(retired) => {
                lease.boundary = retired.boundary.clone();
                Ok(retired)
            }
            _ => unreachable!("retire command returns an authority retirement"),
        }
    }

    async fn runtime_cleanup_debts(
        &self,
        id: &SessionId,
    ) -> Result<Vec<RuntimeCleanup>, StoreError> {
        self.mongo_runtime_cleanup_debts(id).await
    }

    async fn legacy_runtime_cleanup_authorized(&self) -> Result<bool, StoreError> {
        self.verify_coordination_activation(self.collection_ids.as_ref())
            .await?;
        Ok(true)
    }

    async fn acknowledge_runtime_cleanup(
        &self,
        lease: &SessionLease,
        cleanup: &RuntimeCleanup,
    ) -> Result<(), StoreError> {
        match self
            .run_coordination_command(CoordinationCommand::Acknowledge {
                lease: lease.clone(),
                cleanup: cleanup.clone(),
            })
            .await?
        {
            CoordinationOutcome::Unit => Ok(()),
            _ => unreachable!("cleanup acknowledgement returns no value"),
        }
    }

    async fn list_for(
        &self,
        owner: &SubjectId,
        query: &SessionListQuery,
    ) -> Result<Page<SessionRecord>, StoreError> {
        self.query(doc! { "owner": &owner.0 }, query).await
    }

    async fn count_live_for(&self, owner: &SubjectId) -> Result<u32, StoreError> {
        let mut entitlement = runtime_entitlement_filter()?;
        entitlement.insert("owner", owner.0.clone());
        entitlement.insert("chat_origin", doc! { "$exists": false });
        let mut cursor = self
            .sessions
            .aggregate(visible_count_pipeline(entitlement))
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

    async fn list_all(&self, query: &SessionListQuery) -> Result<Page<SessionRecord>, StoreError> {
        self.query(doc! {}, query).await
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
        self.transactional_commit(write_id, record, events, false, None, None)
            .await
    }

    async fn commit_conversation_idempotent(
        &self,
        write_id: &str,
        authority: &ConversationWriteAuthority,
        record: &SessionRecord,
        events: &[Event],
    ) -> Result<u64, StoreError> {
        self.transactional_commit(write_id, record, events, false, Some(authority), None)
            .await
    }

    async fn commit_chat_runtime_transition_idempotent(
        &self,
        write_id: &str,
        transition: &ChatRuntimeTransition,
        record: &SessionRecord,
        events: &[Event],
    ) -> Result<u64, StoreError> {
        self.transactional_commit(write_id, record, events, false, None, Some(transition))
            .await
    }

    async fn create_session_idempotent(
        &self,
        write_id: &str,
        record: &SessionRecord,
        events: &[Event],
    ) -> Result<u64, StoreError> {
        self.transactional_commit(write_id, record, events, true, None, None)
            .await
    }

    async fn event_tail(&self, id: &SessionId) -> Result<u64, StoreError> {
        if self.is_tombstoned(id).await? {
            return Ok(0);
        }
        Ok(self.last_event_sequence(id).await?.unwrap_or(0))
    }

    async fn newest_event(&self, id: &SessionId) -> Result<Option<StoredEvent>, StoreError> {
        if self.is_tombstoned(id).await? {
            return Ok(None);
        }
        self.events
            .find_one(doc! { "session_id": &id.0 })
            .sort(doc! { "seq": -1 })
            .await
            .map_err(|e| StoreError::Unavailable(format!("newest event: {e}")))?
            .map(event_doc::from_document)
            .transpose()
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
        conversation_authority: Option<&ConversationWriteAuthority>,
        chat_runtime_transition: Option<&ChatRuntimeTransition>,
    ) -> Result<u64, StoreError> {
        validate_transaction_input(write_id, record, events, create)?;
        if let Some(transition) = chat_runtime_transition {
            transition.validate_for(record)?;
        }
        if conversation_authority.is_some() && chat_runtime_transition.is_some() {
            return Err(StoreError::Malformed(
                "one session write cannot carry two authority predicates".into(),
            ));
        }

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
            conversation_authority: conversation_authority.cloned(),
            chat_runtime_transition: chat_runtime_transition.cloned(),
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

                    if transaction.conversation_authority.is_some()
                        || transaction.chat_runtime_transition.is_some()
                    {
                        let clocked = transaction
                            .sessions_collection
                            .update_one(
                                doc! { "_id": &transaction.id.0 },
                                coordination_clock_update(),
                            )
                            .session(&mut *session)
                            .await?;
                        if clocked.matched_count != 1 {
                            return Err(mongodb::error::Error::custom(
                                StoreError::SessionDeleted {
                                    id: transaction.id.clone(),
                                },
                            ));
                        }
                    }

                    let mut existing = transaction
                        .sessions_collection
                        .find_one(doc! { "_id": &transaction.id.0 })
                        .session(&mut *session)
                        .await?;
                    if let Some(authority) = transaction.conversation_authority.as_ref() {
                        let stored = existing.as_mut().ok_or_else(|| {
                            mongodb::error::Error::custom(StoreError::SessionDeleted {
                                id: transaction.id.clone(),
                            })
                        })?;
                        require_conversation_write_authority(stored, authority)
                            .map_err(mongodb::error::Error::custom)?;
                    }
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
                        validate_coordination_state(existing)
                            .map_err(mongodb::error::Error::custom)?;
                        if existing.runtime_id != transaction.record.runtime_id
                            && transaction.chat_runtime_transition.is_none()
                        {
                            return Err(mongodb::error::Error::custom(
                                StoreError::SessionAuthorityChanged {
                                    id: transaction.id.clone(),
                                },
                            ));
                        }
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
                    if let Some(existing) = existing.as_ref() {
                        let mut coordination = existing
                            .coordination
                            .clone()
                            .unwrap_or_else(CoordinationDoc::default);
                        if let Some(transition) = transaction.chat_runtime_transition.as_ref() {
                            apply_chat_runtime_transition(existing, transition, &mut coordination)
                                .map_err(mongodb::error::Error::custom)?;
                        }
                        durable.coordination = Some(coordination);
                    }
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
        validate_ordinary_event_batch(events)?;
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
            self.receipts.clone(),
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
                |session, (sessions, events, writes, tombstones, receipts, id)| {
                    async move {
                        sessions
                            .update_one(doc! { "_id": &id.0 }, coordination_clock_update())
                            .session(&mut *session)
                            .await?;
                        let stored = sessions
                            .find_one(doc! { "_id": &id.0 })
                            .session(&mut *session)
                            .await?;
                        let mut receipt_cursor = receipts
                            .clone_with_type::<Document>()
                            .find(all_session_receipts_filter(id))
                            .session(&mut *session)
                            .await?;
                        let mut receipt_documents = Vec::new();
                        while let Some(document) =
                            receipt_cursor.next(&mut *session).await.transpose()?
                        {
                            receipt_documents.push(document);
                        }
                        let cleanup_debts = validate_session_coordination_receipts(
                            stored.as_ref(),
                            receipt_documents,
                        )
                        .map_err(mongodb::error::Error::custom)?;
                        if let Some(stored) = stored.as_ref() {
                            if has_session_lease(stored).map_err(mongodb::error::Error::custom)? {
                                return Err(mongodb::error::Error::custom(
                                    StoreError::SessionLeaseHeld { id: id.clone() },
                                ));
                            }
                            let authority = runtime_authority_from_doc(stored)
                                .map_err(mongodb::error::Error::custom)?;
                            if (authority.runtime_id.is_some() && stored.chat_origin.is_none())
                                || cleanup_debts != 0
                            {
                                return Err(mongodb::error::Error::custom(
                                    StoreError::SessionCleanupPending {
                                        id: id.clone(),
                                        runtime_id: authority.runtime_id,
                                        cleanup_debts,
                                    },
                                ));
                            }
                            let skill_packs = skill_pack_state_from_doc(stored.skill_packs.clone())
                                .map_err(mongodb::error::Error::custom)?;
                            if skill_packs != SkillPackState::Cleaned {
                                return Err(mongodb::error::Error::custom(
                                    StoreError::SessionSkillCleanupPending { id: id.clone() },
                                ));
                            }
                        } else if cleanup_debts != 0 {
                            return Err(mongodb::error::Error::custom(
                                StoreError::SessionCleanupPending {
                                    id: id.clone(),
                                    runtime_id: None,
                                    cleanup_debts,
                                },
                            ));
                        }
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
                        receipts
                            .delete_many(all_session_receipts_filter(id))
                            .session(&mut *session)
                            .await?;
                        Ok(())
                    }
                    .boxed()
                },
            )
            .await
            .map_err(|error| {
                error
                    .get_custom::<StoreError>()
                    .cloned()
                    .unwrap_or_else(|| {
                        StoreError::Unavailable(format!("delete session transaction: {error}"))
                    })
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

    async fn query(
        &self,
        filter: Document,
        query: &SessionListQuery,
    ) -> Result<Page<SessionRecord>, StoreError> {
        let hint = session_page_hint(&filter, query)?;
        let mut cursor = self
            .sessions
            .aggregate(session_page_pipeline(filter, query)?)
            .hint(hint)
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
            out.push(from_list_doc(stored)?);
        }
        let has_more = out.len() > query.page.limit() as usize;
        if has_more {
            out.pop();
        }
        let next = has_more.then(|| {
            let last = out.last().expect("a non-empty bounded page has a last row");
            PageCursor::new(&last.session.created_at, last.session.id.as_str())
        });
        Ok(Page { items: out, next })
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

fn session_page_sort() -> Document {
    doc! { "created_at": -1, "_id": -1 }
}

fn session_page_fetch_limit(query: &SessionListQuery) -> i64 {
    i64::from(query.page.limit().saturating_add(1))
}

fn session_page_hint(filter: &Document, query: &SessionListQuery) -> Result<Hint, StoreError> {
    let owner_scoped = match filter.len() {
        0 => false,
        1 if filter.get_str("owner").is_ok() => true,
        _ => {
            return Err(StoreError::Malformed(
                "session listing has an unsupported base filter for its required index".into(),
            ))
        }
    };
    let index = match (owner_scoped, query.status.is_some()) {
        (true, true) => OWNER_STATUS_SESSION_PAGE_INDEX,
        (true, false) => OWNER_SESSION_PAGE_INDEX,
        (false, true) => STATUS_SESSION_PAGE_INDEX,
        (false, false) => ALL_SESSION_PAGE_INDEX,
    };
    Ok(Hint::Name(index.to_owned()))
}

fn session_page_pipeline(
    filter: Document,
    query: &SessionListQuery,
) -> Result<Vec<Document>, StoreError> {
    let mut pipeline = visible_sessions_pipeline(session_page_filter(filter, query)?);
    // Redact before the tombstone join. A page can ask Mongo for 201 rows and
    // each exact staged row may approach 4 MiB; no later pipeline stage or
    // client cursor should ever retain those bodies for a listing.
    pipeline.insert(2, staged_skill_summary_stage());
    pipeline.push(doc! { "$limit": session_page_fetch_limit(query) });
    Ok(pipeline)
}

fn exact_session_pipeline(id: &SessionId) -> Vec<Document> {
    let mut pipeline = visible_sessions_pipeline(doc! { "_id": &id.0 });
    pipeline.push(doc! { "$limit": 1_i64 });
    pipeline
}

fn runtime_authority_projection() -> Document {
    doc! {
        "_id": 1_i32,
        WRITE_PROTOCOL_FIELD: 1_i32,
        COORDINATION_PROTOCOL_FIELD: 1_i32,
        "runtime_id": 1_i32,
        "coordination": 1_i32,
    }
}

fn staged_skill_summary_stage() -> Document {
    doc! {
        "$set": {
            "skill_packs": {
                "$cond": [
                    { "$in": ["$skill_packs.state", ["staged", "legacy_handover"]] },
                    { "state": "staged_summary" },
                    "$skill_packs",
                ]
            }
        }
    }
}

fn session_page_filter(
    mut filter: Document,
    query: &SessionListQuery,
) -> Result<Document, StoreError> {
    if let Some(status) = query.status {
        filter.insert("status", serde_plain_string(&status)?);
    }
    if let Some(surface) = query.surface {
        let exists = matches!(surface, layer2_core::SessionSurfaceFilter::Chat);
        filter.insert("chat_origin", doc! { "$exists": exists });
    }
    if let Some(cursor) = query.page.cursor() {
        filter.insert(
            "$or",
            vec![
                doc! { "created_at": { "$lt": &cursor.created_at } },
                doc! {
                    "created_at": &cursor.created_at,
                    "_id": { "$lt": &cursor.id },
                },
            ],
        );
    }
    Ok(filter)
}

#[cfg(test)]
#[path = "mongo_tests.rs"]
mod tests;

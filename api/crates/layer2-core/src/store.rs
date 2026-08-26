//! Where sessions and their history are kept.
//!
//! The core defines what durable storage must do and never how. A port per
//! kind of state rather than one store with many methods, because the kinds
//! have genuinely different shapes and lifetimes: session records are small
//! queryable documents, an agent's accumulated context is an opaque blob read
//! and written whole, a reusable bundle outlives every session that used it,
//! and the record of what was asked for is append-only and expires on its own.
//! Split this way, any one of them can be replaced without the others.
//!
//! Nothing here is live state. A screen model, a socket, and an attachment
//! lease exist only while a runtime is running and are rebuilt on attach;
//! persisting them would be persisting a snapshot of something already gone.

use std::{collections::HashSet, time::SystemTime};

use crate::activity::{ActivityQuery, ActivityRecord};
use crate::capabilities::SessionCapabilities;
use crate::coordination::{
    ConversationLink, ConversationOpeningLease, ConversationOpeningLeaseRequest,
    ConversationWriteAuthority, RuntimeAuthority, RuntimeAuthorityRetirement,
    RuntimeAuthoritySwitch, RuntimeCandidate, RuntimeCleanup, RuntimeReservation, SessionBoundary,
    SessionInteractionLeaseRequest, SessionLease, SessionLeaseRequest,
};
use crate::directory::{DirectoryQuery, RegisteredUser};
use crate::environment::{EnvironmentProfile, EnvironmentProfileSpec, ServiceId};
use crate::event::{Event, StoredEvent};
use crate::ids::{EnvironmentProfileId, SessionId, SubjectId};
use crate::options::SessionMode;
use crate::session::{Session, SessionStatus};
use crate::Reserved;

/// Records one internal store read asks for while walking a complete set.
///
/// Full scans are legitimate for reconciliation and repair, but every query
/// they issue remains bounded. Public interfaces choose their own smaller or
/// equal page size before reaching this port.
const INTERNAL_SCAN_PAGE_SIZE: u32 = MAX_PAGE_SIZE;

/// Rows returned when a caller does not choose a page size.
pub const DEFAULT_PAGE_SIZE: u32 = 50;

/// No one store listing may ask persistence to materialize more than this many rows.
pub const MAX_PAGE_SIZE: u32 = 200;

/// A session as it is stored.
#[derive(Debug, Clone)]
pub struct SessionRecord {
    /// The session itself.
    pub session: Session,
    /// Identifier of the runtime currently backing it, when one is running.
    ///
    /// Opaque to the core, and cleared when the runtime goes away. Held so a
    /// restarted process can still find and reclaim what it provisioned.
    pub runtime_id: Option<String>,
    /// Where this session's accumulated agent context is stored, if any.
    pub context_ref: Option<String>,
    /// Unique owner of an in-progress runtime handover.
    ///
    /// Kept on the durable record rather than the public session because it is
    /// a compare token, not user-visible lifecycle state. History may advance a
    /// record's revision while a runtime is being captured; this claim lets the
    /// handover rebase over that history without mistaking a later rotation of
    /// the same runtime for its own work. It remains present after the public
    /// status advances from `Rotating` until the replacement runtime has been
    /// published locally, so every control-plane replica observes the same
    /// non-interactive boundary during recovery and readoption.
    #[doc(hidden)]
    pub rotation_claim: Option<String>,
    /// Durable recovery instructions for a compatibility handover.
    ///
    /// Compatibility stores cannot keep H1 authoritative while provisioning
    /// H2. This descriptor therefore names the separately stored snapshot and
    /// the exact target that must be published after H1 is cleared. Its claim
    /// must equal [`Self::rotation_claim`] while the handover is active; an
    /// explicit owner stop may clear the active claim and retain the descriptor
    /// on an `Idle` session for verified revive. Every other mismatch is fenced
    /// rather than interpreted. It is cleared atomically with the claim only
    /// after H2 is published.
    #[doc(hidden)]
    pub pending_handover: Option<PendingHandover>,
    /// Store-owned optimistic revision.
    ///
    /// Zero means the record has not been created yet. Stores increment this
    /// after every atomic record/event commit. It is deliberately absent from
    /// the public `Session`: callers do not choose or observe persistence
    /// concurrency tokens.
    #[doc(hidden)]
    pub revision: u64,
}

impl SessionRecord {
    /// A record for a freshly created session.
    pub fn new(session: Session) -> Self {
        Self {
            session,
            runtime_id: None,
            context_ref: None,
            rotation_claim: None,
            pending_handover: None,
            revision: 0,
        }
    }

    /// Whether this durable record is still inside a runtime handover.
    ///
    /// The public status describes the current provisioning stage. The claim
    /// spans the later `Provisioning` and `Ready` publication gaps that status
    /// alone cannot identify across control-plane replicas.
    pub(crate) fn is_effectively_rotating(&self) -> bool {
        self.session.status.is_rotating() || self.rotation_claim.is_some()
    }
}

/// Durable instructions for completing one compatibility runtime handover.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PendingHandover {
    /// Exact rotation owner. Must equal the record's rotation claim.
    pub claim: String,
    /// Stable object-store reference returned for this session's handover blob.
    pub snapshot_ref: String,
    /// Conservative time at which the first usable snapshot was captured.
    pub snapshot_at: String,
    /// Rotation count at the exact source boundary.
    pub rotations_from: u32,
    /// Rotation count to publish with the target semantics.
    pub rotations_to: u32,
    /// Session semantics to publish after H1 is durably gone.
    pub target: PendingHandoverTarget,
}

/// The semantic change a pending compatibility handover must publish.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PendingHandoverTarget {
    /// Move onto this exact recorded runtime version.
    RuntimeVersion {
        from_runtime_version: Option<String>,
        runtime_version: String,
    },
    /// Move onto this plane with the capabilities derived before teardown.
    Plane {
        from_mode: SessionMode,
        mode: SessionMode,
        capabilities: SessionCapabilities,
    },
}

/// Claim-bound opaque bytes stored outside the session document.
///
/// `None` is distinct from an empty snapshot on the storage wire. Rotation
/// writes `Some`, while retaining the distinction makes malformed or future
/// envelope versions fail closed instead of silently becoming blank restores.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandoverSnapshot {
    /// Claim whose descriptor is allowed to consume these bytes.
    pub claim: String,
    /// Capture time for these exact bytes.
    pub snapshot_at: String,
    /// Opaque adapter state, when the envelope contains a captured snapshot.
    pub restore: Option<Vec<u8>>,
}

/// The stable position after which a newest-first listing continues.
///
/// Creation timestamps are not unique, so the durable resource identifier is
/// the tie-breaker. Stores sort both descending and return rows strictly older
/// than this pair.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PageCursor {
    /// Creation time of the last row already returned.
    pub created_at: String,
    /// Durable identifier of that row.
    pub id: String,
}

impl PageCursor {
    /// Name one stable position in a listing.
    pub fn new(created_at: impl Into<String>, id: impl Into<String>) -> Self {
        Self {
            created_at: created_at.into(),
            id: id.into(),
        }
    }
}

/// One bounded request against a newest-first store listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageRequest {
    /// Most rows the caller may receive.
    limit: u32,
    /// Last row already received, when this continues an earlier page.
    cursor: Option<PageCursor>,
}

impl PageRequest {
    /// Build a request, keeping every store query within the supported bounds.
    pub fn new(limit: u32, cursor: Option<PageCursor>) -> Self {
        Self {
            limit: limit.clamp(1, MAX_PAGE_SIZE),
            cursor,
        }
    }

    /// The clamped number of rows the store may return.
    pub fn limit(&self) -> u32 {
        self.limit
    }

    /// The last row already returned, when this continues another page.
    pub fn cursor(&self) -> Option<&PageCursor> {
        self.cursor.as_ref()
    }
}

impl Default for PageRequest {
    fn default() -> Self {
        Self::new(DEFAULT_PAGE_SIZE, None)
    }
}

/// Filters attached to a bounded session listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionListQuery {
    /// Bounds and continuation position.
    pub page: PageRequest,
    /// Exact lifecycle state to retain, or every state when absent.
    pub status: Option<SessionStatus>,
}

impl SessionListQuery {
    /// Build one bounded session listing request.
    pub fn new(page: PageRequest, status: Option<SessionStatus>) -> Self {
        Self { page, status }
    }
}

impl Default for SessionListQuery {
    fn default() -> Self {
        Self::new(PageRequest::default(), None)
    }
}

/// One bounded store answer and the position of the following page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page<T> {
    /// Rows in stable newest-first order.
    pub items: Vec<T>,
    /// Last returned row when at least one further row exists.
    pub next: Option<PageCursor>,
}

impl<T> Page<T> {
    /// Transform rows without changing the continuation position.
    pub fn map<U>(self, mut render: impl FnMut(T) -> U) -> Page<U> {
        Page {
            items: self.items.into_iter().map(&mut render).collect(),
            next: self.next,
        }
    }
}

/// Failures a store may report.
#[derive(Debug, Clone, thiserror::Error)]
pub enum StoreError {
    /// The store could not be reached.
    #[error("store unavailable: {0}")]
    Unavailable(String),
    /// The stored data did not match what the core expects.
    #[error("stored data not usable: {0}")]
    Malformed(String),
    /// A batch begins at or below the session's durable tail.
    ///
    /// Named rather than folded into `Unavailable` because the two call for
    /// opposite responses: a store that is down is retried unchanged, and a
    /// sequence already taken can only ever be retried under a different one.
    /// A caller that cannot tell them apart does neither, which is how a turn's
    /// events came to be dropped with nothing but a log line to say so.
    ///
    /// Event appends are atomic, so this error never means a prefix was
    /// accepted. `expected` is the next durable position and `got` is the
    /// already-held position at the start of the refused batch.
    #[error("sequence already taken: expected {expected}, got {got}")]
    SequenceTaken {
        /// The next sequence the durable log can accept.
        expected: u64,
        /// The first sequence claimed by the refused batch.
        got: u64,
    },
    /// A batch would leave an unreadable hole in a session's event sequence.
    ///
    /// Separate from a sequence already taken: nothing in this batch was
    /// accepted, and the missing lower sequence may still be in flight from
    /// another control-plane process. The caller may retry the batch unchanged;
    /// moving it higher would preserve the hole this error exists to expose.
    #[error("non-contiguous append: expected sequence {expected}, got {got}")]
    NonContiguousAppend {
        /// The next sequence the durable log can accept.
        expected: u64,
        /// The first sequence that would skip it.
        got: u64,
    },
    /// An update named a session which no longer exists durably.
    #[error("session {id} no longer exists")]
    SessionDeleted {
        /// The deleted session.
        id: SessionId,
    },
    /// An ordinary update attempted to return a terminated session to service.
    #[error("session {id} is terminated")]
    SessionTerminated {
        /// The terminated session.
        id: SessionId,
    },
    /// Another durable writer changed the record after this writer read it.
    #[error("session revision changed: expected {expected}, found {actual}")]
    RevisionConflict {
        /// Revision carried by the refused writer.
        expected: u64,
        /// Revision currently durable.
        actual: u64,
    },
    /// This store is running in a compatibility stage without coordination.
    #[error("durable session coordination is not enabled")]
    CoordinationDisabled,
    /// Another unexpired operation currently owns this session.
    #[error("session {id} is held by another operation")]
    SessionLeaseHeld {
        /// Session whose exclusive lease is occupied.
        id: SessionId,
    },
    /// A runtime interaction met another active operation.
    #[error("session {id} interaction is blocked by {operation:?}")]
    SessionInteractionBlocked {
        /// Session whose runtime is currently fenced.
        id: SessionId,
        /// Operation holding the durable fence.
        operation: crate::coordination::SessionOperation,
    },
    /// The session no longer has the authority or status the caller observed.
    #[error("session {id} authority changed")]
    SessionAuthorityChanged {
        /// Session whose boundary no longer matches.
        id: SessionId,
    },
    /// This lease expired or was replaced by a later holder.
    #[error("session {id} lease is no longer authoritative")]
    SessionLeaseLost {
        /// Session whose lease no longer matches.
        id: SessionId,
    },
    /// Another unexpired conversation opening currently owns this session.
    #[error("session {id} conversation opening is held by another replica")]
    ConversationOpeningHeld {
        /// Session whose opening claim is occupied.
        id: SessionId,
    },
    /// This conversation opening expired, was released, or was replaced.
    #[error("session {id} conversation opening is no longer authoritative")]
    ConversationOpeningLost {
        /// Session whose opening claim no longer matches.
        id: SessionId,
    },
    /// A conversation write came from a link superseded by a later publication.
    #[error("session {id} conversation link is no longer authoritative")]
    ConversationLinkChanged {
        /// Session whose published link no longer matches.
        id: SessionId,
    },
    /// This runtime reservation expired, was replaced, or belongs to another lease.
    #[error("session {id} runtime reservation is no longer authoritative")]
    RuntimeReservationChanged {
        /// Session whose reservation no longer matches.
        id: SessionId,
    },
    /// A reservation was already registered against a different provider runtime.
    #[error("session {id} runtime candidate changed")]
    RuntimeCandidateChanged {
        /// Session whose candidate no longer matches.
        id: SessionId,
    },
    /// A cleanup debt was replaced, completed, or names current authority.
    #[error("session {id} runtime cleanup changed")]
    RuntimeCleanupChanged {
        /// Session whose cleanup no longer matches.
        id: SessionId,
    },
    /// Removing the session would erase authority needed to reclaim a runtime.
    #[error(
        "session {id} still has runtime authority {runtime_id:?} or {cleanup_debts} cleanup debts"
    )]
    SessionCleanupPending {
        /// Session that cannot yet be removed safely.
        id: SessionId,
        /// Runtime still authoritative, when one remains.
        runtime_id: Option<String>,
        /// Number of unacknowledged cleanup debts.
        cleanup_debts: usize,
    },
}

/// One subject-scoped attempt to perform a named operation once.
///
/// The name, key, and fingerprint are opaque to the core. The interface that
/// accepts the request decides how to derive them; persistence only guarantees
/// that one exact fingerprint can hold a key until the claim expires.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationClaim {
    subject: SubjectId,
    operation: String,
    key: String,
    fingerprint: [u8; 32],
    expires_at: SystemTime,
}

impl OperationClaim {
    /// Build a durable operation claim.
    pub fn new(
        subject: SubjectId,
        operation: impl Into<String>,
        key: impl Into<String>,
        fingerprint: [u8; 32],
        expires_at: SystemTime,
    ) -> Self {
        Self {
            subject,
            operation: operation.into(),
            key: key.into(),
            fingerprint,
            expires_at,
        }
    }

    /// The subject whose operation this is.
    pub fn subject(&self) -> &SubjectId {
        &self.subject
    }

    /// The interface-defined operation name.
    pub fn operation(&self) -> &str {
        &self.operation
    }

    /// The caller-supplied operation key.
    pub fn key(&self) -> &str {
        &self.key
    }

    /// A stable digest of everything that distinguishes this request.
    pub fn fingerprint(&self) -> &[u8; 32] {
        &self.fingerprint
    }

    /// When persistence may forget this claim and permit the key again.
    pub fn expires_at(&self) -> SystemTime {
        self.expires_at
    }
}

/// What an atomic attempt to reserve an operation key found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperationReservation {
    /// This caller acquired a new claim and may perform the operation.
    Acquired,
    /// The same operation is still being performed elsewhere.
    Pending,
    /// The operation finished and its opaque outcome can be replayed.
    Complete(Vec<u8>),
    /// This key already names a different request fingerprint.
    FingerprintMismatch,
}

/// Durable, subject-scoped operation claims and their opaque outcomes.
///
/// The stored bytes belong to the interface that produced them. Keeping them
/// opaque here lets any interface use this guarantee without placing its
/// response vocabulary in the core.
#[async_trait::async_trait]
pub trait OperationStore: Send + Sync {
    /// Atomically acquire a new claim or inspect the one already holding it.
    async fn reserve(&self, claim: &OperationClaim) -> Result<OperationReservation, StoreError>;

    /// Replace a held claim's pending marker with its replayable outcome.
    async fn complete(&self, claim: &OperationClaim, outcome: Vec<u8>) -> Result<(), StoreError>;
}

#[cfg(test)]
mod operation_claim_tests {
    use std::time::{Duration, SystemTime};

    use super::*;

    #[test]
    fn a_claim_preserves_every_part_of_its_durable_identity() {
        let expires_at = SystemTime::UNIX_EPOCH + Duration::from_secs(2_000_000_000);
        let claim = OperationClaim::new(
            SubjectId("subject-a".into()),
            "create-resource",
            "attempt-a",
            [9; 32],
            expires_at,
        );

        assert_eq!(claim.subject(), &SubjectId("subject-a".into()));
        assert_eq!(claim.operation(), "create-resource");
        assert_eq!(claim.key(), "attempt-a");
        assert_eq!(claim.fingerprint(), &[9; 32]);
        assert_eq!(claim.expires_at(), expires_at);
    }
}

/// Durable session records and their event history.
#[async_trait::async_trait]
pub trait SessionStore: Send + Sync {
    /// Verify that the store can answer a readiness probe.
    ///
    /// This exists only for the process readiness endpoint. Business paths use
    /// their purpose-specific operations so a health check cannot become an
    /// accidental substitute for handling a real store failure.
    async fn ping(&self) -> Result<(), StoreError>;

    /// Read one session, whoever owns it. Ownership is decided above this.
    async fn get(&self, id: &SessionId) -> Result<Option<SessionRecord>, StoreError>;

    /// Read the separately stored runtime generation and handle.
    ///
    /// A compatibility-stage store returns [`StoreError::CoordinationDisabled`].
    async fn runtime_authority(
        &self,
        _id: &SessionId,
    ) -> Result<Option<RuntimeAuthority>, StoreError> {
        Err(StoreError::CoordinationDisabled)
    }

    /// Acquire the one durable operation lease for a session.
    ///
    /// Retrying a token while its exact lease remains active is idempotent.
    /// Once expired or released, that token is permanently spent and cannot
    /// acquire authority again even when the session boundary is unchanged.
    async fn acquire_session_lease(
        &self,
        _request: &SessionLeaseRequest,
    ) -> Result<SessionLease, StoreError> {
        Err(StoreError::CoordinationDisabled)
    }

    /// Acquire a bounded lease for one runtime interaction.
    ///
    /// Unlike provider-operation acquisition, this does not retain a permanent
    /// token receipt. The store issues each lease token from monotonic
    /// per-session state, so expiry cannot make an old lease token authoritative
    /// again while frequent terminal traffic keeps durable state bounded.
    ///
    /// Calls carrying one exact request nonce may overlap while an unavailable
    /// outcome is recovered. Once one such call returns `Ok(lease)`, it must be
    /// ordered after every earlier overlapping call with that nonce: no earlier
    /// call may subsequently create or extend authority after exact release of
    /// `lease`. This lets bounded request state recover cancellation without a
    /// permanent receipt for every interaction.
    async fn acquire_interaction_lease(
        &self,
        _request: &SessionInteractionLeaseRequest,
    ) -> Result<SessionLease, StoreError> {
        Err(StoreError::CoordinationDisabled)
    }

    /// Extend a provider-operation lease using the store's authoritative clock.
    ///
    /// Interaction leases must use [`Self::renew_interaction_lease`], whose
    /// authority check deliberately permits action-owned status changes.
    async fn renew_session_lease(
        &self,
        _lease: &SessionLease,
        _ttl: std::time::Duration,
    ) -> Result<(), StoreError> {
        Err(StoreError::CoordinationDisabled)
    }

    /// Extend an interaction lease while its exact runtime authority is current.
    ///
    /// The active token, holder, and operation must still match, as must the
    /// runtime id and generation. Current lifecycle status is intentionally not
    /// part of renewal: the interaction's retained work may own that transition.
    async fn renew_interaction_lease(
        &self,
        _lease: &SessionLease,
        _ttl: std::time::Duration,
    ) -> Result<(), StoreError> {
        Err(StoreError::CoordinationDisabled)
    }

    /// Move a held lease onto a status boundary this holder just committed.
    async fn advance_session_lease(
        &self,
        _lease: &mut SessionLease,
        _boundary: &SessionBoundary,
    ) -> Result<(), StoreError> {
        Err(StoreError::CoordinationDisabled)
    }

    /// Release only this exact lease; a stale holder cannot release a successor.
    async fn release_session_lease(&self, _lease: &SessionLease) -> Result<(), StoreError> {
        Err(StoreError::CoordinationDisabled)
    }

    /// Reserve one pending conversation opening without replacing the active link.
    ///
    /// `session_lease` is required by coordinated callers and must be the real
    /// current provider-operation lease. Compatibility callers pass none; the
    /// store then conditions the claim on the exact runtime handle in `request`.
    async fn acquire_conversation_opening(
        &self,
        _request: &ConversationOpeningLeaseRequest,
        _session_lease: Option<&SessionLease>,
    ) -> Result<ConversationOpeningLease, StoreError> {
        Err(StoreError::CoordinationDisabled)
    }

    /// Extend an exact pending opening using the store's authoritative clock.
    async fn renew_conversation_opening(
        &self,
        _opening: &ConversationOpeningLease,
        _session_lease: Option<&SessionLease>,
        _ttl: std::time::Duration,
    ) -> Result<(), StoreError> {
        Err(StoreError::CoordinationDisabled)
    }

    /// Promote a pending opening into the sole active conversation link.
    ///
    /// Publication is the cross-replica linearization point. It advances a
    /// monotonic epoch and atomically makes every older link unable to commit.
    async fn publish_conversation_link(
        &self,
        _opening: &ConversationOpeningLease,
        _session_lease: Option<&SessionLease>,
    ) -> Result<ConversationLink, StoreError> {
        Err(StoreError::CoordinationDisabled)
    }

    /// Release this exact opening, including a publication whose response was
    /// never observed; stale release cannot retire a successor link.
    async fn release_conversation_opening(
        &self,
        _opening: &ConversationOpeningLease,
    ) -> Result<(), StoreError> {
        Err(StoreError::CoordinationDisabled)
    }

    /// Check one published link without changing the session or its history.
    ///
    /// Conversation frames use this as their durable admission point before
    /// changing replica-local state or broadcasting liveness. Publication and
    /// validation must therefore observe one total order for a session.
    async fn validate_conversation_link(&self, _link: &ConversationLink) -> Result<(), StoreError> {
        Err(StoreError::CoordinationDisabled)
    }

    /// Reserve a never-reused runtime generation while holding a lease.
    async fn reserve_runtime_generation(
        &self,
        _lease: &SessionLease,
        _reservation_token: &str,
    ) -> Result<RuntimeReservation, StoreError> {
        Err(StoreError::CoordinationDisabled)
    }

    /// Register the provider handle returned for one exact reservation.
    async fn register_runtime_candidate(
        &self,
        _lease: &SessionLease,
        _reservation: &RuntimeReservation,
        _runtime_id: &str,
    ) -> Result<RuntimeCandidate, StoreError> {
        Err(StoreError::CoordinationDisabled)
    }

    /// Atomically switch both the session record handle and durable authority.
    ///
    /// This changes no lifecycle or event fields, so event folds that committed
    /// while provisioning was in flight remain intact. The candidate generation
    /// must strictly exceed current authority. An unknown-result retry returns
    /// its receipt only while that switch is still the durable lease boundary;
    /// a later switch can never be mistaken for the earlier result.
    async fn switch_runtime_authority(
        &self,
        _lease: &mut SessionLease,
        _candidate: &RuntimeCandidate,
    ) -> Result<RuntimeAuthoritySwitch, StoreError> {
        Err(StoreError::CoordinationDisabled)
    }

    /// Atomically clear current authority and retain its exact cleanup debt.
    ///
    /// `retirement_token` is generated once per logical operation. Retrying it
    /// after an unknown store result returns the same receipt only while this
    /// lease still owns the resulting boundary.
    async fn retire_runtime_authority(
        &self,
        _lease: &mut SessionLease,
        _retirement_token: &str,
    ) -> Result<RuntimeAuthorityRetirement, StoreError> {
        Err(StoreError::CoordinationDisabled)
    }

    /// Cleanup debts currently durable for one session.
    async fn runtime_cleanup_debts(
        &self,
        _id: &SessionId,
    ) -> Result<Vec<RuntimeCleanup>, StoreError> {
        Err(StoreError::CoordinationDisabled)
    }

    /// Whether absent generation metadata is an exact, stable legacy identity.
    ///
    /// This may become true only as a monotonic store-protocol promotion after
    /// every legacy writer has been fenced by the store's exact old-writer-drain
    /// acknowledgement; merely observing no legacy writers at one instant is
    /// not sufficient. The default retains generation-zero cleanup debt for an
    /// operator or a later capable binary.
    async fn legacy_runtime_cleanup_authorized(&self) -> Result<bool, StoreError> {
        Ok(false)
    }

    /// Acknowledge exact cleanup after a conditional provider teardown succeeds.
    async fn acknowledge_runtime_cleanup(
        &self,
        _lease: &SessionLease,
        _cleanup: &RuntimeCleanup,
    ) -> Result<(), StoreError> {
        Err(StoreError::CoordinationDisabled)
    }

    /// One bounded page of a subject's sessions, newest first.
    async fn list_for(
        &self,
        owner: &SubjectId,
        query: &SessionListQuery,
    ) -> Result<Page<SessionRecord>, StoreError>;

    /// Walk all of one subject's records through bounded store reads.
    ///
    /// Used by internal accounting fallbacks, never by a public list handler.
    async fn scan_for(&self, owner: &SubjectId) -> Result<Vec<SessionRecord>, StoreError> {
        let mut records = Vec::new();
        let mut cursor = None;
        let mut seen = HashSet::new();
        loop {
            let page = self
                .list_for(
                    owner,
                    &SessionListQuery::new(
                        PageRequest::new(INTERNAL_SCAN_PAGE_SIZE, cursor.clone()),
                        None,
                    ),
                )
                .await?;
            records.extend(page.items);
            match page.next {
                Some(next) if seen.insert(next.clone()) => cursor = Some(next),
                Some(_) => {
                    return Err(StoreError::Malformed(
                        "session listing repeated its continuation cursor".into(),
                    ))
                }
                None => return Ok(records),
            }
        }
    }

    /// How many of this subject's sessions are recorded as wanting a runtime.
    ///
    /// Its own method rather than a fold over `list_for`, which decodes every
    /// record the subject has ever had on a query made for every creation. The
    /// default is that fold so a store with nothing better remains correct.
    ///
    /// Recorded state is authoritative. Provider inventory can retain a dead
    /// runtime and would then refuse a session its owner is entitled to start.
    async fn count_live_for(&self, owner: &SubjectId) -> Result<u32, StoreError> {
        Ok(self
            .scan_for(owner)
            .await?
            .into_iter()
            .filter(|record| record.session.wants_runtime())
            .count() as u32)
    }

    /// What this deployment has promised to sandboxes that should be running.
    ///
    /// A port method rather than a fold over `list_all`, which decodes every
    /// record ever written and is documented for the monitoring surface. This
    /// runs on every session creation.
    async fn reserved_capacity(&self) -> Result<Reserved, StoreError>;

    /// One bounded page of every session, newest first.
    async fn list_all(&self, query: &SessionListQuery) -> Result<Page<SessionRecord>, StoreError>;

    /// Walk every record through bounded store reads.
    ///
    /// Reconciliation and provider inventory need the complete authority. They
    /// use this explicit scan so no public listing can acquire the same cost by
    /// forgetting to pass a bound.
    async fn scan_all(&self) -> Result<Vec<SessionRecord>, StoreError> {
        let mut records = Vec::new();
        let mut cursor = None;
        let mut seen = HashSet::new();
        loop {
            let page = self
                .list_all(&SessionListQuery::new(
                    PageRequest::new(INTERNAL_SCAN_PAGE_SIZE, cursor.clone()),
                    None,
                ))
                .await?;
            records.extend(page.items);
            match page.next {
                Some(next) if seen.insert(next.clone()) => cursor = Some(next),
                Some(_) => {
                    return Err(StoreError::Malformed(
                        "session listing repeated its continuation cursor".into(),
                    ))
                }
                None => return Ok(records),
            }
        }
    }

    /// Remove a session and its history permanently.
    ///
    /// Distinct from ending one: a terminated session stays visible until its
    /// owner chooses to remove it. A coordination-enabled store keeps cleanup
    /// authority with the live session row, so it must refuse with
    /// [`StoreError::SessionCleanupPending`] while a runtime remains authoritative
    /// or any exact cleanup debt is unacknowledged.
    async fn delete(&self, id: &SessionId) -> Result<(), StoreError>;

    /// Append durable events.
    ///
    /// Sequence numbers are allocated by the caller and are monotonic per
    /// session, so a store must preserve them rather than assign its own. A
    /// batch may become readable only when its first sequence follows the
    /// session's durable tail and every sequence inside it is contiguous.
    /// The append is atomic: a refusal commits no prefix of the batch.
    ///
    /// **A sequence a session already has must be refused**, and refused as
    /// `SequenceTaken` rather than as a general failure. The core allocates,
    /// but more than one process may be allocating for the same session, so
    /// the store is the only place the clash is visible. A store that accepts
    /// a duplicate leaves two events answering to one cursor, and the client
    /// reading history sees one of them.
    ///
    /// **A batch that skips the next durable sequence must be refused** as
    /// `NonContiguousAppend`. A reader advances its cursor to every event it
    /// sees, so accepting a higher sequence while a lower batch is in flight
    /// makes that lower event permanently invisible to that reader.
    async fn append_events(&self, events: &[Event]) -> Result<(), StoreError>;

    /// Atomically replace a session record and append its state-change events.
    ///
    /// The record and batch must either both become durable or neither may.
    /// In particular, `record.session.last_seq` must never become visible ahead
    /// of the events that establish that position. The same collision and
    /// contiguity contract as `append_events` applies.
    ///
    /// `write_id` identifies the whole atomic unit, including an empty event
    /// batch. Retrying a committed ID must return its resulting revision
    /// without applying the record again. That distinction matters when a
    /// later record-only writer has already committed.
    ///
    /// A store upgrading data written by the former two-step contract must
    /// treat the greater of the physical event tail and the record's advertised
    /// `last_seq` as the durable sequence floor. Already-advertised missing
    /// positions cannot safely be reused, because a client may already hold
    /// that cursor. A record-only update may preserve a higher physical tail,
    /// but may not introduce a higher tail of its own.
    ///
    /// Termination is irreversible. An update may keep a terminated record
    /// terminated while clearing durable references, but must refuse any
    /// transition from a durable terminal status back to a live status as
    /// [`StoreError::SessionTerminated`].
    async fn commit_session_idempotent(
        &self,
        write_id: &str,
        record: &SessionRecord,
        events: &[Event],
    ) -> Result<u64, StoreError>;

    /// Atomically commit a session write only while its conversation authority
    /// is still current. A committed `write_id` remains replayable after later
    /// publication; otherwise an old opening or active link is refused before
    /// any record or event mutation becomes durable.
    async fn commit_conversation_idempotent(
        &self,
        _write_id: &str,
        _authority: &ConversationWriteAuthority,
        _record: &SessionRecord,
        _events: &[Event],
    ) -> Result<u64, StoreError> {
        Err(StoreError::CoordinationDisabled)
    }

    /// Atomically create a session and its initial events.
    ///
    /// Separate from update so a stale writer can never recreate a session
    /// another replica deleted. A create requires revision zero; an update
    /// requires the named durable revision and must never upsert.
    async fn create_session_idempotent(
        &self,
        write_id: &str,
        record: &SessionRecord,
        events: &[Event],
    ) -> Result<u64, StoreError>;

    /// The highest sequence currently durable for a session, or zero.
    ///
    /// Implementations may override this with an indexed tail query. The
    /// default is intentionally based on events, never on `Session.last_seq`:
    /// a whole-record write may legitimately lag the event log.
    async fn event_tail(&self, id: &SessionId) -> Result<u64, StoreError> {
        Ok(self
            .events_since(id, 0)
            .await?
            .into_iter()
            .map(|event| event.seq)
            .max()
            .unwrap_or(0))
    }

    /// Events after a cursor, in sequence order.
    async fn events_since(
        &self,
        id: &SessionId,
        after: u64,
    ) -> Result<Vec<StoredEvent>, StoreError>;
}

/// Durable environment profiles.
///
/// A third port rather than a second collection behind the session store: a
/// profile outlives every session that used it and is queried by owner, so
/// conflating the two would mean one implementation could not be replaced
/// without the other.
///
/// Nothing here filters by owner except `list_for`. Ownership is decided above
/// this, exactly as it is for sessions, so that no store implementation can
/// hold a different opinion about who may read what.
#[async_trait::async_trait]
pub trait EnvironmentProfileStore: Send + Sync {
    /// Create a profile with a new identifier.
    async fn upsert(&self, profile: &EnvironmentProfile) -> Result<(), StoreError>;

    /// Atomically replace caller-controlled content on an existing profile.
    ///
    /// Implementations must update only the fields carried by `content` plus
    /// `updated_at`. The current `owner`, `created_at`, and `preferred` values
    /// are server-owned and must survive even when they changed after the
    /// service read the row. This operation must never upsert: `Ok(None)` means
    /// the identifier no longer exists, while `Ok(Some(_))` returns the complete
    /// post-update row including its current server-owned fields.
    async fn replace_content(
        &self,
        id: &EnvironmentProfileId,
        content: &EnvironmentProfileSpec,
        updated_at: &str,
    ) -> Result<Option<EnvironmentProfile>, StoreError>;

    /// Read one profile, whoever owns it.
    async fn get(
        &self,
        id: &EnvironmentProfileId,
    ) -> Result<Option<EnvironmentProfile>, StoreError>;

    /// One bounded page of a subject's profiles, newest first.
    async fn list_for(
        &self,
        owner: &SubjectId,
        page: &PageRequest,
    ) -> Result<Page<EnvironmentProfile>, StoreError>;

    /// Replace every stored preference claim for an owner with one chosen profile, or none.
    ///
    /// The chosen identifier must still name one of the owner's profiles when
    /// this operation begins. Implementations must validate that before
    /// clearing an existing claim. This is a mutation rather than a listing:
    /// it must not collect or return the owner's profiles while doing the
    /// replacement.
    async fn set_preferred(
        &self,
        owner: &SubjectId,
        chosen: Option<&EnvironmentProfileId>,
    ) -> Result<(), StoreError>;

    /// The winning preferred profile, where one or more records claim it.
    ///
    /// This lookup must retain at most one profile. It is deliberately a
    /// required store operation: implementing it by collecting a complete
    /// profile listing would make both listing one page and changing the
    /// preference grow with the owner's history.
    async fn preferred_for(
        &self,
        owner: &SubjectId,
    ) -> Result<Option<EnvironmentProfile>, StoreError>;

    /// Remove a profile permanently.
    ///
    /// Sessions already started with it are unaffected: they were handed a
    /// resolved bundle at creation and never read the profile again.
    async fn delete(&self, id: &EnvironmentProfileId) -> Result<(), StoreError>;
}

/// The durable record of what the service was asked to do.
///
/// Append-only by construction: there is no method that edits or removes an
/// entry, because an account of what happened that can be rewritten is not an
/// account of anything.
///
/// **Entries do not accumulate forever.** An implementation is required to
/// expire them, and how long it keeps them is its own decision to state — an
/// unbounded collection of one document per request is an outage waiting for a
/// busy week. Nothing in the core reads an expired entry back, and nothing
/// depends on one being there.
#[async_trait::async_trait]
pub trait ActivityStore: Send + Sync {
    /// Keep one entry.
    async fn append(&self, record: &ActivityRecord) -> Result<(), StoreError>;

    /// The most recent entries matching a query, newest first.
    ///
    /// Newest first because that is the only order this is ever read in: the
    /// question is always "what has been happening", never "what happened at
    /// the beginning".
    async fn recent(&self, query: &ActivityQuery) -> Result<Vec<ActivityRecord>, StoreError>;
}

/// The subjects that have used this service.
///
/// The core models no user directory of its own — identity belongs to whoever
/// issues it — so this is a record of who has turned up, not an authority on
/// who exists. Nothing is authorised against it and no decision is made on the
/// strength of a name in it.
///
/// **No read of this is unbounded.** It holds one document per person rather
/// than one per request, so it grows more slowly than the record of actions —
/// but it still only grows, and a listing with no ceiling is a listing that
/// works until the day it does not. Both reads below are bounded by
/// construction: one by an explicit limit, the other by the subjects actually
/// being asked about.
///
/// Deliberately *not* expired, unlike the record of actions. An entry is a
/// person, not an event, and dropping one would both lose the earliest arrival
/// this exists to remember and make somebody who came back look new.
#[async_trait::async_trait]
pub trait UserDirectoryStore: Send + Sync {
    /// Record that a subject was seen, creating the entry if it is new.
    ///
    /// The implementation keeps the earliest `first_seen` it has ever held for
    /// a subject. Overwriting it on each sign-in would make everybody look new
    /// and lose the one fact this record exists to hold.
    async fn record_sign_in(&self, user: &RegisteredUser) -> Result<(), StoreError>;

    /// The most recently seen subjects, newest first, at most `query.limit`.
    ///
    /// The limit is pushed into the query rather than applied to the answer:
    /// truncating afterwards still reads the whole table, which is the cost
    /// worth avoiding.
    async fn recent(&self, query: &DirectoryQuery) -> Result<Vec<RegisteredUser>, StoreError>;

    /// The subjects named, in whatever order the store finds them.
    ///
    /// For naming a page of work by its owners. Asking for the whole table to
    /// resolve a screenful of names costs more every day the service runs,
    /// and asking one query per row costs more every row.
    async fn by_subjects(&self, subjects: &[SubjectId]) -> Result<Vec<RegisteredUser>, StoreError>;
}

/// Where a session's credential comes from when nobody is asking for it.
///
/// A session outlives the request that created it, so the credential it
/// presents cannot only be the one that arrived with that request. After a
/// restart, or when a runtime is replaced under a session, something has to
/// supply the owner's current credential without the owner being present — and
/// the core cannot know where credentials come from, so it asks through this.
#[async_trait::async_trait]
pub trait CredentialSource: Send + Sync {
    /// The credential a session should present on its owner's behalf.
    ///
    /// Named per session rather than per subject because two of one owner's
    /// sessions may be entitled to different things: what a session may reach
    /// is a property of the work it was started for, not of the person who
    /// started it.
    ///
    /// `services` is the profile's selection, carried verbatim and never read
    /// here. The core does not know what a service is; it knows only that the
    /// answer depends on which ones were chosen.
    ///
    /// `caller_credential` is present only on the initiating request. A source
    /// may use it to establish per-session custody, but recovery must work from
    /// the custody record because no request credential exists after restart.
    ///
    /// Store failures are returned explicitly. `Ok(None)` means no credential
    /// is available; it must never stand in for unknown durable custody.
    async fn credential_for(
        &self,
        session: &SessionId,
        subject: &SubjectId,
        services: &[ServiceId],
        caller_credential: Option<&crate::session::CallerCredential>,
    ) -> Result<Option<String>, StoreError>;

    /// Select the credential for one exact runtime delivery target.
    ///
    /// Called only after the receiving runtime generation exists and while the
    /// core owns the runtime's durable interaction or lifecycle authority. A
    /// source with pending custody must use this as its ordering point: select
    /// the pending value when one exists and bind it to `target` atomically
    /// before returning it. If a prior target's acknowledgement wins first, the
    /// retry must instead return the newly current value.
    ///
    /// The default re-reads ordinary selection at this late boundary. Sources
    /// that maintain current/pending custody must override it.
    async fn prepare_credential_delivery(
        &self,
        session: &SessionId,
        subject: &SubjectId,
        services: &[ServiceId],
        caller_credential: Option<&crate::session::CallerCredential>,
        _target: &crate::CredentialDeliveryTarget,
    ) -> Result<Option<String>, StoreError> {
        self.credential_for(session, subject, services, caller_credential)
            .await
    }

    /// Confirm a correlated acknowledgement for an exact runtime target.
    ///
    /// Sources with pending custody promote only when both the delivered value
    /// and `target` still match their durable binding. Returning `false` leaves
    /// the runtime without local delivery evidence so a later pass retries.
    async fn confirm_credential_delivery(
        &self,
        _session: &SessionId,
        _subject: &SubjectId,
        _services: &[ServiceId],
        _delivered_value: &str,
        _target: &crate::CredentialDeliveryTarget,
    ) -> Result<bool, StoreError> {
        Ok(true)
    }

    /// Whether this session should receive later subject-token renewals.
    ///
    /// Sources that mint per-session credentials or pin an initiating bearer
    /// return false. On failure callers must conservatively avoid replacing a
    /// credential whose provenance is unknown.
    async fn tracks_subject_credential(&self, _session: &SessionId) -> Result<bool, StoreError> {
        Ok(true)
    }

    /// Give up whatever was issued for a session that has ended.
    ///
    /// Separate from issuing because the two happen at different moments and
    /// the second must happen even when nobody is asking for a credential. A
    /// credential nothing will ever present again is one nobody is watching,
    /// which is the kind worth taking back.
    ///
    /// A store failure leaves the recorded credential discoverable for a later
    /// reconciliation pass and is returned to the caller.
    ///
    /// Implementations must be idempotent for one exact session. Lifecycle and
    /// reconciliation deliberately retry this call when a process can fail
    /// after durable runtime cleanup but before observing custody retirement.
    async fn release(&self, session: &SessionId) -> Result<(), StoreError>;
}

/// The agent's accumulated context, stored whole.
///
/// Separate from the session store because it is a blob, not a document: it
/// grows with the conversation, is written wholesale and read wholesale, and
/// would eventually exceed what a document store will hold.
#[async_trait::async_trait]
pub trait ContextStore: Send + Sync {
    /// Store a session's context, returning a reference to it.
    async fn put(&self, id: &SessionId, bytes: Vec<u8>) -> Result<String, StoreError>;

    /// Read a session's context back, if any was stored.
    async fn get(&self, reference: &str) -> Result<Option<Vec<u8>>, StoreError>;

    /// Remove a session's context.
    async fn delete(&self, reference: &str) -> Result<(), StoreError>;
}

/// Stable object storage for compatibility runtime handovers.
///
/// Separate from [`ContextStore`]: terminal context is a whole transcript at a
/// fixed key, while this is opaque adapter state with a claim-bound envelope.
/// Implementations keep one stable object per session, overwrite it on `put`,
/// retain it after successful publication, and remove it with the session.
#[async_trait::async_trait]
pub trait HandoverStore: Send + Sync {
    /// Store one claim-bound envelope and return its stable reference.
    async fn put(&self, id: &SessionId, snapshot: HandoverSnapshot) -> Result<String, StoreError>;

    /// Load a stored envelope after binding its reference to this session.
    async fn get_for(
        &self,
        id: &SessionId,
        reference: &str,
    ) -> Result<Option<HandoverSnapshot>, StoreError>;

    /// Remove the stable handover object derived for this session.
    async fn delete_for(&self, id: &SessionId) -> Result<(), StoreError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::KernelId;
    use crate::options::AgentOptions;
    use crate::session::{RecoveryState, SessionStatus};
    use crate::terminal::TerminalGeometry;

    fn session() -> Session {
        Session {
            id: SessionId("sesn_1".into()),
            owner: SubjectId("subject-a".into()),
            kernel: KernelId::new("example-runtime"),
            status: SessionStatus::Ready,
            recovery_state: RecoveryState::Normal,
            stop_reason: None,
            options: AgentOptions::default(),
            environment_profile: None,
            geometry: TerminalGeometry::default(),
            last_seq: 0,
            runtime_version: None,
            rotations: 0,
            rotated_at: None,
            capabilities: Default::default(),
            granted_services: None,
            granted_skills: None,
            pricing: None,
            created_at: "2026-01-01T00:00:00Z".into(),
        }
    }

    #[test]
    fn a_new_record_references_no_runtime_and_no_context() {
        // Both are acquired later: a session exists before a sandbox does, and
        // before the agent has said anything worth keeping.
        let r = SessionRecord::new(session());
        assert!(r.runtime_id.is_none());
        assert!(r.context_ref.is_none());
    }

    #[test]
    fn a_record_carries_the_session_unchanged() {
        let s = session();
        let r = SessionRecord::new(s.clone());
        assert_eq!(r.session.id, s.id);
        assert_eq!(r.session.owner, s.owner);
    }
}

/// Where the deployment's own settings are kept.
///
/// A singleton rather than a collection: there is one deployment, so there is
/// one record, and nothing addresses it by identifier. That is why there is no
/// `get(id)` here and why the service rather than the caller decides what an
/// absent record means.
///
/// The save is conditional on what the writer last read. Two administrators
/// with the editor open is not a hypothetical — it is the ordinary case for a
/// setting that changes rarely and is opened by whoever is asking about it —
/// and a last-write-wins save silently discards the other's work.
#[async_trait::async_trait]
pub trait PlatformSettingsStore: Send + Sync {
    /// Read the settings, and the revision they were at.
    ///
    /// `None` where nothing has ever been written, which is a real answer and
    /// not a failure: a deployment nobody has configured is the common case.
    async fn load(&self) -> Result<Option<StoredSettings>, StoreError>;

    /// Write these settings, but only if nobody has written since `expected`.
    ///
    /// `Ok(None)` means somebody had, and the caller is told rather than
    /// having their write applied over the top.
    ///
    /// `expected` is a plain number and **zero means nothing is stored**, which
    /// is the revision an unconfigured deployment reports. An `Option` here
    /// carried no information the number did not: `None` and `Some(0)` were
    /// the same state described twice, and the first write on a fresh
    /// deployment failed depending on which one a caller happened to send.
    async fn save_if_unchanged(
        &self,
        expected: u64,
        settings: &crate::PlatformSettings,
    ) -> Result<Option<StoredSettings>, StoreError>;
}

/// Settings as they were read, with the revision that identifies them.
#[derive(Debug, Clone)]
pub struct StoredSettings {
    pub settings: crate::PlatformSettings,
    /// Increments on every write, and is **zero when nothing is stored**.
    ///
    /// Carried by a client so its next save can be refused rather than
    /// silently overwriting somebody else's. Zero is a real revision that a
    /// client sends back like any other, so the first write on a fresh
    /// deployment takes the same conditional path as every write after it
    /// rather than a special case that skips the check.
    pub revision: u64,
    /// What was stored before this write, so an overwrite is recoverable.
    ///
    /// One step and no more: this exists so a mistake can be undone in the
    /// moment, not so the service keeps a history nobody asked it to keep.
    pub previous: Option<crate::PlatformSettings>,
}

/// One skill, fetched and ready to be written.
#[derive(Debug, Clone)]
pub struct SkillPack {
    /// The directory it occupies, which is the name the agent invokes.
    pub name: String,
    /// Which version of it this is, where the registry said.
    ///
    /// Carried from the fetch rather than looked up later, so what a session
    /// reports is the version it actually received. Asking the registry
    /// afterwards would answer with whatever is current, which is a different
    /// fact and is wrong for every session that has been running a while.
    pub version: Option<String>,
    /// Each file, relative to that directory, exactly as the registry names it.
    pub files: Vec<(String, Vec<u8>)>,
}

/// Where a session's skills come from.
///
/// The core knows a session was granted skills and knows nothing about where
/// they live or how one is resolved — the same division as a credential.
/// Named per skill and fetched as the caller, because which skills exist
/// depends on who is asking.
#[async_trait::async_trait]
pub trait SkillSource: Send + Sync {
    /// Fetch each of these, or say which one could not be had.
    ///
    /// Failing by name matters: a session started without a skill it was
    /// promised looks identical to one that never asked for it, and the
    /// difference only surfaces when the agent cannot do what it was told it
    /// could.
    async fn packs_for(
        &self,
        caller_credential: Option<&str>,
        skills: &[crate::environment::SkillId],
    ) -> Result<Vec<SkillPack>, String>;
}

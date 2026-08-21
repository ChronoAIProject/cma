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

use crate::activity::{ActivityQuery, ActivityRecord};
use crate::directory::{DirectoryQuery, RegisteredUser};
use crate::environment::{EnvironmentProfile, ServiceId};
use crate::event::{Event, StoredEvent};
use crate::ids::{EnvironmentProfileId, SessionId, SubjectId};
use crate::session::Session;

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
}

impl SessionRecord {
    /// A record for a freshly created session.
    pub fn new(session: Session) -> Self {
        Self {
            session,
            runtime_id: None,
            context_ref: None,
        }
    }
}

/// Failures a store may report.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// The store could not be reached.
    #[error("store unavailable: {0}")]
    Unavailable(String),
    /// The stored data did not match what the core expects.
    #[error("stored data not usable: {0}")]
    Malformed(String),
    /// A batch claimed a sequence the session already has.
    ///
    /// Named rather than folded into `Unavailable` because the two call for
    /// opposite responses: a store that is down is retried unchanged, and a
    /// sequence already taken can only ever be retried under a different one.
    /// A caller that cannot tell them apart does neither, which is how a turn's
    /// events came to be dropped with nothing but a log line to say so.
    ///
    /// `written` is how many of the batch the store accepted before the
    /// collision. A caller re-numbering the remainder must skip those, or it
    /// writes them a second time under a sequence nothing rejects.
    #[error("sequence already taken: {written} of {total} events written")]
    SequenceTaken {
        /// Events accepted before the collision.
        written: usize,
        /// Events in the batch.
        total: usize,
    },
}

/// Durable session records and their event history.
#[async_trait::async_trait]
pub trait SessionStore: Send + Sync {
    /// Create or replace a session record.
    async fn upsert(&self, record: &SessionRecord) -> Result<(), StoreError>;

    /// Read one session, whoever owns it. Ownership is decided above this.
    async fn get(&self, id: &SessionId) -> Result<Option<SessionRecord>, StoreError>;

    /// Every session owned by a subject, newest first.
    async fn list_for(&self, owner: &SubjectId) -> Result<Vec<SessionRecord>, StoreError>;

    /// Every session, newest first. For the monitoring surface.
    async fn list_all(&self) -> Result<Vec<SessionRecord>, StoreError>;

    /// Remove a session and its history permanently.
    ///
    /// Distinct from ending one: a terminated session stays visible until its
    /// owner chooses to remove it.
    async fn delete(&self, id: &SessionId) -> Result<(), StoreError>;

    /// Append durable events.
    ///
    /// Sequence numbers are allocated by the caller and are monotonic per
    /// session, so a store must preserve them rather than assign its own.
    ///
    /// **A sequence a session already has must be refused**, and refused as
    /// `SequenceTaken` rather than as a general failure. The core allocates,
    /// but more than one process may be allocating for the same session, so
    /// the store is the only place the clash is visible. A store that accepts
    /// a duplicate leaves two events answering to one cursor, and the client
    /// reading history sees one of them.
    async fn append_events(&self, events: &[Event]) -> Result<(), StoreError>;

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
    /// Create or replace a profile.
    async fn upsert(&self, profile: &EnvironmentProfile) -> Result<(), StoreError>;

    /// Read one profile, whoever owns it.
    async fn get(
        &self,
        id: &EnvironmentProfileId,
    ) -> Result<Option<EnvironmentProfile>, StoreError>;

    /// Every profile owned by a subject, newest first.
    async fn list_for(&self, owner: &SubjectId) -> Result<Vec<EnvironmentProfile>, StoreError>;

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
    async fn credential_for(
        &self,
        session: &SessionId,
        subject: &SubjectId,
        services: &[ServiceId],
    ) -> Option<String>;

    /// Give up whatever was issued for a session that has ended.
    ///
    /// Separate from issuing because the two happen at different moments and
    /// the second must happen even when nobody is asking for a credential. A
    /// credential nothing will ever present again is one nobody is watching,
    /// which is the kind worth taking back.
    async fn release(&self, session: &SessionId);
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

/// One skill, fetched and ready to be written.
#[derive(Debug, Clone)]
pub struct SkillPack {
    /// The directory it occupies, which is the name the agent invokes.
    pub name: String,
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

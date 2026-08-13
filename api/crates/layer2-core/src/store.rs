//! Where sessions and their history are kept.
//!
//! The core defines what durable storage must do and never how. Two ports,
//! because the two kinds of state have genuinely different shapes: session
//! records are small, queryable documents, while an agent's accumulated
//! context is an opaque blob that grows without bound and is only ever read
//! whole.
//!
//! Nothing here is live state. A screen model, a socket, and an attachment
//! lease exist only while a runtime is running and are rebuilt on attach;
//! persisting them would be persisting a snapshot of something already gone.

use crate::environment::EnvironmentProfile;
use crate::event::Event;
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
    async fn append_events(&self, events: &[Event]) -> Result<(), StoreError>;

    /// Events after a cursor, in sequence order.
    async fn events_since(&self, id: &SessionId, after: u64) -> Result<Vec<Event>, StoreError>;
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

/// Where a session's credential comes from when nobody is asking for it.
///
/// A session outlives the request that created it, so the credential it
/// presents cannot only be the one that arrived with that request. After a
/// restart, or when a runtime is replaced under a session, something has to
/// supply the owner's current credential without the owner being present — and
/// the core cannot know where credentials come from, so it asks through this.
#[async_trait::async_trait]
pub trait CredentialSource: Send + Sync {
    /// The credential to present on a subject's behalf, if one is available.
    async fn credential_for(&self, subject: &SubjectId) -> Option<String>;
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

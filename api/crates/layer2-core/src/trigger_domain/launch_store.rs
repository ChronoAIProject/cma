use async_trait::async_trait;

use crate::ids::PublicTriggerId;
use crate::{LaunchId, ServiceHandoffId, SubjectId};

use super::{
    Launch, LaunchEvent, LaunchEventKind, LaunchState, PublicTrigger, ServiceConnectionHandoff,
    TriggerError,
};

mod memory;

pub use memory::InMemoryLaunchStore;

pub const DEFAULT_LAUNCH_EVENT_LIMIT: u16 = 100;
pub const MAX_LAUNCH_EVENT_LIMIT: u16 = 200;

#[derive(Debug, Clone)]
pub struct LaunchAdmission {
    pub launch: Launch,
    pub trigger: PublicTrigger,
    pub events: Vec<LaunchEventKind>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchAdmissionResult {
    Created(Launch),
    Existing(Launch),
}

impl LaunchAdmissionResult {
    pub fn launch(&self) -> &Launch {
        match self {
            Self::Created(launch) | Self::Existing(launch) => launch,
        }
    }

    pub const fn was_created(&self) -> bool {
        matches!(self, Self::Created(_))
    }
}

#[derive(Debug, Clone)]
pub struct LaunchCommit {
    pub expected_revision: u64,
    pub launch: Launch,
    pub event_at: String,
    pub events: Vec<LaunchEventKind>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LaunchStoreError {
    #[error("launch not found")]
    NotFound,
    #[error("launch id already exists")]
    Duplicate,
    #[error("idempotency key is already bound to different launch input")]
    CommandConflict,
    #[error("launch revision conflict: expected {expected}, found {actual}")]
    Conflict { expected: u64, actual: u64 },
    #[error("launch event cursor is invalid")]
    InvalidCursor,
    #[error("published trigger is unavailable")]
    TriggerUnavailable,
    #[error("published trigger is retired")]
    TriggerRetired,
    #[error("launch work lease is held")]
    LeaseHeld,
    #[error("launch store unavailable: {0}")]
    Unavailable(String),
    #[error("stored launch data is not usable: {0}")]
    Corrupt(String),
    #[error(transparent)]
    Validation(#[from] TriggerError),
}

impl LaunchStoreError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::NotFound => "launch_not_found",
            Self::Duplicate => "launch_already_exists",
            Self::CommandConflict => "launch_idempotency_conflict",
            Self::Conflict { .. } => "launch_changed",
            Self::InvalidCursor => "invalid_launch_cursor",
            Self::TriggerUnavailable => "trigger_not_found",
            Self::TriggerRetired => "trigger_retired",
            Self::LeaseHeld => "launch_lease_held",
            Self::Unavailable(_) => "launch_store_unavailable",
            Self::Corrupt(_) => "stored_launch_corrupt",
            Self::Validation(error) => error.code,
        }
    }
}

#[async_trait]
pub trait LaunchStore: Send + Sync {
    /// Atomically verifies the exact published Trigger revision, inserts the
    /// Launch and its initial events, and increments the Trigger launch count
    /// plus the ready count when admission itself resolves an existing Agent.
    /// A replay of the same command returns the original Launch; the same key
    /// with a different fingerprint fails without changing either aggregate.
    async fn admit_launch(
        &self,
        admission: LaunchAdmission,
    ) -> Result<LaunchAdmissionResult, LaunchStoreError>;

    /// Looks up an already admitted owner command before any mutable launch
    /// dependencies are consulted.
    async fn launch_by_command(
        &self,
        owner: &SubjectId,
        public_id: &PublicTriggerId,
        command_key_hash: &str,
    ) -> Result<Option<Launch>, LaunchStoreError>;

    async fn launch(
        &self,
        id: &LaunchId,
        owner: &SubjectId,
    ) -> Result<Option<Launch>, LaunchStoreError>;

    /// Commits the aggregate and events and atomically increments the Trigger
    /// ready count on the first transition that records `ready_at`.
    async fn commit_launch(&self, commit: LaunchCommit) -> Result<Launch, LaunchStoreError>;

    async fn launch_events(
        &self,
        id: &LaunchId,
        owner: &SubjectId,
        after: u64,
        limit: u16,
    ) -> Result<Vec<LaunchEvent>, LaunchStoreError>;

    async fn due_launches(&self, now: &str, limit: u16) -> Result<Vec<LaunchId>, LaunchStoreError>;

    async fn claim_launch(
        &self,
        id: &LaunchId,
        holder: &str,
        now: String,
        lease_expires_at: String,
    ) -> Result<Option<Launch>, LaunchStoreError>;
}

#[async_trait]
pub trait ServiceConnectionStore: Send + Sync {
    async fn create_handoff(
        &self,
        handoff: ServiceConnectionHandoff,
    ) -> Result<ServiceConnectionHandoff, LaunchStoreError>;

    async fn handoff(
        &self,
        id: &ServiceHandoffId,
        owner: &SubjectId,
    ) -> Result<Option<ServiceConnectionHandoff>, LaunchStoreError>;

    async fn save_handoff(
        &self,
        expected_revision: u64,
        handoff: ServiceConnectionHandoff,
    ) -> Result<ServiceConnectionHandoff, LaunchStoreError>;
}

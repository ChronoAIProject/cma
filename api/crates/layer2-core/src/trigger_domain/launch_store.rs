use async_trait::async_trait;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use serde::{Deserialize, Serialize};

use crate::ids::{PlacementId, PublicTriggerId, TriggerId};
use crate::{LaunchId, ServiceHandoffId, SubjectId};

use super::{
    Launch, LaunchEvent, LaunchEventKind, LaunchState, PublicTrigger, ServiceConnectionHandoff,
    TriggerError,
};

mod memory;
mod memory_queries;

pub use memory::InMemoryLaunchStore;

pub const DEFAULT_LAUNCH_EVENT_LIMIT: u16 = 100;
pub const MAX_LAUNCH_EVENT_LIMIT: u16 = 200;
pub const DEFAULT_LAUNCH_PAGE_SIZE: u16 = 50;
pub const MAX_LAUNCH_PAGE_SIZE: u16 = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchPageRequest {
    pub limit: u16,
    pub cursor: Option<String>,
    pub trigger: Option<TriggerId>,
    pub state: Option<LaunchState>,
}

impl Default for LaunchPageRequest {
    fn default() -> Self {
        Self {
            limit: DEFAULT_LAUNCH_PAGE_SIZE,
            cursor: None,
            trigger: None,
            state: None,
        }
    }
}

impl LaunchPageRequest {
    pub fn validate(&self) -> Result<(), LaunchStoreError> {
        if self.limit == 0 || self.limit > MAX_LAUNCH_PAGE_SIZE {
            return Err(LaunchStoreError::InvalidCursor);
        }
        if self.trigger.as_ref().is_some_and(|trigger| {
            !trigger.as_str().starts_with("trg_") || trigger.as_str().len() == 4
        }) {
            return Err(LaunchStoreError::InvalidCursor);
        }
        self.cursor_boundary().map(|_| ())
    }

    #[doc(hidden)]
    pub fn cursor_boundary(&self) -> Result<Option<LaunchPageBoundary>, LaunchStoreError> {
        let Some(encoded) = self.cursor.as_deref() else {
            return Ok(None);
        };
        if encoded.is_empty() || encoded.len() > 2_048 {
            return Err(LaunchStoreError::InvalidCursor);
        }
        let bytes = URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|_| LaunchStoreError::InvalidCursor)?;
        let cursor: LaunchCursor =
            serde_json::from_slice(&bytes).map_err(|_| LaunchStoreError::InvalidCursor)?;
        if cursor.version != 1
            || crate::seconds_from_rfc3339(&cursor.created_at).is_none()
            || !cursor.id.as_str().starts_with("lch_")
            || cursor.id.as_str().len() == 4
            || cursor.trigger != self.trigger
            || cursor.state != self.state
        {
            return Err(LaunchStoreError::InvalidCursor);
        }
        Ok(Some(LaunchPageBoundary {
            created_at: cursor.created_at,
            id: cursor.id,
        }))
    }

    #[doc(hidden)]
    pub fn cursor_after(&self, launch: &Launch) -> Result<String, LaunchStoreError> {
        if self
            .trigger
            .as_ref()
            .is_some_and(|id| id != &launch.trigger_id)
            || self.state.is_some_and(|state| state != launch.state)
        {
            return Err(LaunchStoreError::Corrupt(
                "Launch page cursor does not match its filters".into(),
            ));
        }
        let bytes = serde_json::to_vec(&LaunchCursor {
            version: 1,
            created_at: launch.created_at.clone(),
            id: launch.id.clone(),
            trigger: self.trigger.clone(),
            state: self.state,
        })
        .map_err(|error| LaunchStoreError::Corrupt(error.to_string()))?;
        Ok(URL_SAFE_NO_PAD.encode(bytes))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[doc(hidden)]
pub struct LaunchPageBoundary {
    pub created_at: String,
    pub id: LaunchId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchPage {
    pub items: Vec<Launch>,
    pub next: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchPlacementStats {
    pub placement_id: Option<PlacementId>,
    pub launches_total: u64,
    pub agents_ready_total: u64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LaunchCursor {
    version: u8,
    created_at: String,
    id: LaunchId,
    trigger: Option<TriggerId>,
    state: Option<LaunchState>,
}

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
    #[error("daily visitor Launch allowance exhausted")]
    VisitorQuota {
        limit: u32,
        reset_at: std::time::SystemTime,
    },
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
    #[error("model budget refused the Launch: {0}")]
    Budget(crate::budget::BudgetError),
    #[error("new work is temporarily paused; existing work continues")]
    AdmissionPaused,
}

impl LaunchStoreError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::VisitorQuota { .. } => "resource_quota",
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
            Self::Budget(reason) => reason.code(),
            Self::AdmissionPaused => "admission_paused",
        }
    }
}

#[async_trait]
pub trait LaunchStore: Send + Sync {
    /// Read the owner's admitted visitor count for early refusal only.
    /// The admission transaction remains the authority, including when this
    /// optional optimization returns its default count of zero.
    async fn visitor_launches_admitted(
        &self,
        _owner: &SubjectId,
        _utc_day: u64,
    ) -> Result<u32, LaunchStoreError> {
        Ok(0)
    }

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

    /// The earliest durable Launch that resolved this owned Agent. This also
    /// records Trigger provenance when an automatic prepared Agent was adopted.
    async fn source_launch(
        &self,
        owner: &SubjectId,
        agent_id: &crate::AgentId,
        trigger: Option<&TriggerId>,
    ) -> Result<Option<Launch>, LaunchStoreError> {
        let mut page = LaunchPageRequest {
            limit: MAX_LAUNCH_PAGE_SIZE,
            trigger: trigger.cloned(),
            ..Default::default()
        };
        let mut oldest = None;
        let mut seen = std::collections::BTreeSet::new();
        loop {
            let rows = self.list_launches(owner, &page).await?;
            for launch in rows.items {
                if launch.owner != *owner {
                    return Err(LaunchStoreError::Corrupt(
                        "Agent source crossed its owner boundary".into(),
                    ));
                }
                if launch.resolution.agent.id() == Some(agent_id) {
                    oldest = Some(launch);
                }
            }
            let Some(next) = rows.next else {
                return Ok(oldest);
            };
            if !seen.insert(next.clone()) {
                return Err(LaunchStoreError::Corrupt(
                    "Agent source pagination did not advance".into(),
                ));
            }
            page.cursor = Some(next);
        }
    }

    async fn list_launches(
        &self,
        owner: &SubjectId,
        page: &LaunchPageRequest,
    ) -> Result<LaunchPage, LaunchStoreError>;

    async fn placement_stats(
        &self,
        trigger_id: &TriggerId,
    ) -> Result<Vec<LaunchPlacementStats>, LaunchStoreError>;

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
    async fn handoff_by_intent(
        &self,
        owner: &SubjectId,
        launch_id: &LaunchId,
        intent_id: &str,
    ) -> Result<Option<ServiceConnectionHandoff>, LaunchStoreError>;

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

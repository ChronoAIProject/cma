//! Durable write-once title coordination for agents.

use std::time::{Duration, SystemTime};

use async_trait::async_trait;

use crate::ids::AgentId;

use super::store::{Page, PageRequest};
use super::{AgentTitle, AgentTitleSource, AgentTitleTranscript, AGENT_TITLE_CLAIM_SECONDS};

mod memory;

pub use memory::InMemoryAgentTitleStore;

/// Whether a newly acquired claim may make model calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentTitleClaimMode {
    Generate,
    FallbackOnly,
}

/// One idempotent attempt to acquire title authority for an agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTitleClaimRequest {
    agent_id: AgentId,
    holder: String,
    token: String,
}

impl AgentTitleClaimRequest {
    pub fn new(agent_id: AgentId, holder: impl Into<String>) -> Self {
        Self {
            agent_id,
            holder: holder.into(),
            token: format!("agent_title_claim_{}", uuid::Uuid::new_v4().simple()),
        }
    }

    pub fn agent_id(&self) -> &AgentId {
        &self.agent_id
    }

    pub fn holder(&self) -> &str {
        &self.holder
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    pub const fn ttl(&self) -> Duration {
        Duration::from_secs(AGENT_TITLE_CLAIM_SECONDS)
    }
}

/// Exact token and deadline fencing one title write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTitleClaim {
    request: AgentTitleClaimRequest,
    mode: AgentTitleClaimMode,
    acquired_at: SystemTime,
    expires_at: SystemTime,
    attempts: u32,
    terminal_fallback: bool,
}

impl AgentTitleClaim {
    /// Construct the claim returned by a durable store adapter.
    #[doc(hidden)]
    pub fn acquired(
        request: AgentTitleClaimRequest,
        mode: AgentTitleClaimMode,
        acquired_at: SystemTime,
        expires_at: SystemTime,
    ) -> Self {
        Self {
            request,
            mode,
            acquired_at,
            expires_at,
            attempts: 0,
            terminal_fallback: false,
        }
    }

    #[doc(hidden)]
    pub fn with_attempts(mut self, attempts: u32) -> Self {
        self.attempts = attempts;
        self
    }
    pub const fn attempts(&self) -> u32 {
        self.attempts
    }
    pub fn settle_fallback(mut self) -> Self {
        self.terminal_fallback = true;
        self
    }
    pub const fn terminal_fallback(&self) -> bool {
        self.terminal_fallback
    }
    pub fn next_attempt_at(&self) -> SystemTime {
        self.acquired_at
            + Duration::from_secs(
                super::AGENT_TITLE_RETRY_SECONDS * (1 << self.attempts.saturating_sub(1).min(2)),
            )
    }

    pub fn agent_id(&self) -> &AgentId {
        self.request.agent_id()
    }

    pub fn holder(&self) -> &str {
        self.request.holder()
    }

    pub fn token(&self) -> &str {
        self.request.token()
    }

    pub const fn mode(&self) -> AgentTitleClaimMode {
        self.mode
    }

    pub const fn acquired_at(&self) -> SystemTime {
        self.acquired_at
    }

    pub const fn expires_at(&self) -> SystemTime {
        self.expires_at
    }
}

/// The committed title projection returned to Layer 2 consumers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredAgentTitle {
    pub agent_id: AgentId,
    pub title: AgentTitle,
    pub source: AgentTitleSource,
    pub set_at: String,
}

/// Result of a write-once user title request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetAgentTitleOutcome {
    Set,
    AlreadySet,
}

/// Content-free failures from the durable title boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AgentTitleStoreError {
    #[error("agent title record not found")]
    NotFound,
    #[error("agent title record already exists")]
    Duplicate,
    #[error("invalid agent title claim")]
    InvalidClaim,
    #[error("invalid agent title timestamp")]
    InvalidTimestamp,
    #[error("agent log sequence would not be gap-free")]
    SequenceConflict,
    #[error("agent log capacity exceeded")]
    AgentLogCapacityExceeded,
    #[error("invalid agent title page")]
    InvalidPage,
    #[error("agent title store unavailable")]
    Unavailable,
    #[error("stored agent title data is not usable")]
    Corrupt,
}

/// Atomic title discovery, claiming, completion, and user override boundary.
#[async_trait]
pub trait AgentTitleStore: Send + Sync {
    /// Walk raw agents in deterministic pages, returning eligible titleless rows.
    async fn agent_title_candidates(
        &self,
        page: &PageRequest,
    ) -> Result<Page<AgentId>, AgentTitleStoreError>;

    /// Read the bounded transcript proven by the first completed turn.
    async fn agent_title_transcript(
        &self,
        id: &AgentId,
    ) -> Result<Option<AgentTitleTranscript>, AgentTitleStoreError>;

    /// Atomically verify eligibility and acquire independent title authority.
    async fn acquire_agent_title_claim(
        &self,
        request: &AgentTitleClaimRequest,
        now: SystemTime,
    ) -> Result<Option<AgentTitleClaim>, AgentTitleStoreError>;

    /// Commit a generated or fallback title under the exact current claim.
    ///
    /// `true` includes a retry of the exact claim after its commit response was
    /// lost. `false` means a title or successor claim fenced this caller.
    async fn complete_agent_title(
        &self,
        claim: &AgentTitleClaim,
        title: &AgentTitle,
        source: AgentTitleSource,
        set_at: String,
    ) -> Result<bool, AgentTitleStoreError>;

    /// Commit a user title once, suppressing any pending automatic completion.
    async fn set_user_agent_title(
        &self,
        id: &AgentId,
        title: &AgentTitle,
        set_at: String,
    ) -> Result<SetAgentTitleOutcome, AgentTitleStoreError>;

    async fn agent_title(
        &self,
        id: &AgentId,
    ) -> Result<Option<StoredAgentTitle>, AgentTitleStoreError>;
}

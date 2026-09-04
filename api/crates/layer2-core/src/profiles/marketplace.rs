use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::ids::{AgentProfileId, SandboxProfileId, SubjectId, WorkspaceProfileId};

use super::{
    AgentProfile, AgentProfileRevision, ProfileCapacityBounds, ProfileVisibility, SandboxProfile,
    SandboxProfileRevision, WorkspaceProfile, WorkspaceProfileRevision,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MarketplaceKind {
    Agent,
    Workspace,
    Sandbox,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarketplaceLocator {
    Agent(AgentProfileId),
    Workspace(WorkspaceProfileId),
    Sandbox(SandboxProfileId),
}

impl MarketplaceLocator {
    pub fn kind(&self) -> MarketplaceKind {
        match self {
            Self::Agent(_) => MarketplaceKind::Agent,
            Self::Workspace(_) => MarketplaceKind::Workspace,
            Self::Sandbox(_) => MarketplaceKind::Sandbox,
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Agent(id) => id.as_str(),
            Self::Workspace(id) => id.as_str(),
            Self::Sandbox(id) => id.as_str(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketplaceQuery {
    pub kind: Option<MarketplaceKind>,
    pub text: Option<String>,
    pub cursor: Option<String>,
    pub limit: u16,
}

impl Default for MarketplaceQuery {
    fn default() -> Self {
        Self {
            kind: None,
            text: None,
            cursor: None,
            limit: 50,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarketplaceSummary {
    pub kind: MarketplaceKind,
    pub id: String,
    pub name: String,
    pub description: String,
    pub owner_display: String,
    pub latest_revision: u32,
    pub published_at: String,
    pub uses_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketplacePage {
    pub items: Vec<MarketplaceSummary>,
    pub next: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedProfileName {
    pub kind: MarketplaceKind,
    pub id: String,
    pub revision: u32,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarketplaceRevision {
    Agent(AgentProfileRevision),
    Workspace(WorkspaceProfileRevision),
    Sandbox(SandboxProfileRevision),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedProfileRevision {
    /// Internal ownership evidence used to compute caller-relative trust.
    pub owner: SubjectId,
    pub name: String,
    pub revision: MarketplaceRevision,
}

impl ResolvedProfileRevision {
    pub fn resolved_name(&self) -> ResolvedProfileName {
        let (kind, id, revision) = match &self.revision {
            MarketplaceRevision::Agent(revision) => (
                MarketplaceKind::Agent,
                revision.profile_id.to_string(),
                revision.revision,
            ),
            MarketplaceRevision::Workspace(revision) => (
                MarketplaceKind::Workspace,
                revision.profile_id.to_string(),
                revision.revision,
            ),
            MarketplaceRevision::Sandbox(revision) => (
                MarketplaceKind::Sandbox,
                revision.profile_id.to_string(),
                revision.revision,
            ),
        };
        ResolvedProfileName {
            kind,
            id,
            revision,
            name: self.name.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketplaceEntry {
    /// Internal ownership evidence used to compute caller-relative trust.
    pub owner: SubjectId,
    pub summary: MarketplaceSummary,
    pub revision: MarketplaceRevision,
    pub resolved_profiles: Vec<ResolvedProfileName>,
    pub resolved_revisions: Vec<ResolvedProfileRevision>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForkedProfile {
    Agent(AgentProfile),
    Workspace(WorkspaceProfile),
    Sandbox(SandboxProfile),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustConfirmation {
    pub commands_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MarketplaceError {
    #[error("marketplace is disabled")]
    Disabled,
    #[error("profile not found")]
    NotFound,
    #[error("profile cannot be changed by this subject")]
    Forbidden,
    #[error("profile has no published revision")]
    Unpublished,
    #[error("profile validation failed: {0}")]
    Validation(String),
    #[error("trust confirmation required for digest {commands_digest}")]
    TrustConfirmationRequired { commands_digest: String },
    #[error("invalid marketplace page")]
    InvalidPage,
    #[error("marketplace store unavailable: {0}")]
    Unavailable(String),
    #[error("stored marketplace data is not usable: {0}")]
    Corrupt(String),
}

#[async_trait]
pub trait MarketplaceStore: Send + Sync {
    async fn list_marketplace(
        &self,
        query: &MarketplaceQuery,
    ) -> Result<MarketplacePage, MarketplaceError>;
    async fn read_marketplace(
        &self,
        locator: &MarketplaceLocator,
    ) -> Result<MarketplaceEntry, MarketplaceError>;
    async fn set_profile_visibility(
        &self,
        owner: &SubjectId,
        locator: &MarketplaceLocator,
        visibility: ProfileVisibility,
        updated_at: String,
        bounds: ProfileCapacityBounds,
        secret_patterns: &[String],
    ) -> Result<(), MarketplaceError>;
    async fn fork_profile(
        &self,
        owner: &SubjectId,
        locator: &MarketplaceLocator,
        revision: u32,
        now: String,
    ) -> Result<ForkedProfile, MarketplaceError>;
}

pub struct MarketplaceService<S> {
    store: Arc<S>,
    enabled: bool,
}

impl<S: MarketplaceStore> MarketplaceService<S> {
    pub fn new(store: Arc<S>, enabled: bool) -> Self {
        Self { store, enabled }
    }

    pub async fn list(
        &self,
        query: &MarketplaceQuery,
    ) -> Result<MarketplacePage, MarketplaceError> {
        self.require_enabled()?;
        self.store.list_marketplace(query).await
    }

    pub async fn read(
        &self,
        locator: &MarketplaceLocator,
    ) -> Result<MarketplaceEntry, MarketplaceError> {
        self.require_enabled()?;
        self.store.read_marketplace(locator).await
    }

    pub async fn fork(
        &self,
        owner: &SubjectId,
        locator: &MarketplaceLocator,
        revision: u32,
        now: String,
    ) -> Result<ForkedProfile, MarketplaceError> {
        self.require_enabled()?;
        self.store.fork_profile(owner, locator, revision, now).await
    }

    fn require_enabled(&self) -> Result<(), MarketplaceError> {
        self.enabled.then_some(()).ok_or(MarketplaceError::Disabled)
    }
}

pub fn require_trust_confirmation(
    owner: &SubjectId,
    consumer: &SubjectId,
    has_commands_or_environment: bool,
    revision_digest: &str,
    confirmed_digest: Option<&str>,
) -> Result<Option<TrustConfirmation>, MarketplaceError> {
    if owner == consumer || !has_commands_or_environment {
        return Ok(None);
    }
    match confirmed_digest {
        None => Err(MarketplaceError::TrustConfirmationRequired {
            commands_digest: revision_digest.to_string(),
        }),
        Some(digest) if digest == revision_digest => Ok(Some(TrustConfirmation {
            commands_digest: digest.to_string(),
        })),
        Some(_) => Err(MarketplaceError::TrustConfirmationRequired {
            commands_digest: revision_digest.to_string(),
        }),
    }
}

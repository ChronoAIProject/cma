use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::resource_access::{
    ResourceAccessSnapshot, ResourceAccessStoreError, ResourceAuthorizationAudit,
    ResourceCapability, ResourceOwner, ResourceTarget,
};
use crate::{AgentId, SandboxId, WorkspaceId, WorktreeId};

pub const MAX_ARTIFACT_BYTES: usize = 1024 * 1024;
pub const MAX_AGENT_ARTIFACTS: usize = 256;
pub const MAX_ARTIFACT_PAGE: u16 = 50;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublishAgentArtifact {
    pub name: String,
    pub media_type: String,
    pub content_base64: String,
}
impl PublishAgentArtifact {
    pub fn content(&self) -> Result<Vec<u8>, ResourceAccessStoreError> {
        if self.name.is_empty()
            || self.name.len() > 160
            || self.name.trim() != self.name
            || matches!(self.name.as_str(), "." | "..")
            || self
                .name
                .chars()
                .any(|c| c.is_control() || matches!(c, '/' | '\\'))
            || !matches!(
                self.media_type.as_str(),
                "text/plain"
                    | "text/markdown"
                    | "application/json"
                    | "application/pdf"
                    | "application/octet-stream"
                    | "image/png"
                    | "image/jpeg"
                    | "image/webp"
            )
            || self.content_base64.len() > MAX_ARTIFACT_BYTES.div_ceil(3) * 4
        {
            return Err(ResourceAccessStoreError::Invalid);
        }
        let bytes = STANDARD
            .decode(&self.content_base64)
            .map_err(|_| ResourceAccessStoreError::Invalid)?;
        if bytes.is_empty()
            || bytes.len() > MAX_ARTIFACT_BYTES
            || STANDARD.encode(&bytes) != self.content_base64
        {
            return Err(ResourceAccessStoreError::Invalid);
        }
        Ok(bytes)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentArtifactMetadata {
    pub id: String,
    pub agent_id: AgentId,
    pub name: String,
    pub media_type: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub created_at: String,
}

/// Immutable retained ancestry. A recycled Agent or changed root identity must
/// never inherit the previous resource's published objects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactBinding {
    pub sandbox_id: SandboxId,
    pub sandbox_created_at: String,
    pub workspace_id: WorkspaceId,
    pub workspace_created_at: String,
    pub worktree_id: WorktreeId,
    pub worktree_identity_sha256: String,
    pub agent_id: AgentId,
    pub agent_created_at: String,
    pub owner: ResourceOwner,
}
impl ArtifactBinding {
    pub fn from_snapshot(
        snapshot: &ResourceAccessSnapshot,
    ) -> Result<Self, ResourceAccessStoreError> {
        let lineage = &snapshot.lineage;
        let agent = lineage
            .agent
            .as_ref()
            .ok_or(ResourceAccessStoreError::Corrupt)?;
        let workspace = lineage
            .workspace
            .as_ref()
            .ok_or(ResourceAccessStoreError::Corrupt)?;
        let worktree = lineage
            .worktree
            .as_ref()
            .ok_or(ResourceAccessStoreError::Corrupt)?;
        let target = ResourceTarget::Agent {
            agent_id: agent.id.clone(),
        };
        Ok(Self {
            sandbox_id: lineage.sandbox.id.clone(),
            sandbox_created_at: lineage.sandbox.created_at.clone(),
            workspace_id: workspace.id.clone(),
            workspace_created_at: workspace.created_at.clone(),
            worktree_id: worktree.id.clone(),
            worktree_identity_sha256: digest(
                &serde_json::to_vec(&(
                    &worktree.name,
                    &worktree.path,
                    &worktree.branch,
                    &worktree.base_commit,
                ))
                .map_err(|_| ResourceAccessStoreError::Corrupt)?,
            ),
            agent_id: agent.id.clone(),
            agent_created_at: agent.created_at.clone(),
            owner: snapshot.scope(&target)?.owner,
        })
    }
}

/// Internal immutable bytes and historical publication evidence. Only metadata
/// and the separately authorized byte download are public projections.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredAgentArtifact {
    pub metadata: AgentArtifactMetadata,
    pub binding: ArtifactBinding,
    pub content_base64: String,
    pub authorization: ResourceAuthorizationAudit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredAgentArtifactMetadata {
    pub metadata: AgentArtifactMetadata,
    pub binding: ArtifactBinding,
    pub authorization: ResourceAuthorizationAudit,
}

impl StoredAgentArtifactMetadata {
    pub fn validate(&self) -> Result<(), ResourceAccessStoreError> {
        self.authorization.validate()?;
        PublishAgentArtifact {
            name: self.metadata.name.clone(),
            media_type: self.metadata.media_type.clone(),
            content_base64: "eA==".into(),
        }
        .content()
        .map_err(|_| ResourceAccessStoreError::Corrupt)?;
        if !valid_artifact_id(&self.metadata.id)
            || self.metadata.agent_id != self.binding.agent_id
            || !(1..=MAX_ARTIFACT_BYTES as u64).contains(&self.metadata.size_bytes)
            || self.metadata.sha256.len() != 64
            || !self
                .metadata
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            || crate::seconds_from_rfc3339(&self.metadata.created_at).is_none()
            || self.authorization.target
                != (ResourceTarget::Agent {
                    agent_id: self.binding.agent_id.clone(),
                })
            || self.authorization.owner != self.binding.owner
            || self.authorization.sandbox_id != self.binding.sandbox_id
            || self.authorization.sandbox_created_at != self.binding.sandbox_created_at
            || self.authorization.capability != ResourceCapability::ManageSharing
            || self.authorization.captured_at != self.metadata.created_at
        {
            return Err(ResourceAccessStoreError::Corrupt);
        }
        Ok(())
    }
}

impl StoredAgentArtifact {
    pub fn metadata_record(&self) -> StoredAgentArtifactMetadata {
        StoredAgentArtifactMetadata {
            metadata: self.metadata.clone(),
            binding: self.binding.clone(),
            authorization: self.authorization.clone(),
        }
    }

    pub fn content(&self) -> Result<Vec<u8>, ResourceAccessStoreError> {
        self.authorization.validate()?;
        let bytes = PublishAgentArtifact {
            name: self.metadata.name.clone(),
            media_type: self.metadata.media_type.clone(),
            content_base64: self.content_base64.clone(),
        }
        .content()
        .map_err(|_| ResourceAccessStoreError::Corrupt)?;
        if !valid_artifact_id(&self.metadata.id)
            || self.metadata.agent_id != self.binding.agent_id
            || self.metadata.size_bytes != bytes.len() as u64
            || self.metadata.sha256 != digest(&bytes)
            || crate::seconds_from_rfc3339(&self.metadata.created_at).is_none()
            || self.authorization.target
                != (ResourceTarget::Agent {
                    agent_id: self.binding.agent_id.clone(),
                })
            || self.authorization.owner != self.binding.owner
            || self.authorization.sandbox_id != self.binding.sandbox_id
            || self.authorization.sandbox_created_at != self.binding.sandbox_created_at
            || self.authorization.capability != ResourceCapability::ManageSharing
            || self.authorization.captured_at != self.metadata.created_at
        {
            return Err(ResourceAccessStoreError::Corrupt);
        }
        Ok(bytes)
    }
}

#[derive(Debug, Clone)]
pub struct ArtifactPageRequest {
    pub limit: u16,
    pub cursor: Option<String>,
}
impl ArtifactPageRequest {
    pub fn validate(&self) -> Result<(), ResourceAccessStoreError> {
        if self.limit == 0
            || self.limit > MAX_ARTIFACT_PAGE
            || self
                .cursor
                .as_ref()
                .is_some_and(|cursor| !valid_artifact_id(cursor))
        {
            return Err(ResourceAccessStoreError::Invalid);
        }
        Ok(())
    }
}

pub fn valid_artifact_id(id: &str) -> bool {
    id.strip_prefix("art_").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}
pub(super) fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[async_trait::async_trait]
pub trait AgentArtifactStore: Send + Sync {
    async fn agent_artifact_metadata(
        &self,
        binding: &ArtifactBinding,
        id: &str,
    ) -> Result<Option<StoredAgentArtifactMetadata>, ResourceAccessStoreError> {
        Ok(self
            .agent_artifact(binding, id)
            .await?
            .map(|record| record.metadata_record()))
    }

    /// Verify the opaque live publication fence in the same transaction as the
    /// immutable insert and capacity check. Replays also require current access.
    async fn publish_artifact(
        &self,
        request: &super::ArtifactPublication,
    ) -> Result<StoredAgentArtifact, crate::resource_access::ResourceAccessError>;
    /// Ordered limit+1 metadata records for one retained Agent binding.
    /// Project out content at storage: list requests must never fetch or decode it.
    async fn agent_artifacts(
        &self,
        binding: &ArtifactBinding,
        page: &ArtifactPageRequest,
    ) -> Result<Vec<StoredAgentArtifactMetadata>, ResourceAccessStoreError>;
    async fn agent_artifact(
        &self,
        binding: &ArtifactBinding,
        id: &str,
    ) -> Result<Option<StoredAgentArtifact>, ResourceAccessStoreError>;
}

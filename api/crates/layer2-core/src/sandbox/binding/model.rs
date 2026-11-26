use super::{BindingFieldRefusal, BindingProfilePlan};
use crate::{seconds_from_rfc3339, AgentId, SandboxId, SubjectId, WorkspaceId, WorktreeId};
use serde::{Deserialize, Serialize};

pub const BINDING_CONFORMANCE_LEVEL: u8 = 2;
pub const EXISTING_AGENT_CONVERSATION: &str = "existing_agent";

/// Borrowed hierarchy is validated by CMA; channel retirement only revokes this
/// association and never owns the existing Agent's resource lifecycle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExistingBindingAgent {
    pub agent_id: AgentId,
    pub worktree_id: WorktreeId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum BindingState {
    Provisioning,
    Ready,
    Failed { reason: String },
    Retired,
}

/// Durable work remains discoverable after a control-plane restart. Credential
/// admission is completed before acknowledging the initial caller request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BindingWork {
    AwaitingCredential,
    DeliverCredential,
    RetirePreviousConversations,
    RetirePreviousSandbox,
    ProvisionSandbox,
    ProvisionWorkspace,
    RetireCurrentConversations,
    RetireCurrentSandbox,
    Idle,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreviousBindingEnvironment {
    pub generation: u64,
    pub sandbox_id: SandboxId,
    pub workspace_id: WorkspaceId,
    /// Only compatible immutable environment and credential snapshots can reuse
    /// the parent; old conversation Agents are retired in either case.
    pub retire_sandbox: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderBinding {
    pub binding_id: String,
    /// Promoted atomically with reset state; predecessors reject this strict record.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "reset_format"
    )]
    pub reset_format: Option<super::ConversationResetFormat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub existing_agent: Option<ExistingBindingAgent>,
    /// The management principal is independent of the resource owner.
    pub manager: SubjectId,
    pub owner: SubjectId,
    pub profile_digest: String,
    pub sandbox_id: SandboxId,
    pub workspace_id: WorkspaceId,
    pub state: BindingState,
    pub level: u8,
    pub honoured: Vec<String>,
    pub refused: Vec<BindingFieldRefusal>,
    pub profile: BindingProfilePlan,
    pub key_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_endpoint: Option<String>,
    pub generation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation_guard: Option<super::BindingGenerationGuard>,
    pub work: BindingWork,
    pub previous: Option<PreviousBindingEnvironment>,
    pub created_at: String,
    pub updated_at: String,
    pub revision: u64,
}

fn reset_format<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<super::ConversationResetFormat>, D::Error> {
    super::ConversationResetFormat::deserialize(deserializer).map(Some)
}

impl ProviderBinding {
    pub fn new(
        binding_id: String,
        manager: SubjectId,
        profile: BindingProfilePlan,
        key_sha256: String,
        at: String,
    ) -> Result<Self, String> {
        Self::new_with_tool_endpoint(binding_id, manager, profile, key_sha256, None, at)
    }

    pub fn new_with_tool_endpoint(
        binding_id: String,
        manager: SubjectId,
        profile: BindingProfilePlan,
        key_sha256: String,
        tool_endpoint: Option<String>,
        at: String,
    ) -> Result<Self, String> {
        let (state, work) = if profile.environment.is_some() {
            (BindingState::Provisioning, BindingWork::AwaitingCredential)
        } else {
            (
                BindingState::Failed {
                    reason: "environment is required".into(),
                },
                BindingWork::Idle,
            )
        };
        let result = Self {
            binding_id,
            reset_format: None,
            existing_agent: None,
            manager,
            owner: profile.owner.clone(),
            profile_digest: profile.profile_digest.clone(),
            sandbox_id: SandboxId::generate(),
            workspace_id: WorkspaceId::generate(),
            state,
            level: BINDING_CONFORMANCE_LEVEL,
            honoured: profile.honoured.clone(),
            refused: profile.refused.clone(),
            profile,
            key_sha256,
            tool_endpoint,
            generation: 1,
            generation_guard: None,
            work,
            previous: None,
            created_at: at.clone(),
            updated_at: at,
            revision: 1,
        };
        result.validate()?;
        Ok(result)
    }

    pub fn validate(&self) -> Result<(), String> {
        if !valid_id(&self.binding_id, 1)
            || self.manager.as_str().is_empty()
            || self.owner.as_str().is_empty()
            || self.owner != self.profile.owner
            || self.profile_digest != self.profile.profile_digest
            || !self
                .profile_digest
                .strip_prefix("sha256:")
                .is_some_and(valid_sha256)
            || !valid_sha256(&self.key_sha256)
            || self
                .tool_endpoint
                .as_deref()
                .is_some_and(|value| !super::valid_tool_endpoint(value))
            || self.revision == 0
            || self.generation == 0
            || self
                .generation_guard
                .as_ref()
                .is_some_and(|guard| !guard.valid(self.generation))
            || !(1..=BINDING_CONFORMANCE_LEVEL).contains(&self.level)
            || self.honoured != self.profile.honoured
            || !super::profile::valid_queue_lifetime(self.profile.queue_lifetime_seconds)
            || self
                .profile
                .configuration_digest
                .as_deref()
                .is_some_and(|digest| !digest.strip_prefix("sha256:").is_some_and(valid_sha256))
            || self.refused != self.profile.refused
            || self.sandbox_id.as_str().is_empty()
            || self.workspace_id.as_str().is_empty()
            || self.existing_agent.as_ref().is_some_and(|agent| {
                agent.agent_id.as_str().is_empty()
                    || agent.worktree_id.as_str().is_empty()
                    || self.previous.is_some()
                    || !matches!(self.state, BindingState::Ready | BindingState::Retired)
                    || !matches!(
                        self.work,
                        BindingWork::Idle | BindingWork::RetireCurrentConversations
                    )
                    || self.level != 1
            })
            || seconds_from_rfc3339(&self.created_at)
                .zip(seconds_from_rfc3339(&self.updated_at))
                .is_none_or(|(created, updated)| updated < created)
            || self
                .previous
                .as_ref()
                .is_some_and(|old| old.generation >= self.generation)
            || (self.state == BindingState::Ready
                && (self.work != BindingWork::Idle || self.previous.is_some()))
            || (self.state == BindingState::Provisioning
                && (self.work == BindingWork::Idle || self.profile.environment.is_none()))
        {
            return Err("invalid provider binding record".into());
        }
        Ok(())
    }

    pub fn credential_source(&self) -> crate::sandbox::SandboxCredentialSource {
        crate::sandbox::SandboxCredentialSource::Binding {
            binding_id: self.binding_id.clone(),
            tool_endpoint: self.tool_endpoint.clone(),
        }
    }

    /// Binding-owned runtime configuration is pinned. A PUT cannot replace the
    /// retained hierarchy or conversation to apply a changed configuration.
    pub fn retains_runtime_configuration(&self, profile: &BindingProfilePlan) -> bool {
        self.profile.environment == profile.environment
            && self.profile.agent == profile.agent
            && self.profile.session_affinity == profile.session_affinity
            && self.profile.owner == profile.owner
            && self.profile.organization_owned == profile.organization_owned
    }

    pub fn skill_version_update(
        &self,
        profile: &BindingProfilePlan,
    ) -> Option<(String, String, String)> {
        if self.profile.environment != profile.environment
            || self.profile.session_affinity != profile.session_affinity
            || self.profile.owner != profile.owner
            || self.profile.organization_owned != profile.organization_owned
            || self.profile.agent.isolated_agent != profile.agent.isolated_agent
            || self.profile.agent.model != profile.agent.model
            || self.profile.agent.thinking_effort != profile.agent.thinking_effort
            || self.profile.agent.permission_level != profile.agent.permission_level
            || self.profile.agent.system_prompt != profile.agent.system_prompt
            || self.profile.agent.skills.len() != profile.agent.skills.len()
        {
            return None;
        }
        if !self
            .profile
            .agent
            .skills
            .iter()
            .zip(&profile.agent.skills)
            .all(|(current, requested)| {
                current.source == requested.source && current.id == requested.id
            })
        {
            return None;
        }
        self.profile
            .agent
            .skills
            .iter()
            .zip(&profile.agent.skills)
            .find_map(|(current, requested)| {
                (current.version != requested.version).then(|| {
                    (
                        current.id.as_str().into(),
                        current.version.clone().unwrap_or_default(),
                        requested.version.clone().unwrap_or_default(),
                    )
                })
            })
    }

    /// Metadata and input policy updates retain the generation. Runtime configuration
    /// updates require a separately proved continuity-preserving operation.
    pub fn reconfigure(
        &self,
        profile: BindingProfilePlan,
        key_sha256: String,
        at: String,
    ) -> Result<Self, String> {
        if (self.existing_agent.is_some()
            && (self.profile.configuration_digest() != profile.configuration_digest()
                || (self.profile.queue_lifetime_seconds == profile.queue_lifetime_seconds
                    && self.profile.verified_sender_context_enabled()
                        == profile.verified_sender_context_enabled())))
            || self.state != BindingState::Ready
            || self.work != BindingWork::Idle
            || self.previous.is_some()
            || !self.retains_runtime_configuration(&profile)
            || self.key_sha256 != key_sha256
        {
            return Err("binding update cannot preserve its retained runtime configuration".into());
        }
        let mut next = self.clone();
        next.profile_digest = profile.profile_digest.clone();
        next.honoured = profile.honoured.clone();
        next.refused = profile.refused.clone();
        next.profile = profile;
        next.advance_revision(at)?;
        Ok(next)
    }

    pub fn advance_revision(&mut self, at: String) -> Result<(), String> {
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or("binding revision exhausted")?;
        self.updated_at = at;
        self.validate()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BindingConversationState {
    Provisioning,
    Ready,
    Failed,
    Retiring,
    Retired,
}

/// A generation-scoped external identifier has exactly one durable Agent/Worktree
/// pair, even when admission is retried after an unknown store result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindingConversation {
    pub binding_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_transfer: Option<Box<super::ProfileTransferReceipt>>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub borrowed: bool,
    pub generation: u64,
    pub conversation_id: String,
    pub owner: SubjectId,
    pub sandbox_id: SandboxId,
    pub workspace_id: WorkspaceId,
    pub agent_id: AgentId,
    pub worktree_id: WorktreeId,
    pub state: BindingConversationState,
    pub created_at: String,
    pub updated_at: String,
    pub revision: u64,
}

impl BindingConversation {
    pub fn new(
        binding: &ProviderBinding,
        conversation_id: String,
        at: String,
    ) -> Result<Self, String> {
        if binding.state != BindingState::Ready {
            return Err("binding is not ready".into());
        }
        let result = Self {
            binding_id: binding.binding_id.clone(),
            profile_transfer: None,
            borrowed: binding.existing_agent.is_some(),
            generation: binding.generation,
            conversation_id: if binding.existing_agent.is_some() {
                EXISTING_AGENT_CONVERSATION.into()
            } else {
                conversation_id
            },
            owner: binding.owner.clone(),
            sandbox_id: binding.sandbox_id.clone(),
            workspace_id: binding.workspace_id.clone(),
            agent_id: binding
                .existing_agent
                .as_ref()
                .map_or_else(AgentId::generate, |a| a.agent_id.clone()),
            worktree_id: binding
                .existing_agent
                .as_ref()
                .map_or_else(WorktreeId::generate, |a| a.worktree_id.clone()),
            state: if binding.existing_agent.is_some() {
                BindingConversationState::Ready
            } else {
                BindingConversationState::Provisioning
            },
            created_at: at.clone(),
            updated_at: at,
            revision: 1,
        };
        result.validate()?;
        Ok(result)
    }

    pub fn validate(&self) -> Result<(), String> {
        if !valid_id(&self.binding_id, 1)
            || !valid_id(&self.conversation_id, 8)
            || self.generation == 0
            || self.revision == 0
            || self.owner.as_str().is_empty()
            || self.sandbox_id.as_str().is_empty()
            || self.workspace_id.as_str().is_empty()
            || self.agent_id.as_str().is_empty()
            || self.worktree_id.as_str().is_empty()
            || seconds_from_rfc3339(&self.created_at)
                .zip(seconds_from_rfc3339(&self.updated_at))
                .is_none_or(|(created, updated)| updated < created)
        {
            return Err("invalid binding conversation record".into());
        }
        if let Some(transfer) = &self.profile_transfer {
            transfer.validate_source(self)?;
        }
        Ok(())
    }
}

pub fn valid_id(value: &str, minimum: usize) -> bool {
    (minimum..=128).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

impl ProviderBinding {
    /// Owner kind is part of retained binding authority, independently of the
    /// subject string carried by a binding credential.
    pub fn resource_owner(&self) -> crate::resource_access::ResourceOwner {
        use crate::resource_access::ResourceOwner;
        if self.profile.organization_owned {
            ResourceOwner::Organization {
                subject: self.owner.clone(),
            }
        } else {
            ResourceOwner::Person {
                subject: self.owner.clone(),
            }
        }
    }

    /// The authenticated binding manager attests the owner kind after verifying
    /// the creator with NyxID. Persist this evidence with new sandbox admission.
    pub fn sandbox_resource_policy(
        &self,
        sandbox: &crate::sandbox::Sandbox,
    ) -> Result<
        Option<crate::resource_access::ResourcePolicyRecord>,
        crate::sandbox::store::SandboxStoreError,
    > {
        use crate::resource_access::{
            OrganizationOwnershipEvidence, ResourceActor, ResourcePolicyRecord,
        };
        if self.sandbox_id != sandbox.id || self.owner != sandbox.owner {
            return Err(crate::sandbox::store::SandboxStoreError::Conflict);
        }
        if !self.profile.organization_owned {
            return Ok(None);
        }
        let policy = ResourcePolicyRecord {
            sandbox_id: sandbox.id.clone(),
            sandbox_created_at: sandbox.created_at.clone(),
            owner: self.resource_owner(),
            organization_evidence: Some(OrganizationOwnershipEvidence {
                issuer: "cma:authenticated-provider-binding-manager".into(),
                organization: self.owner.clone(),
                admitted_by: ResourceActor::Api {
                    subject: self.manager.clone(),
                    delegation_id: None,
                },
                verified_at: self.created_at.clone(),
            }),
            revision: 1,
        };
        policy.validate().map_err(|_| {
            crate::sandbox::store::SandboxStoreError::Corrupt(
                "invalid binding ownership evidence".into(),
            )
        })?;
        Ok(Some(policy))
    }
}

use serde::{Deserialize, Serialize};

use crate::bridge::{BridgeConfig, BridgeManagementLease, BridgePlacementPolicy};
use crate::profiles::{
    content_digest, AgentProfileSnapshot, ProfileCapacityBounds, SandboxProfileSnapshot,
    WorkspaceProfileSnapshot,
};
use crate::resource_access::{ResourceActor, ResourceOwner};

use super::AllocationError;

/// Exact selected clean templates. Child-profile lists are pinned for ancestry
/// validation; provisioning must create only the selected Workspace and Agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AllocationProfiles {
    pub sandbox: SandboxProfileSnapshot,
    pub workspace: WorkspaceProfileSnapshot,
    pub agent: AgentProfileSnapshot,
    pub trust_digest: String,
    pub approved_by: ResourceActor,
}

impl AllocationProfiles {
    pub fn digest(
        sandbox: &SandboxProfileSnapshot,
        workspace: &WorkspaceProfileSnapshot,
        agent: &AgentProfileSnapshot,
    ) -> Result<String, AllocationError> {
        super::hash(&("cma.bridge.selected-profiles.v1", sandbox, workspace, agent))
    }

    pub fn validate(&self, config: &BridgeConfig) -> Result<(), AllocationError> {
        let BridgePlacementPolicy::NewSandbox {
            sandbox_profile,
            workspace_profile,
            agent_profile,
        } = &config.placement
        else {
            return Err(AllocationError::InvalidProfiles);
        };
        if self.sandbox.id != sandbox_profile.id
            || self.sandbox.revision != sandbox_profile.revision
            || self.workspace.id != workspace_profile.id
            || self.workspace.revision != workspace_profile.revision
            || self.agent.id != agent_profile.id
            || self.agent.revision != agent_profile.revision
            || self.sandbox.digest
                != content_digest(&self.sandbox.content)
                    .map_err(|_| AllocationError::InvalidProfiles)?
            || self.workspace.digest
                != content_digest(&self.workspace.content)
                    .map_err(|_| AllocationError::InvalidProfiles)?
            || self.agent.digest
                != content_digest(&self.agent.content)
                    .map_err(|_| AllocationError::InvalidProfiles)?
            || self.trust_digest != Self::digest(&self.sandbox, &self.workspace, &self.agent)?
            || !matches!(
                &self.approved_by,
                ResourceActor::Human { .. }
                    | ResourceActor::Api {
                        delegation_id: None,
                        ..
                    }
            )
            || self
                .sandbox
                .content
                .workspace_profiles
                .iter()
                .any(|p| p.id == self.workspace.id && p.revision != self.workspace.revision)
            || self
                .workspace
                .content
                .agent_profiles
                .iter()
                .any(|p| p.id == self.agent.id && p.revision != self.agent.revision)
            || self.sandbox.content.max_workspaces == 0
            || self.sandbox.content.max_agents_per_workspace == 0
        {
            return Err(AllocationError::InvalidProfiles);
        }
        self.workspace
            .content
            .validate()
            .map_err(|_| AllocationError::InvalidProfiles)?;
        self.agent
            .content
            .validate()
            .map_err(|_| AllocationError::InvalidProfiles)?;
        if serde_json::to_vec(self)
            .map_err(|_| AllocationError::InvalidProfiles)?
            .len()
            > 256 * 1024
        {
            return Err(AllocationError::InvalidProfiles);
        }
        Ok(())
    }
}

/// Trusted adapter facts, never callback/API input. Full quota, service, model,
/// skill and runtime checks apply to the explicit one-Sandbox/Workspace/Agent
/// plan; nested profile defaults do not add hidden resources.
pub struct AllocationCapabilityObservation {
    pub owner: ResourceOwner,
    pub profiles_digest: String,
    pub capacity_bounds: ProfileCapacityBounds,
    pub one_sandbox_workspace_worktree_agent_available: bool,
    pub runtime_model_services_skills_available: bool,
    pub deadline: tokio::time::Instant,
}

/// Actual management consent plus live plan validation. It cannot be rebuilt
/// from a deserialized snapshot or a callback sender. This foundation exposes no
/// provider implementation; automatic renewable creation remains an adapter job.
#[derive(Clone)]
pub struct AllocationProfilePlan {
    pub(super) profiles: AllocationProfiles,
    pub(super) bridge: crate::bridge::BridgeRecord,
    pub(super) pin: crate::bridge::BridgeWorkPin,
    pub(super) deadline: tokio::time::Instant,
}

impl AllocationProfilePlan {
    pub fn prepare(
        management: &BridgeManagementLease,
        sandbox: SandboxProfileSnapshot,
        workspace: WorkspaceProfileSnapshot,
        agent: AgentProfileSnapshot,
        accepted_digest: &str,
        observation: AllocationCapabilityObservation,
    ) -> Result<Self, AllocationError> {
        let pin = crate::bridge::BridgeWorkPin {
            bridge_id: management.record().id.clone(),
            config_revision: management.record().config_revision,
        };
        Self::prepare_for_generation(
            management,
            pin,
            sandbox,
            workspace,
            agent,
            accepted_digest,
            observation,
        )
    }

    /// Renew authority for the original accepted generation. The current
    /// management snapshot and live capabilities are rechecked; the original
    /// templates and accepted digest cannot migrate to a newer configuration.
    pub fn prepare_for_generation(
        management: &BridgeManagementLease,
        pin: crate::bridge::BridgeWorkPin,
        sandbox: SandboxProfileSnapshot,
        workspace: WorkspaceProfileSnapshot,
        agent: AgentProfileSnapshot,
        accepted_digest: &str,
        observation: AllocationCapabilityObservation,
    ) -> Result<Self, AllocationError> {
        management
            .require_current()
            .map_err(|_| AllocationError::AuthorityUnavailable)?;
        let bridge = management.record();
        super::authority::require_accepted_generation(bridge, &pin)?;
        let profiles = AllocationProfiles {
            sandbox,
            workspace,
            agent,
            trust_digest: accepted_digest.to_owned(),
            approved_by: management.actor().clone(),
        };
        profiles.validate(
            &bridge
                .pinned(&pin)
                .map_err(|_| AllocationError::IdentityMismatch)?
                .config,
        )?;
        profiles
            .sandbox
            .content
            .validate(observation.capacity_bounds)
            .map_err(|_| AllocationError::InvalidProfiles)?;
        if observation.owner != bridge.owner
            || observation.profiles_digest != profiles.trust_digest
            || !observation.one_sandbox_workspace_worktree_agent_available
            || !observation.runtime_model_services_skills_available
        {
            return Err(AllocationError::InvalidProfiles);
        }
        let deadline = observation.deadline.min(management.deadline());
        if tokio::time::Instant::now() >= deadline {
            return Err(AllocationError::AuthorityUnavailable);
        }
        Ok(Self {
            profiles,
            bridge: bridge.clone(),
            pin,
            deadline,
        })
    }
    pub fn profiles(&self) -> &AllocationProfiles {
        &self.profiles
    }
}

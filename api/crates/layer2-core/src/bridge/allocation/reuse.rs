//! Shared-parent placement proves current owner authority and immutable parent
//! compatibility. It never changes parent credentials, profiles or lifecycle.
use super::{
    AllocationError, AllocationIdentity, AllocationPlacement, AllocationProfiles, AllocationRecord,
};
use crate::bridge::{
    BridgeConfig, BridgeManagementLease, BridgePlacementPolicy, BridgeRecord, BridgeValidation,
    BridgeWorkPin,
};
use crate::resource_access::{
    AccessDecision, AuthorizedResource, ResourceAccessSnapshot, ResourceCapability,
};
use crate::sandbox::{SandboxLimits, SandboxStatus};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AllocationParentIdentity {
    pub sandbox_created_at: String,
    pub workspace_created_at: Option<String>,
    pub limits: SandboxLimits,
    pub service_scope: Vec<crate::ServiceId>,
}

impl AllocationParentIdentity {
    pub fn require(
        &self,
        config: &BridgeConfig,
        profiles: &AllocationProfiles,
        resources: &ResourceAccessSnapshot,
    ) -> Result<(), AllocationError> {
        let sandbox = &resources.lineage.sandbox;
        sandbox
            .validate()
            .map_err(|_| AllocationError::IncompatibleTarget)?;
        let (sandbox_id, workspace_id) = match &config.placement {
            BridgePlacementPolicy::WorkspacePerPartition { sandbox_id, .. } => (sandbox_id, None),
            BridgePlacementPolicy::WorktreeAgentPerPartition {
                sandbox_id,
                workspace_id,
                ..
            } => (sandbox_id, Some(workspace_id)),
            _ => return Err(AllocationError::IncompatibleTarget),
        };
        if &sandbox.id != sandbox_id
            || sandbox.created_at != self.sandbox_created_at
            || sandbox.profile != profiles.sandbox
            || sandbox.limits != self.limits
            || sandbox.credential.effective_reaches() != self.service_scope
            || (workspace_id.is_none() && !profiles.workspace.content.agent_profiles.is_empty())
            || sandbox.retired_at.is_some()
            || matches!(
                sandbox.status,
                SandboxStatus::Retiring | SandboxStatus::Retired
            )
        {
            return Err(AllocationError::IncompatibleTarget);
        }
        match (workspace_id, &self.workspace_created_at) {
            (None, None) => {}
            (Some(id), Some(created_at)) => {
                let workspace = resources
                    .lineage
                    .workspace
                    .as_ref()
                    .ok_or(AllocationError::IncompatibleTarget)?;
                if &workspace.id != id
                    || &workspace.created_at != created_at
                    || workspace.sandbox_id != sandbox.id
                    || workspace.owner != sandbox.owner
                    || workspace.profile != profiles.workspace
                    || workspace.removed_at.is_some()
                {
                    return Err(AllocationError::IncompatibleTarget);
                }
            }
            _ => return Err(AllocationError::IncompatibleTarget),
        }
        let reserve = config
            .placement
            .shared_capacity()
            .ok_or(AllocationError::IncompatibleTarget)?;
        if self.limits.max_agents_per_workspace <= reserve.keep_other_agents_per_workspace
            || (workspace_id.is_none()
                && self.limits.max_workspaces <= reserve.keep_other_workspaces)
        {
            return Err(AllocationError::IncompatibleTarget);
        }
        Ok(())
    }
}

/// Verified current runtime/model/skill compatibility. Parent store counts are
/// fenced during admission; a caller cannot supply this observation as JSON.
pub struct AllocationReuseObservation {
    pub ordinary_parent_lifecycle_available: bool,
    pub profiles_digest: String,
    pub runtime_model_services_skills_available: bool,
    pub workspace_writer_coordination_available: bool,
    pub deadline: tokio::time::Instant,
}

#[derive(Clone)]
pub struct AllocationReuseProof {
    pub(super) bridge: BridgeRecord,
    pub(super) pin: BridgeWorkPin,
    pub(super) allocation_id: String,
    pub(super) profiles: AllocationProfiles,
    pub(super) parent: AllocationParentIdentity,
    resources: ResourceAccessSnapshot,
    decision: AccessDecision,
    pub(super) connection: BridgeValidation,
    deadline: tokio::time::Instant,
}

impl AllocationReuseProof {
    pub fn prepare(
        management: &BridgeManagementLease,
        identity: &AllocationIdentity,
        pin: BridgeWorkPin,
        profiles: AllocationProfiles,
        authorized: AuthorizedResource,
        connection: BridgeValidation,
        observation: AllocationReuseObservation,
    ) -> Result<Self, AllocationError> {
        management
            .require_current()
            .map_err(|_| AllocationError::AuthorityUnavailable)?;
        let bridge = management.record();
        let config = &bridge
            .pinned(&pin)
            .map_err(|_| AllocationError::IdentityMismatch)?
            .config;
        let target = config
            .placement
            .reused_parent()
            .ok_or(AllocationError::IncompatibleTarget)?;
        let scope = authorized
            .snapshot
            .scope(&target)
            .map_err(|_| AllocationError::IncompatibleTarget)?;
        if !authorized
            .decision
            .allows(ResourceCapability::ManageSandbox)
            || scope.owner != bridge.owner
            || identity.bridge_id != bridge.id
            || profiles.approved_by != *management.actor()
            || profiles.trust_digest != observation.profiles_digest
            || !observation.ordinary_parent_lifecycle_available
            || !observation.runtime_model_services_skills_available
            || !observation.workspace_writer_coordination_available
        {
            return Err(AllocationError::AuthorityUnavailable);
        }
        profiles.validate(config)?;
        let sandbox = &authorized.snapshot.lineage.sandbox;
        let parent = AllocationParentIdentity {
            sandbox_created_at: sandbox.created_at.clone(),
            workspace_created_at: match config.placement {
                BridgePlacementPolicy::WorktreeAgentPerPartition { .. } => Some(
                    authorized
                        .snapshot
                        .lineage
                        .workspace
                        .as_ref()
                        .ok_or(AllocationError::IncompatibleTarget)?
                        .created_at
                        .clone(),
                ),
                _ => None,
            },
            limits: sandbox.limits,
            service_scope: sandbox.credential.effective_reaches(),
        };
        parent.require(config, &profiles, &authorized.snapshot)?;
        let proof = Self {
            bridge: bridge.clone(),
            pin,
            allocation_id: identity.id()?,
            profiles,
            parent,
            resources: authorized.snapshot,
            decision: authorized.decision,
            connection,
            deadline: observation.deadline.min(management.deadline()),
        };
        proof.require_current()?;
        Ok(proof)
    }

    pub fn renew_for_record(
        management: &BridgeManagementLease,
        record: &AllocationRecord,
        authorized: AuthorizedResource,
        connection: BridgeValidation,
        observation: AllocationReuseObservation,
    ) -> Result<Self, AllocationError> {
        record.validate()?;
        let AllocationPlacement::ReuseParent { profiles, .. } = &record.placement else {
            return Err(AllocationError::IdentityMismatch);
        };
        let mut approved = profiles.as_ref().clone();
        approved.approved_by = management.actor().clone();
        let mut proof = Self::prepare(
            management,
            &record.identity,
            record.pin.clone(),
            approved,
            authorized,
            connection,
            observation,
        )?;
        proof.profiles.approved_by = profiles.approved_by.clone();
        proof.require_record(record)?;
        Ok(proof)
    }

    pub(super) fn require_current(&self) -> Result<(), AllocationError> {
        if tokio::time::Instant::now() >= self.deadline
            || !self.decision.allows(ResourceCapability::ManageSandbox)
        {
            return Err(AllocationError::AuthorityUnavailable);
        }
        super::authority::require_accepted_generation(&self.bridge, &self.pin)?;
        let config = &self
            .bridge
            .pinned(&self.pin)
            .map_err(|_| AllocationError::IdentityMismatch)?
            .config;
        self.profiles.validate(config)?;
        self.parent
            .require(config, &self.profiles, &self.resources)?;
        self.connection
            .require(&self.bridge.owner, config)
            .map_err(|_| AllocationError::AuthorityUnavailable)
    }

    pub fn resources(&self) -> &ResourceAccessSnapshot {
        &self.resources
    }
    pub fn profiles(&self) -> &AllocationProfiles {
        &self.profiles
    }
    pub fn parent(&self) -> &AllocationParentIdentity {
        &self.parent
    }
    pub fn deadline(&self) -> tokio::time::Instant {
        self.deadline
    }

    pub(super) fn placement(&self, id: &str) -> Result<AllocationPlacement, AllocationError> {
        self.require_current()?;
        if id != self.allocation_id {
            return Err(AllocationError::IdentityMismatch);
        }
        let mut target = super::model::reserved_target(id)?;
        target.sandbox_id = self.resources.lineage.sandbox.id.clone();
        if self.parent.workspace_created_at.is_some() {
            target.workspace_id = self
                .resources
                .lineage
                .workspace
                .as_ref()
                .ok_or(AllocationError::IncompatibleTarget)?
                .id
                .clone();
        }
        Ok(AllocationPlacement::ReuseParent {
            target,
            profiles: Box::new(self.profiles.clone()),
            parent: self.parent.clone(),
        })
    }

    pub(super) fn require_record(&self, record: &AllocationRecord) -> Result<(), AllocationError> {
        if record.pin != self.pin || record.placement != self.placement(&record.id)? {
            return Err(AllocationError::IdentityMismatch);
        }
        Ok(())
    }
}

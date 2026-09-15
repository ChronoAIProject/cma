//! Durable child progress for a reused parent. It contains no Sandbox creation
//! sidecar and cannot authorize parent ownership, credentials or lifecycle.
use crate::sandbox::{
    channel_provisioning::state::{ChannelProvisioningPhase, ChannelProvisioningSettings},
    channel_startup::model::ChannelStartupPlan,
};
use layer2_core::{
    bridge::allocation::{
        AllocationAuthority, AllocationError, AllocationPlacement, AllocationRecord,
    },
    sandbox::*,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChannelReuseState {
    pub schema_version: u16,
    pub allocation_id: String,
    pub phase: ChannelProvisioningPhase,
    pub revision: u64,
    pub settings: ChannelProvisioningSettings,
    pub startup_plan: Option<ChannelStartupPlan>,
    pub last_receipt_digest: Option<String>,
}

impl ChannelReuseState {
    pub fn initial(
        record: &AllocationRecord,
        settings: ChannelProvisioningSettings,
    ) -> Result<Self, AllocationError> {
        let AllocationPlacement::ReuseParent { parent, .. } = &record.placement else {
            return Err(AllocationError::IdentityMismatch);
        };
        Ok(Self {
            schema_version: 1,
            allocation_id: record.id.clone(),
            phase: if parent.workspace_created_at.is_some() {
                ChannelProvisioningPhase::AgentAdmission
            } else {
                ChannelProvisioningPhase::WorkspaceAdmission
            },
            revision: 1,
            settings,
            startup_plan: None,
            last_receipt_digest: None,
        })
    }
    pub fn validate(&self, record: &AllocationRecord) -> Result<(), AllocationError> {
        use ChannelProvisioningPhase::*;
        self.settings.validate()?;
        if self.schema_version != 1
            || self.allocation_id != record.id
            || self.revision == 0
            || !matches!(
                self.phase,
                WorkspaceAdmission
                    | WorkspacePrepare
                    | WorkspaceSetup
                    | AgentAdmission
                    | AgentStart
                    | Ready
            )
            || self
                .startup_plan
                .as_ref()
                .is_some_and(|plan| plan.schema_version != 1 || plan.allocation_id != record.id)
            || (self.phase == Ready && self.startup_plan.is_none())
            || self.last_receipt_digest.as_ref().is_some_and(|digest| {
                digest.len() != 64
                    || !digest
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
        {
            return Err(AllocationError::Corrupt);
        }
        Ok(())
    }
    pub fn advance(
        &self,
        next: ChannelProvisioningPhase,
        receipt: &impl Serialize,
    ) -> Result<Self, AllocationError> {
        use ChannelProvisioningPhase::*;
        if !matches!(
            (self.phase, next),
            (WorkspaceAdmission, WorkspacePrepare)
                | (WorkspacePrepare, WorkspaceSetup)
                | (WorkspaceSetup, AgentAdmission)
                | (AgentAdmission, AgentStart)
                | (AgentStart, AgentStart | Ready)
        ) {
            return Err(AllocationError::InvalidTransition);
        }
        let mut result = self.clone();
        result.phase = next;
        result.revision = result
            .revision
            .checked_add(1)
            .ok_or(AllocationError::Corrupt)?;
        result.last_receipt_digest = Some(
            layer2_core::profiles::content_digest(receipt).map_err(|_| AllocationError::Corrupt)?,
        );
        Ok(result)
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ChannelReuseSnapshot {
    pub allocation: AllocationRecord,
    pub sandbox: Sandbox,
    pub state: ChannelReuseState,
    pub workspace: Option<Workspace>,
    pub worktree: Option<Worktree>,
    pub agent: Option<Agent>,
    pub creation: Option<AgentCreationProvenance>,
    pub workspace_creation: Option<super::children::ReuseWorkspaceCreation>,
}

impl ChannelReuseSnapshot {
    pub fn require(&self, authority: &AllocationAuthority) -> Result<(), AllocationError> {
        self.allocation.validate()?;
        authority.require_record(&self.allocation)?;
        self.state.validate(&self.allocation)?;
        let AllocationAuthority::Reuse(proof) = authority else {
            return Err(AllocationError::IdentityMismatch);
        };
        if self.sandbox != proof.resources().lineage.sandbox {
            return Err(AllocationError::Conflict);
        }
        let target = self.allocation.placement.target();
        if let Some(workspace) = &self.workspace {
            if proof.parent().workspace_created_at.is_none() {
                self.workspace_creation
                    .as_ref()
                    .ok_or(AllocationError::Corrupt)?
                    .require(self, workspace)?;
            } else if self.workspace_creation.is_some() {
                return Err(AllocationError::Corrupt);
            }
        } else if self.workspace_creation.is_some() {
            return Err(AllocationError::Corrupt);
        }
        if let Some(workspace) = &self.workspace {
            if workspace.id != target.workspace_id
                || workspace.sandbox_id != self.sandbox.id
                || workspace.owner != self.sandbox.owner
                || workspace.profile != proof.profiles().workspace
                || workspace.removed_at.is_some()
            {
                return Err(AllocationError::IdentityMismatch);
            }
        }
        if let Some(agent) = &self.agent {
            let workspace = self.workspace.as_ref().ok_or(AllocationError::Corrupt)?;
            let worktree = self.worktree.as_ref().ok_or(AllocationError::Corrupt)?;
            let creation = self.creation.as_ref().ok_or(AllocationError::Corrupt)?;
            worktree
                .validate_prerequisites(workspace)
                .map_err(|_| AllocationError::Corrupt)?;
            creation
                .validate_against(agent, &self.sandbox)
                .map_err(|_| AllocationError::Corrupt)?;
            if creation.created_by.as_ref() != Some(&proof.profiles().approved_by)
                || agent.id != target.agent_id
                || worktree.id != target.worktree_id
                || agent.worktree_id != worktree.id
                || agent.workspace_id != workspace.id
                || agent.profile != proof.profiles().agent
                || agent.creation_key != format!("channel-allocation:{}", self.allocation.id)
                || agent.creation_fingerprint
                    != channel_fingerprint(&self.allocation)
                        .map_err(|_| AllocationError::Corrupt)?
                || !matches!(&creation.creation_origin,AgentCreationOrigin::ChannelBridge{bridge_id,partition_id,generation,source_reference}
                    if bridge_id==self.allocation.identity.bridge_id.as_str() && partition_id==self.allocation.identity.key.hash()
                        && *generation==self.allocation.identity.generation && source_reference==self.allocation.first_source.inbox_id())
            {
                return Err(AllocationError::IdentityMismatch);
            }
        } else if self.worktree.is_some() || self.creation.is_some() {
            return Err(AllocationError::Corrupt);
        }
        Ok(())
    }
}

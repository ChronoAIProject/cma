//! Immutable preparation inputs and the exact ownership transfer receipt.

use layer2_core::profiles::{
    content_digest, AgentProfileContent, AgentProfileRef, AgentProfileSnapshot, AgentProvider,
    ModelAccess, SandboxProfileContent, SandboxProfileSnapshot, WorkspaceLocation,
    WorkspaceProfileContent, WorkspaceProfileRef, WorkspaceProfileSnapshot,
};
use layer2_core::sandbox::{
    ProvisioningExecution, SandboxStoreError, WarmSandbox, WarmSandboxPlan, WarmSandboxState,
    WarmWorkspacePlan,
};
use layer2_core::{AgentProfileId, SandboxProfileId, ServiceId, SubjectId, WorkspaceProfileId};
use layer3_adapter::HomeVolumePolicy;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NativePreparedPlan {
    pub warm: WarmSandboxPlan,
    pub home_policy: HomeVolumePolicy,
}

impl NativePreparedPlan {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        mut sandbox: SandboxProfileContent,
        mut workspace: WorkspaceProfileContent,
        agent: AgentProfileContent,
        mut reaches: Vec<ServiceId>,
        runtime_version: String,
        execution: ProvisioningExecution,
        home_policy: HomeVolumePolicy,
    ) -> Result<Self, SandboxStoreError> {
        if !workspace.agent_profiles.is_empty() {
            return Err(corrupt(
                "owned inline workspace must have no Agent references",
            ));
        }
        let agent = AgentProfileSnapshot {
            id: AgentProfileId("agp_native_prepared_default".into()),
            revision: 1,
            digest: digest(&agent)?,
            content: agent,
        };
        workspace.agent_profiles = vec![AgentProfileRef::new(agent.id.clone(), agent.revision)];
        let workspace = WorkspaceProfileSnapshot {
            id: WorkspaceProfileId("wsp_native_prepared_default".into()),
            revision: 1,
            digest: digest(&workspace)?,
            content: workspace,
        };
        sandbox.workspace_profiles = vec![WorkspaceProfileRef::new(
            workspace.id.clone(),
            workspace.revision,
        )];
        let profile = SandboxProfileSnapshot {
            id: SandboxProfileId("sbp_native_prepared_default".into()),
            revision: 1,
            digest: digest(&sandbox)?,
            content: sandbox,
        };
        reaches.sort();
        reaches.dedup();
        let plan = Self {
            warm: WarmSandboxPlan {
                profile,
                workspaces: vec![WarmWorkspacePlan {
                    profile: workspace,
                    agents: vec![agent],
                }],
                reaches,
                runtime_version,
                execution,
            },
            home_policy,
        };
        plan.validate()?;
        Ok(plan)
    }

    pub(crate) fn warm_plan(&self) -> &WarmSandboxPlan {
        &self.warm
    }

    pub(crate) fn validate(&self) -> Result<(), SandboxStoreError> {
        self.warm.validate()?;
        self.home_policy
            .validate()
            .map_err(|e| corrupt(&e.to_string()))?;
        let [workspace] = self.warm.workspaces.as_slice() else {
            return Err(corrupt("requires one workspace"));
        };
        let [agent] = workspace.agents.as_slice() else {
            return Err(corrupt("requires one Agent"));
        };
        let sandbox = &self.warm.profile.content;
        if !self.home_policy.mounts_full_home()
            || sandbox.agent_provider != AgentProvider::Codex
            || sandbox.model_access != ModelAccess::ChronoaiManaged
            || !sandbox.software_install_commands.is_empty()
            || !sandbox.environment_variables.is_empty()
            || !workspace.profile.content.setup_commands.is_empty()
            || !matches!(&workspace.profile.content.location, WorkspaceLocation::Directory { name } if name == "workspace")
            || workspace.profile.content.name != "workspace"
            || !agent.content.skills.is_empty()
            || !crate::agent_lifecycle::guidance::is_prepared_prompt(&agent.content.system_prompt)
            || agent.content.isolated_agent
            || sandbox
                .runtime_image
                .as_ref()
                .is_some_and(|image| image != &self.warm.runtime_version)
        {
            return Err(corrupt("outside the credential-free managed default"));
        }
        Ok(())
    }

    pub(crate) fn matches(
        &self,
        sandbox: &SandboxProfileSnapshot,
        workspace: &WorkspaceProfileSnapshot,
        agent: &AgentProfileContent,
        reaches: &[ServiceId],
    ) -> bool {
        let mut actual_sandbox = sandbox.content.clone();
        let mut expected_sandbox = self.warm.profile.content.clone();
        let mut expected_workspace = self.warm.workspaces[0].profile.content.clone();
        expected_workspace.agent_profiles.clear();
        let mut actual_reaches = reaches.to_vec();
        actual_reaches.sort();
        actual_reaches.dedup();
        let exact_workspace = sandbox.content.workspace_profiles.as_slice()
            == [WorkspaceProfileRef::new(
                workspace.id.clone(),
                workspace.revision,
            )];
        actual_sandbox.workspace_profiles.clear();
        expected_sandbox.workspace_profiles.clear();
        exact_workspace
            && actual_sandbox == expected_sandbox
            && workspace.content == expected_workspace
            && *agent == self.warm.workspaces[0].agents[0].content
            && actual_reaches == self.warm.reaches
            && digest(&sandbox.content).ok().as_ref() == Some(&sandbox.digest)
            && digest(&workspace.content).ok().as_ref() == Some(&workspace.digest)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Ownership {
    pub owner: SubjectId,
    pub operation_id: String,
    pub sandbox: SandboxProfileSnapshot,
    pub workspace: WorkspaceProfileSnapshot,
    pub agent_content: AgentProfileContent,
    pub agent: Option<AgentProfileSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Record {
    pub plan: NativePreparedPlan,
    pub payload: WarmSandbox,
    pub ownership: Option<Ownership>,
}

impl Record {
    pub(super) fn validate(&self) -> Result<(), SandboxStoreError> {
        self.plan.validate()?;
        self.payload.validate()?;
        if self.payload.plan != self.plan.warm
            || (matches!(
                self.payload.state,
                WarmSandboxState::Installing | WarmSandboxState::Ready | WarmSandboxState::Claimed
            ) && (self.payload.home.is_none()
                || self
                    .payload
                    .runtime
                    .as_ref()
                    .is_none_or(|runtime| runtime.boot_id.is_none())))
            || (self.payload.state == WarmSandboxState::Claimed) != self.ownership.is_some()
            || self.ownership.as_ref().is_some_and(|owned| {
                owned.owner.as_str().is_empty()
                    || owned.operation_id.is_empty()
                    || !self.plan.matches(
                        &owned.sandbox,
                        &owned.workspace,
                        &owned.agent_content,
                        &self.plan.warm.reaches,
                    )
                    || owned.agent.as_ref().is_some_and(|agent| {
                        agent.content != owned.agent_content
                            || digest(&agent.content).ok().as_ref() != Some(&agent.digest)
                    })
            })
        {
            return Err(corrupt("immutable plan or ownership receipt changed"));
        }
        if let Some(home) = &self.payload.home {
            let native = home
                .native_pool
                .as_ref()
                .ok_or_else(|| corrupt("native home has no Claim authority"))?;
            let spec = &native.request.pool.spec;
            if spec.image.uri != self.plan.warm.runtime_version
                || spec.cpu_millis != self.plan.warm.profile.content.cpu_millis
                || spec.memory_mib != self.plan.warm.profile.content.memory_mib
                || spec.home != self.plan.home_policy
            {
                return Err(corrupt(
                    "native Claim differs from pinned preparation inputs",
                ));
            }
        }
        Ok(())
    }
}

pub(super) fn digest<T: Serialize>(value: &T) -> Result<String, SandboxStoreError> {
    content_digest(value).map_err(|e| corrupt(&e.to_string()))
}
pub(super) fn corrupt(message: &str) -> SandboxStoreError {
    SandboxStoreError::Corrupt(format!("native prepared: {message}"))
}

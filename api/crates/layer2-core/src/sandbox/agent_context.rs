//! Resolve an owned conversation context from pinned deployment defaults.

use std::{sync::Arc, time::Duration};

use crate::profiles::AgentProfileStore;
use crate::{
    AgentId, AgentProfileId, Caller, Capability, PlatformDefaults, PlatformSettingsService,
    SandboxId, WorkspaceId,
};

use super::{
    Agent, AgentOrigin, AgentService, AgentServiceError, AgentStatus, CreateAgent, Sandbox,
    SandboxLifecycleCommandError, SandboxService, SandboxServiceError, SandboxStatus, Workspace,
    WorkspaceService, WorkspaceServiceError, WorkspaceStatus,
};

mod selection;

const CONTEXT_POLL_INTERVAL: Duration = Duration::from_millis(250);
pub const AGENT_CONTEXT_ADMISSION_BOUND: Duration = Duration::from_secs(10 * 60);

#[derive(Clone, Copy)]
enum CheckpointReadiness {
    Wait,
    Refuse,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentContextOverrides {
    pub sandbox_id: Option<SandboxId>,
    pub workspace_id: Option<WorkspaceId>,
    pub agent_profile_id: Option<AgentProfileId>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AgentContextError {
    #[error("forbidden")]
    Forbidden,
    #[error("an existing sandbox is required")]
    SandboxRequired,
    #[error("context override is not available to this caller")]
    InvalidOverride,
    #[error("context did not become ready within the admission bound")]
    AdmissionTimeout,
    #[error("context is not ready")]
    NotReady,
    #[error("sandbox checkpoint is in progress; input was not admitted")]
    CheckpointInProgress,
    #[error("context dependency is unavailable")]
    Unavailable,
    #[error(transparent)]
    Sandbox(#[from] SandboxServiceError),
    #[error(transparent)]
    Workspace(#[from] WorkspaceServiceError),
    #[error(transparent)]
    Agent(#[from] AgentServiceError),
    #[error(transparent)]
    Lifecycle(#[from] SandboxLifecycleCommandError),
}

#[async_trait::async_trait]
pub trait AgentContextDefaults: Send + Sync {
    async fn defaults(&self) -> Result<PlatformDefaults, AgentContextError>;
}

#[async_trait::async_trait]
impl AgentContextDefaults for PlatformDefaults {
    async fn defaults(&self) -> Result<PlatformDefaults, AgentContextError> {
        Ok(self.clone())
    }
}

#[async_trait::async_trait]
impl AgentContextDefaults for PlatformSettingsService {
    async fn defaults(&self) -> Result<PlatformDefaults, AgentContextError> {
        self.current()
            .await
            .map(|stored| stored.settings.defaults)
            .map_err(|_| AgentContextError::Unavailable)
    }
}

pub struct AgentContextService {
    sandboxes: Arc<SandboxService>,
    workspaces: Arc<WorkspaceService>,
    agents: Arc<AgentService>,
    profiles: Arc<dyn AgentProfileStore>,
    defaults: Arc<dyn AgentContextDefaults>,
}

impl AgentContextService {
    /// One bounded preparation step. The command worker observes lifecycle
    /// progress on its next sweep without retaining renewable authority here.
    pub async fn prepare_command_input(
        &self,
        authorization: Arc<super::agent_io::CommandRevivalAuthorization>,
    ) -> Result<(), AgentContextError> {
        use super::store::SandboxLifecycleCommandResult;
        match self.sandboxes.revive_for_command(authorization).await? {
            SandboxLifecycleCommandResult::Accepted(_) => Ok(()),
            SandboxLifecycleCommandResult::NotEligible(sandbox) => {
                if matches!(
                    sandbox.status,
                    SandboxStatus::Failed | SandboxStatus::Retiring | SandboxStatus::Retired
                ) {
                    return Err(AgentContextError::NotReady);
                }
                let reason = sandbox.revive_eligibility().err().ok_or_else(|| {
                    SandboxLifecycleCommandError::Corrupt(
                        "eligible sandbox could not schedule revival".into(),
                    )
                })?;
                Err(SandboxLifecycleCommandError::SandboxBusy {
                    status: sandbox.status,
                    reason,
                }
                .into())
            }
        }
    }

    pub fn new(
        sandboxes: Arc<SandboxService>,
        workspaces: Arc<WorkspaceService>,
        agents: Arc<AgentService>,
        profiles: Arc<dyn AgentProfileStore>,
        defaults: Arc<dyn AgentContextDefaults>,
    ) -> Arc<Self> {
        Arc::new(Self {
            sandboxes,
            workspaces,
            agents,
            profiles,
            defaults,
        })
    }

    pub async fn create(
        &self,
        caller: &Caller,
        overrides: AgentContextOverrides,
    ) -> Result<Agent, AgentContextError> {
        require(caller)?;
        tokio::time::timeout(
            AGENT_CONTEXT_ADMISSION_BOUND,
            self.create_in_context(caller, overrides),
        )
        .await
        .map_err(|_| AgentContextError::AdmissionTimeout)?
    }

    async fn create_in_context(
        &self,
        caller: &Caller,
        overrides: AgentContextOverrides,
    ) -> Result<Agent, AgentContextError> {
        let defaults = self.defaults.defaults().await?;
        let agent_profile = self
            .resolve_agent_profile(caller, overrides.agent_profile_id, &defaults)
            .await?;
        let explicit_workspace = match overrides.workspace_id {
            Some(id) => Some(self.workspaces.read(caller, &id).await?),
            None => None,
        };
        if explicit_workspace
            .as_ref()
            .zip(overrides.sandbox_id.as_ref())
            .is_some_and(|(workspace, sandbox)| workspace.sandbox_id != *sandbox)
        {
            return Err(AgentContextError::InvalidOverride);
        }
        let sandbox_id = overrides.sandbox_id.or_else(|| {
            explicit_workspace
                .as_ref()
                .map(|workspace| workspace.sandbox_id.clone())
        });
        let sandbox = match sandbox_id {
            Some(id) => self.sandboxes.read(caller, &id).await?,
            None => self.default_sandbox(caller, &defaults).await?,
        };
        let sandbox = self
            .wait_for_sandbox(caller, sandbox, CheckpointReadiness::Wait)
            .await?;
        let workspace = match explicit_workspace {
            Some(workspace) => workspace,
            None => self.default_workspace(caller, &sandbox, &defaults).await?,
        };
        let workspace = self.wait_for_workspace(caller, workspace).await?;
        Ok(self
            .agents
            .create(
                caller,
                &workspace.id,
                CreateAgent {
                    profile: agent_profile,
                    title: None,
                    first_message: None,
                    origin: AgentOrigin::Manual,
                },
            )
            .await?)
    }

    /// A paused conversation is revived before a linear input can be admitted.
    pub async fn await_input_ready(
        &self,
        caller: &Caller,
        id: &AgentId,
    ) -> Result<Agent, AgentContextError> {
        require(caller)?;
        tokio::time::timeout(AGENT_CONTEXT_ADMISSION_BOUND, async {
            loop {
                let agent = self.agents.read(caller, id).await?;
                let sandbox = self.sandboxes.read(caller, &agent.sandbox_id).await?;
                if sandbox.lifecycle.checkpoint_capture.is_some() {
                    return Err(AgentContextError::CheckpointInProgress);
                }
                match agent.status {
                    AgentStatus::Ready => {
                        self.wait_for_sandbox(caller, sandbox, CheckpointReadiness::Refuse)
                            .await?;
                        return Ok(agent);
                    }
                    AgentStatus::Paused => {
                        self.wait_for_sandbox(caller, sandbox, CheckpointReadiness::Refuse)
                            .await?;
                    }
                    AgentStatus::Starting => {}
                    AgentStatus::Working | AgentStatus::WaitingApproval => return Ok(agent),
                    AgentStatus::Stopped | AgentStatus::Deleted | AgentStatus::Failed => {
                        return Err(AgentContextError::NotReady)
                    }
                }
                tokio::time::sleep(CONTEXT_POLL_INTERVAL).await;
            }
        })
        .await
        .map_err(|_| AgentContextError::AdmissionTimeout)?
    }

    async fn wait_for_sandbox(
        &self,
        caller: &Caller,
        mut sandbox: Sandbox,
        checkpoint: CheckpointReadiness,
    ) -> Result<Sandbox, AgentContextError> {
        self.sandboxes
            .require_connections(caller, &sandbox.id)
            .await?;
        loop {
            if matches!(checkpoint, CheckpointReadiness::Refuse)
                && sandbox.lifecycle.checkpoint_capture.is_some()
            {
                return Err(AgentContextError::CheckpointInProgress);
            }
            match sandbox.status {
                SandboxStatus::Active if sandbox.lifecycle.checkpoint_capture.is_none() => {
                    return Ok(sandbox)
                }
                SandboxStatus::Paused => match self.sandboxes.revive(caller, &sandbox.id).await {
                    Ok(_) | Err(SandboxLifecycleCommandError::SandboxNotPaused { .. }) => {}
                    Err(error) => return Err(error.into()),
                },
                SandboxStatus::Failed | SandboxStatus::Retiring | SandboxStatus::Retired => {
                    return Err(AgentContextError::NotReady)
                }
                _ => {}
            }
            tokio::time::sleep(CONTEXT_POLL_INTERVAL).await;
            sandbox = self.sandboxes.read(caller, &sandbox.id).await?;
        }
    }

    async fn wait_for_workspace(
        &self,
        caller: &Caller,
        mut workspace: Workspace,
    ) -> Result<Workspace, AgentContextError> {
        loop {
            match workspace.status {
                WorkspaceStatus::Ready => return Ok(workspace),
                WorkspaceStatus::Preparing => {}
                WorkspaceStatus::Failed | WorkspaceStatus::Removing | WorkspaceStatus::Removed => {
                    return Err(AgentContextError::NotReady)
                }
            }
            tokio::time::sleep(CONTEXT_POLL_INTERVAL).await;
            workspace = self.workspaces.read(caller, &workspace.id).await?;
        }
    }
}

fn require(caller: &Caller) -> Result<(), AgentContextError> {
    if caller.authorization.allows(Capability::UseOwnSandboxes) {
        Ok(())
    } else if caller.authorization.allows(Capability::LaunchTriggers)
        && !caller.authorization.allows(Capability::CreateSandbox)
    {
        Err(AgentContextError::SandboxRequired)
    } else {
        Err(AgentContextError::Forbidden)
    }
}

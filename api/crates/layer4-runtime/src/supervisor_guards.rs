use std::path::Path;

use super::{Phase, Supervisor};
use crate::failure::{Result, RuntimeError};
use crate::paths::safe_component;

impl Supervisor {
    pub(super) async fn authorize_workspace_recovery(
        &self,
        frame: &layer3_adapter::WorkspacePrepareRecoveryFrame,
    ) -> Result<()> {
        frame.validate().map_err(|error| {
            RuntimeError::rejected("workspace_recovery_authority", error.to_string())
        })?;
        self.require_credentialed().await?;
        let current = self.runtime_status(frame.runtime_request()).await?;
        if !layer3_adapter::same_runtime_writer(&current, &frame.runtime)
            || self.config.home_identity.is_none()
            || !self.agents.read().await.is_empty()
            || !crate::mutation_journal::active_agents(&self.config)?.is_empty()
        {
            return Err(RuntimeError::rejected("workspace_recovery_authority", "Workspace recovery requires the exact current runtime and an empty live and durable Agent inventory"));
        }
        self.writer_boundary.require_no_writers().map_err(|error| {
            RuntimeError::unavailable("workspace_recovery_writer_present", error)
        })?;
        Ok(())
    }

    /// Public preparation may run after installation without any user credential.
    pub(super) async fn require_preparation(&self) -> Result<()> {
        if self.allow_unowned_preparation && *self.phase.read().await == Phase::Installed {
            Ok(())
        } else {
            self.require_credentialed().await
        }
    }

    pub(super) async fn authorize_preparation(
        &self,
        request: &layer3_adapter::wire::RequestFrame,
    ) -> Result<()> {
        use layer3_adapter::wire::RequestFrame;
        if matches!(request, RequestFrame::WorkspaceSetup(setup)
            if setup.operation_id.ends_with(":credential-setup:v1"))
        {
            return self.require_credentialed().await;
        }
        if matches!(
            request,
            RequestFrame::PrepareWorkspace(_)
                | RequestFrame::WorkspaceSetup(_)
                | RequestFrame::AddWorktree(_)
                | RequestFrame::MaterializeAgentHome(_)
                | RequestFrame::OpenAgent(_)
                | RequestFrame::CacheSkillArchive(_)
        ) {
            self.require_preparation().await
        } else {
            self.require_credentialed().await
        }
    }
    pub(crate) async fn ensure_workspace_idle(&self, workspace: &str) -> Result<()> {
        safe_component(workspace, "workspace name")?;
        let root = self.config.home.join(workspace);
        if self
            .agents
            .read()
            .await
            .values()
            .any(|agent| Path::new(&agent.launch.cwd).starts_with(&root))
        {
            return Err(RuntimeError::rejected(
                "workspace_agent_open",
                "workspace still has an open agent",
            ));
        }
        if crate::mutation_journal::active_agents(&self.config)?
            .iter()
            .any(|agent| Path::new(&agent.cwd).starts_with(&root))
        {
            return Err(RuntimeError::rejected(
                "workspace_agent_open",
                "workspace has a durable agent open without a completed close",
            ));
        }
        Ok(())
    }

    pub(super) async fn ensure_worktree_idle(&self, worktree: &Path) -> Result<()> {
        if self
            .agents
            .read()
            .await
            .values()
            .any(|agent| Path::new(&agent.launch.cwd) == worktree)
        {
            return Err(RuntimeError::rejected(
                "worktree_agent_open",
                "worktree still has an open agent",
            ));
        }
        if crate::mutation_journal::active_agents(&self.config)?
            .iter()
            .any(|agent| Path::new(&agent.cwd) == worktree)
        {
            return Err(RuntimeError::rejected(
                "worktree_agent_open",
                "worktree has a durable agent open without a completed close",
            ));
        }
        Ok(())
    }

    pub(super) async fn require_credentialed(&self) -> Result<()> {
        match *self.phase.read().await {
            Phase::Credentialed => Ok(()),
            Phase::RestoreCredentialed => Err(RuntimeError::rejected(
                "restore_not_completed",
                "operation is fenced until the admitted checkpoint restore completes",
            )),
            Phase::Provisioned | Phase::Installed | Phase::RestorePending => {
                Err(RuntimeError::rejected(
                    "credential_not_installed",
                    "operation is not valid before credential delivery",
                ))
            }
        }
    }
}

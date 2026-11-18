use super::*;
use layer2_core::sandbox::AgentStatus;

impl AgentWorker {
    pub(super) async fn finish_ownership_attempt(
        &self,
        id: &AgentId,
        coordinator: &layer2_core::agent_ownership::OwnershipCoordinator,
        authority: &Arc<layer2_core::agent_ownership::Authority>,
    ) -> Result<(), SandboxStoreError> {
        let agent = self.dependencies.store.get_agent(id).await?;
        if agent
            .as_ref()
            .is_some_and(|agent| agent.status.is_live() || agent.work.step != AgentWorkStep::None)
        {
            return Ok(());
        }
        self.dependencies.registry.detach_owner(&authority.token);
        if agent
            .as_ref()
            .is_none_or(|agent| matches!(agent.status, AgentStatus::Stopped | AgentStatus::Deleted))
        {
            coordinator
                .handoff(authority, authority.revoke_transports())
                .await
        } else {
            // A deferred handoff keeps custody after terminal background work;
            // it never changes the work result or fabricates a release.
            coordinator.handoff_background(authority).await.map(|_| ())
        }
        .map_err(|_| SandboxStoreError::Conflict)
    }

    pub(super) async fn defer_ownership_retry(&self, id: &AgentId) -> bool {
        let Some(coordinator) = &self.config.ownership else {
            return false;
        };
        if !coordinator.background_cleanup_deferred(id).await {
            return false;
        }
        // User Stop/Delete and all cleanup steps retain their ordinary fenced
        // path. An unavailable read never silently delays a possible control.
        let agent = match tokio::time::timeout(
            Duration::from_millis(layer2_core::agent_ownership::STORE_DEADLINE_MS),
            self.dependencies.store.get_agent(id),
        )
        .await
        {
            Ok(Ok(agent)) => agent,
            Ok(Err(_)) | Err(_) => {
                tracing::debug!(
                    "Ownership pacing observation unavailable; retain ordinary lifecycle admission"
                );
                return false;
            }
        };
        matches!(agent, Some(agent) if matches!(agent.work.step,
            AgentWorkStep::InstallSkill | AgentWorkStep::WaitForWorkspace | AgentWorkStep::AddWorktree
            | AgentWorkStep::MaterializeHome | AgentWorkStep::OpenConversation | AgentWorkStep::PublishReady
            | AgentWorkStep::PublishConnection | AgentWorkStep::DispatchFirstMessage))
    }
}

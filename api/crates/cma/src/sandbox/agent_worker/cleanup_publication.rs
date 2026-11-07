use super::{
    support::{bump_log_sequence, claimed_fence},
    AgentWorker,
};
use layer2_core::rfc3339_at;
use layer2_core::sandbox::store::{AgentWorkMutation, SandboxStoreError, VersionedWorktree};
use layer2_core::sandbox::{
    Agent, AgentLogKind, AgentStatus, AgentWorkStep, SandboxEventKind, WorktreeStatus,
};
use std::time::SystemTime;

impl AgentWorker {
    pub(super) async fn publish_stopped(
        &self,
        mut agent: Agent,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let Some(reason) = agent.requested_stop_reason.clone() else {
            return self.invalid_cleanup(agent, now).await;
        };
        if !agent.status.is_live() {
            return self.invalid_cleanup(agent, now).await;
        }
        let expected = agent.revision;
        let fence = claimed_fence(&agent, &self.config.holder)?;
        let stamp = rfc3339_at(now);
        let previous = agent.status;
        let mut worktree = self
            .dependencies
            .store
            .get_worktree(&agent.worktree_id)
            .await?
            .ok_or(SandboxStoreError::NotFound)?;
        let versioned = match worktree.status {
            WorktreeStatus::Creating => {
                let expected_revision = worktree.revision;
                worktree
                    .transition(WorktreeStatus::Failed)
                    .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
                Some(VersionedWorktree {
                    expected_revision,
                    worktree,
                })
            }
            WorktreeStatus::Ready | WorktreeStatus::Failed => None,
            WorktreeStatus::Removing | WorktreeStatus::Removed => {
                return self.invalid_cleanup(agent, now).await;
            }
        };
        agent
            .work
            .advance(
                &self.config.holder,
                fence,
                AgentWorkStep::None,
                stamp.clone(),
            )
            .map_err(|_| SandboxStoreError::Conflict)?;
        agent
            .transition(AgentStatus::Stopped, stamp.clone(), Some(reason.clone()))
            .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
        bump_log_sequence(&mut agent, 1)?;
        self.dependencies
            .registry
            .fence(&agent.id, agent.conversation.runtime_generation, fence);
        let id = agent.id.clone();
        self.dependencies
            .store
            .commit_agent_work(AgentWorkMutation {
                expected_agent_revision: expected,
                holder: self.config.holder.clone(),
                fence,
                agent,
                worktree: versioned,
                appended_at: stamp,
                sandbox_event: Some(SandboxEventKind::AgentStatusChanged {
                    agent_id: id,
                    from: previous,
                    to: AgentStatus::Stopped,
                }),
                agent_logs: vec![AgentLogKind::Stopped {
                    reason: reason.as_str().into(),
                }],
            })
            .await
    }
}

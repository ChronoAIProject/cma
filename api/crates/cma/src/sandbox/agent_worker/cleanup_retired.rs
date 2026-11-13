use super::super::support::bump_log_sequence;
use super::*;
use layer2_core::{rfc3339_at, sandbox::store::AgentWorkMutation};

impl AgentWorker {
    pub(super) async fn complete_retired_cleanup(
        &self,
        agent: &Agent,
        now: SystemTime,
    ) -> Result<bool, SandboxStoreError> {
        let sandbox = self
            .dependencies
            .store
            .get_sandbox(&agent.sandbox_id)
            .await?
            .ok_or(SandboxStoreError::NotFound)?;
        let unstarted = agent.has_no_runtime_effects()
            && matches!(agent.status, AgentStatus::Stopped | AgentStatus::Failed);
        if !unstarted && !retired_sandbox_cleanup_is_absent(agent, &sandbox) {
            return Ok(false);
        }
        let workspace = self
            .dependencies
            .store
            .get_workspace(&agent.workspace_id)
            .await?
            .ok_or(SandboxStoreError::NotFound)?;
        let mut worktree = self
            .dependencies
            .store
            .get_worktree(&agent.worktree_id)
            .await?
            .ok_or(SandboxStoreError::NotFound)?;
        if let Err(failure) = validate_lineage(agent, &sandbox, &workspace, &worktree) {
            self.handle_failure(agent.clone(), failure, now).await?;
            return Ok(true);
        }
        if !unstarted {
            match self.dependencies.adapter.runtime_inventory().await {
                Ok(inventory)
                    if inventory
                        .iter()
                        .all(|runtime| runtime.sandbox_id != sandbox.id.as_str()) => {}
                Ok(_) => {
                    self.handle_failure(
                        agent.clone(),
                        WorkerFailure::retryable(
                            "retired_runtime_present",
                            "runtime cleanup is not yet confirmed by the provider",
                        ),
                        now,
                    )
                    .await?;
                    return Ok(true);
                }
                Err(error) => {
                    self.handle_failure(agent.clone(), cleanup_failure(error), now)
                        .await?;
                    return Ok(true);
                }
            }
        }
        let next = match agent.work.step {
            AgentWorkStep::EnsureProcessClosed => AgentWorkStep::RemoveHome,
            AgentWorkStep::RemoveHome => AgentWorkStep::RemoveWorktree,
            AgentWorkStep::RemoveWorktree => AgentWorkStep::PublishDeleted,
            _ => return Err(SandboxStoreError::Conflict),
        };
        let expected_revision = worktree.revision;
        let versioned = match agent.work.step {
            AgentWorkStep::EnsureProcessClosed | AgentWorkStep::RemoveWorktree => {
                worktree
                    .transition(if agent.work.step == AgentWorkStep::EnsureProcessClosed {
                        WorktreeStatus::Removing
                    } else {
                        WorktreeStatus::Removed
                    })
                    .map_err(|_| SandboxStoreError::Conflict)?;
                Some(VersionedWorktree {
                    expected_revision,
                    worktree: worktree.clone(),
                })
            }
            _ => None,
        };
        let event = (agent.work.step == AgentWorkStep::RemoveWorktree).then_some({
            SandboxEventKind::WorktreeRemoved {
                workspace_id: worktree.workspace_id,
                worktree_id: worktree.id,
            }
        });
        self.progress(agent.clone(), next, now, None, Vec::new(), versioned, event)
            .await?;
        Ok(true)
    }

    pub(in super::super) async fn publish_deleted(
        &self,
        mut agent: Agent,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        if !matches!(agent.status, AgentStatus::Stopped | AgentStatus::Failed) {
            return self.invalid_cleanup(agent, now).await;
        }
        if !agent.work.cleanup_debt.is_empty() {
            return Err(SandboxStoreError::Corrupt(
                "deleted agent retained uncompensated side-effect debt".into(),
            ));
        }
        self.dependencies
            .store
            .get_worktree(&agent.worktree_id)
            .await?
            .filter(|worktree| worktree.status == WorktreeStatus::Removed)
            .ok_or_else(|| {
                SandboxStoreError::Corrupt("deleted agent worktree was not removed".into())
            })?;
        let expected = agent.revision;
        let fence = claimed_fence(&agent, &self.config.holder)?;
        let stamp = rfc3339_at(now);
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
            .transition(AgentStatus::Deleted, stamp.clone(), None)
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
                worktree: None,
                appended_at: stamp,
                sandbox_event: Some(SandboxEventKind::AgentDeleted { agent_id: id }),
                agent_logs: vec![AgentLogKind::Deleted],
            })
            .await
    }
}

use super::*;
use crate::sandbox::store::{
    AgentControlResult, AgentControlStore, AgentControlStoreError, AgentControlTarget,
    AgentDeleteRequest, AgentStopRequest,
};
use crate::sandbox::{AgentStatus, AgentWorkStep};

#[async_trait]
impl AgentControlStore for InMemorySandboxStore {
    async fn schedule_agent_stop(
        &self,
        request: AgentStopRequest,
    ) -> Result<AgentControlResult, AgentControlStoreError> {
        let mut state = self.lock();
        let current = control_agent(&state, &request.target)?.clone();

        if !current.status.is_live() {
            require_exact_target(&current, &request.target)?;
            return Err(AgentControlStoreError::StatusConflict {
                status: current.status,
            });
        }
        if current.stop_reason.is_some()
            || current.stopped_at.is_some()
            || current.deleted_at.is_some()
        {
            return Err(AgentControlStoreError::Corrupt(
                "live agent contains terminal stop data".into(),
            ));
        }
        if let Some(existing) = current.requested_stop_reason.as_ref() {
            if existing != &request.reason {
                return Err(AgentControlStoreError::StopReasonConflict);
            }
            if is_stop_work(current.work.step) {
                return Ok(AgentControlResult::Existing(current));
            }
            return Err(AgentControlStoreError::WorkConflict {
                step: current.work.step,
            });
        }
        require_exact_target(&current, &request.target)?;
        if !can_supersede_with_stop(current.work.step) {
            return Err(AgentControlStoreError::WorkConflict {
                step: current.work.step,
            });
        }

        let mut next = current;
        next.request_stop(request.reason, request.requested_at)
            .map_err(|_| AgentControlStoreError::Conflict)?;
        state.agents.insert(next.id.clone(), next.clone());
        Ok(AgentControlResult::Scheduled(next))
    }

    async fn schedule_agent_delete(
        &self,
        request: AgentDeleteRequest,
    ) -> Result<AgentControlResult, AgentControlStoreError> {
        let mut state = self.lock();
        let current = control_agent(&state, &request.target)?.clone();

        if current.status == AgentStatus::Deleted {
            return if current.work.step == AgentWorkStep::None || is_delete_work(current.work.step)
            {
                Ok(AgentControlResult::Existing(current))
            } else {
                Err(AgentControlStoreError::WorkConflict {
                    step: current.work.step,
                })
            };
        }
        if matches!(current.status, AgentStatus::Stopped | AgentStatus::Failed)
            && current.requested_stop_reason.is_some()
        {
            return Err(AgentControlStoreError::Corrupt(
                "terminal agent still contains a requested stop reason".into(),
            ));
        }
        if matches!(current.status, AgentStatus::Stopped | AgentStatus::Failed)
            && is_delete_work(current.work.step)
        {
            return Ok(AgentControlResult::Existing(current));
        }
        require_exact_target(&current, &request.target)?;
        if !matches!(current.status, AgentStatus::Stopped | AgentStatus::Failed) {
            return Err(AgentControlStoreError::StatusConflict {
                status: current.status,
            });
        }
        if current.work.step != AgentWorkStep::None {
            return Err(AgentControlStoreError::WorkConflict {
                step: current.work.step,
            });
        }

        let mut next = current;
        next.request_delete(request.requested_at)
            .map_err(|_| AgentControlStoreError::Conflict)?;
        state.agents.insert(next.id.clone(), next.clone());
        Ok(AgentControlResult::Scheduled(next))
    }
}

fn control_agent<'a>(
    state: &'a State,
    target: &AgentControlTarget,
) -> Result<&'a Agent, AgentControlStoreError> {
    let agent = state
        .agents
        .get(&target.agent_id)
        .ok_or(AgentControlStoreError::NotFound)?;
    if agent.id != target.agent_id {
        return Err(AgentControlStoreError::Corrupt(
            "agent storage identity does not match its key".into(),
        ));
    }
    if agent.owner != target.owner {
        return Err(AgentControlStoreError::NotFound);
    }
    if agent.revision == 0 || agent.work.next_fence == 0 {
        return Err(AgentControlStoreError::Corrupt(
            "agent revision or work fence is invalid".into(),
        ));
    }
    validate_stored_lineage(state, agent)?;
    if agent.worktree_id != target.worktree_id
        || agent.workspace_id != target.workspace_id
        || agent.sandbox_id != target.sandbox_id
    {
        return Err(AgentControlStoreError::Conflict);
    }
    Ok(agent)
}

fn require_exact_target(
    agent: &Agent,
    target: &AgentControlTarget,
) -> Result<(), AgentControlStoreError> {
    if agent.status != target.expected_status || agent.revision != target.expected_revision {
        return Err(AgentControlStoreError::Conflict);
    }
    Ok(())
}

fn validate_stored_lineage(state: &State, agent: &Agent) -> Result<(), AgentControlStoreError> {
    let worktree = state
        .worktrees
        .get(&agent.worktree_id)
        .ok_or_else(|| AgentControlStoreError::Corrupt("agent worktree is missing".into()))?;
    let workspace = state
        .workspaces
        .get(&agent.workspace_id)
        .ok_or_else(|| AgentControlStoreError::Corrupt("agent workspace is missing".into()))?;
    let sandbox = state
        .sandboxes
        .get(&agent.sandbox_id)
        .ok_or_else(|| AgentControlStoreError::Corrupt("agent sandbox is missing".into()))?;
    if worktree.id != agent.worktree_id
        || worktree.agent_id.as_ref() != Some(&agent.id)
        || worktree.workspace_id != agent.workspace_id
        || worktree.sandbox_id != agent.sandbox_id
        || worktree.owner != agent.owner
        || workspace.id != agent.workspace_id
        || workspace.sandbox_id != agent.sandbox_id
        || workspace.owner != agent.owner
        || sandbox.id != agent.sandbox_id
        || sandbox.owner != agent.owner
    {
        return Err(AgentControlStoreError::Corrupt(
            "agent lineage does not match its parents".into(),
        ));
    }
    Ok(())
}

fn can_supersede_with_stop(step: AgentWorkStep) -> bool {
    matches!(
        step,
        AgentWorkStep::InstallSkill
            | AgentWorkStep::AddWorktree
            | AgentWorkStep::MaterializeHome
            | AgentWorkStep::OpenConversation
            | AgentWorkStep::PublishReady
            | AgentWorkStep::PublishConnection
            | AgentWorkStep::DispatchFirstMessage
            | AgentWorkStep::None
    )
}

fn is_stop_work(step: AgentWorkStep) -> bool {
    matches!(
        step,
        AgentWorkStep::FenceInteraction
            | AgentWorkStep::CloseProcess
            | AgentWorkStep::PublishStopped
    )
}

fn is_delete_work(step: AgentWorkStep) -> bool {
    matches!(
        step,
        AgentWorkStep::EnsureProcessClosed
            | AgentWorkStep::RemoveHome
            | AgentWorkStep::RemoveWorktree
            | AgentWorkStep::PublishDeleted
    )
}

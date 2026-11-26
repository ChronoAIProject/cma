//! Atomic Mongo scheduling for owner-authorized agent stop and deletion.

use layer2_core::sandbox::store::{
    AgentControlResult, AgentControlStore, AgentControlStoreError, AgentControlTarget,
    AgentDeleteRequest, AgentStopRequest,
};
use layer2_core::sandbox::{Agent, AgentStatus, AgentWorkStep};

use super::agent_control_transaction::{schedule_delete, schedule_stop};
use super::sandbox_mongo::MongoSandboxStore;

#[async_trait::async_trait]
impl AgentControlStore for MongoSandboxStore {
    async fn begin_agent_worktree_recovery(
        &self,
        id: &layer2_core::AgentId,
        caller: &layer2_core::Caller,
        operation_id: &str,
        successor_id: &layer2_core::WorktreeId,
        now: String,
    ) -> Result<Agent, AgentControlStoreError> {
        super::agent_worktree_recovery::begin(self, id, caller, operation_id, successor_id, now)
            .await
    }

    async fn schedule_agent_restart(
        &self,
        request: layer2_core::sandbox::store::AgentRestartRequest,
    ) -> Result<layer2_core::sandbox::store::AgentRestartAcceptance, AgentControlStoreError> {
        super::agent_control_transaction::schedule_restart(self, request).await
    }

    async fn schedule_agent_stop(
        &self,
        request: AgentStopRequest,
    ) -> Result<AgentControlResult, AgentControlStoreError> {
        schedule_stop(self, request).await
    }

    async fn schedule_agent_delete(
        &self,
        request: AgentDeleteRequest,
    ) -> Result<AgentControlResult, AgentControlStoreError> {
        schedule_delete(self, request).await
    }
}

pub(super) fn plan_stop(
    current: Agent,
    request: &AgentStopRequest,
) -> Result<AgentControlResult, AgentControlStoreError> {
    if !current.status.is_live() {
        require_exact_target(&current, &request.target)?;
        return Err(AgentControlStoreError::StatusConflict {
            status: current.status,
        });
    }
    if current.stop_reason.is_some() || current.stopped_at.is_some() || current.deleted_at.is_some()
    {
        return Err(AgentControlStoreError::Corrupt(
            "live agent contains terminal stop data".into(),
        ));
    }
    if let Some(existing) = current.requested_stop_reason.as_ref() {
        if existing != &request.reason {
            return Err(AgentControlStoreError::StopReasonConflict);
        }
        return if is_stop_work(current.work.step) {
            Ok(AgentControlResult::Existing(current))
        } else {
            Err(AgentControlStoreError::WorkConflict {
                step: current.work.step,
            })
        };
    }
    require_exact_target(&current, &request.target)?;
    if !can_supersede_with_stop(current.work.step) {
        return Err(AgentControlStoreError::WorkConflict {
            step: current.work.step,
        });
    }
    let mut next = current;
    next.request_stop(request.reason.clone(), request.requested_at.clone())
        .map_err(|_| AgentControlStoreError::Conflict)?;
    Ok(AgentControlResult::Scheduled(next))
}

pub(super) fn plan_delete(
    current: Agent,
    request: &AgentDeleteRequest,
) -> Result<AgentControlResult, AgentControlStoreError> {
    if current.status == AgentStatus::Deleted {
        return if current.work.step == AgentWorkStep::None || is_delete_work(current.work.step) {
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
    next.request_delete(request.requested_at.clone())
        .map_err(|_| AgentControlStoreError::Conflict)?;
    Ok(AgentControlResult::Scheduled(next))
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

fn can_supersede_with_stop(step: AgentWorkStep) -> bool {
    matches!(
        step,
        AgentWorkStep::InstallSkill
            | AgentWorkStep::WaitForWorkspace
            | AgentWorkStep::AddWorktree
            | AgentWorkStep::MaterializeHome
            | AgentWorkStep::OpenConversation
            | AgentWorkStep::PublishReady
            | AgentWorkStep::PublishConnection
            | AgentWorkStep::DispatchFirstMessage
            | AgentWorkStep::None
    )
}

pub(super) fn is_stop_work(step: AgentWorkStep) -> bool {
    matches!(
        step,
        AgentWorkStep::FenceInteraction
            | AgentWorkStep::CloseProcess
            | AgentWorkStep::PublishStopped
    )
}

pub(super) fn is_delete_work(step: AgentWorkStep) -> bool {
    matches!(
        step,
        AgentWorkStep::EnsureProcessClosed
            | AgentWorkStep::RemoveHome
            | AgentWorkStep::RemoveWorktree
            | AgentWorkStep::PublishDeleted
    )
}

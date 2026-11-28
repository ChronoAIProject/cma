//! Durable Stop and Delete planning over the current Agent record.

use layer2_core::sandbox::store::{
    AgentControlResult, AgentControlStoreError, AgentControlTarget, AgentDeleteRequest,
    AgentStopRequest,
};
use layer2_core::sandbox::{Agent, AgentStatus, AgentWorkStep};

pub fn plan_stop(
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

pub fn plan_delete(
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

pub fn is_stop_work(step: AgentWorkStep) -> bool {
    matches!(
        step,
        AgentWorkStep::FenceInteraction
            | AgentWorkStep::CloseProcess
            | AgentWorkStep::PublishStopped
    )
}

pub fn is_delete_work(step: AgentWorkStep) -> bool {
    matches!(
        step,
        AgentWorkStep::EnsureProcessClosed
            | AgentWorkStep::RemoveHome
            | AgentWorkStep::RemoveWorktree
            | AgentWorkStep::PublishDeleted
    )
}

use std::collections::BTreeSet;

use crate::sandbox::{
    Agent, AgentOrigin, AgentStatus, Sandbox, SandboxAgentActivity, SandboxCheckpointCapturePhase,
    SandboxEventKind, SandboxStatus, SandboxWorkStep, Workspace, WorkspaceStatus,
};
use crate::seconds_from_rfc3339;

use super::{SandboxLifecycleAgentScope, SandboxLifecycleMutation, SandboxStoreError};

pub const PERIODIC_CHECKPOINT_QUIET_SECONDS: u64 = 120;

/// Begin must supply every live Agent's exact durable log head in its transaction.
/// This debounce does not apply to explicit lifecycle checkpoints or writer recovery.
pub fn validate_checkpoint_capture_quiet(
    sandbox: &Sandbox,
    live_agents: &[SandboxAgentActivity],
    at: &str,
) -> Result<(), SandboxStoreError> {
    let now = seconds_from_rfc3339(at).ok_or(SandboxStoreError::Conflict)?;
    let mut ids = BTreeSet::new();
    if live_agents.iter().any(|activity| {
        activity.sandbox_id != sandbox.id
            || !ids.insert(&activity.agent_id)
            || !matches!(activity.status, AgentStatus::Ready | AgentStatus::Paused)
            || activity.active_turn.is_some()
            || activity.work_pending
            || activity.stop_requested
            || activity.queued_messages_present
            || activity.newest_log.as_ref().is_none_or(|newest| {
                newest.agent_id != activity.agent_id
                    || newest.seq == 0
                    || newest.seq != activity.last_seq
                    || seconds_from_rfc3339(&newest.appended_at).is_none_or(|appended| {
                        now < appended.saturating_add(PERIODIC_CHECKPOINT_QUIET_SECONDS)
                    })
            })
    }) {
        return Err(SandboxStoreError::Conflict);
    }
    Ok(())
}

/// Stores must inspect all writer-capable rows in the same transaction as Begin.
pub fn validate_checkpoint_capture_work(
    sandbox: &Sandbox,
    agents: &[Agent],
    workspaces: &[Workspace],
) -> Result<(), SandboxStoreError> {
    if sandbox.status != SandboxStatus::Active
        || sandbox.lifecycle.operation.is_some()
        || sandbox.lifecycle.checkpoint_capture.is_some()
        || sandbox.work.lease.is_some()
        || !matches!(
            sandbox.work.step,
            SandboxWorkStep::None | SandboxWorkStep::WaitForCredentialRotation
        )
        || agents.iter().any(|agent| {
            agent.sandbox_id != sandbox.id
                || agent.owner != sandbox.owner
                || agent.work.step.is_pending()
                || agent.work.lease.is_some()
                || (agent.status.is_live()
                    && (!matches!(agent.status, AgentStatus::Ready | AgentStatus::Paused)
                        || agent.conversation.active_turn.is_some()
                        || agent
                            .conversation
                            .operation
                            .as_ref()
                            .is_some_and(|operation| operation.blocks_dispatch())
                        || agent.requested_stop_reason.is_some()))
        })
        || workspaces.iter().any(|workspace| {
            workspace.sandbox_id != sandbox.id
                || workspace.owner != sandbox.owner
                || workspace.work.step.is_pending()
                || workspace.work.lease.is_some()
                || profile_agents_pending(workspace, agents)
        })
    {
        return Err(SandboxStoreError::Conflict);
    }
    Ok(())
}

fn profile_agents_pending(workspace: &Workspace, agents: &[Agent]) -> bool {
    if workspace.status != WorkspaceStatus::Ready {
        return false;
    }
    workspace
        .profile
        .content
        .agent_profiles
        .iter()
        .enumerate()
        .any(|(index, reference)| {
            !agents.iter().any(|agent| {
                if agent.workspace_id != workspace.id
                    || agent.profile.id != reference.id
                    || agent.profile.revision != reference.revision
                {
                    return false;
                }
                match agent.origin {
                    AgentOrigin::WorkspaceProfile { index: admitted } => {
                        usize::from(admitted) == index
                            && agent.creation_key
                                == crate::sandbox::agent::profile_agent_creation_key(
                                    &workspace.id,
                                    admitted,
                                )
                    }
                    // A Trigger can win the same automatic admission slot. Terminal
                    // rows still fulfill it: profile reconciliation never recreates them.
                    AgentOrigin::Trigger { .. } => {
                        agent.creation_key.starts_with("trigger-profile:")
                            && workspace.profile.content.agent_profiles[..index]
                                .iter()
                                .all(|prior| prior != reference)
                    }
                    AgentOrigin::Manual | AgentOrigin::Lifecycle { .. } => false,
                }
            })
        })
}

pub(super) fn validate_capture_mutation(
    current: &Sandbox,
    agents: &[(Agent, u64)],
    mutation: &SandboxLifecycleMutation,
) -> Result<(), SandboxStoreError> {
    if mutation.agent_scope != SandboxLifecycleAgentScope::Touched
        || !mutation.agents.is_empty()
        || mutation.events
            != [SandboxEventKind::WorkStepCompleted {
                step: SandboxWorkStep::RunLifecycleOperation,
            }]
    {
        return Err(invalid());
    }
    let mut expected = current.clone();
    let next = &mutation.sandbox;
    let at = &mutation.appended_at;
    let result = match (
        &current.lifecycle.checkpoint_capture,
        &next.lifecycle.checkpoint_capture,
    ) {
        (None, Some(_)) => {
            if !current.checkpoint_capture_cooldown_elapsed(at) {
                return Err(SandboxStoreError::Conflict);
            }
            if agents
                .iter()
                .any(|(agent, tail)| agent.conversation.last_seq != *tail)
            {
                return Err(SandboxStoreError::SequenceConflict);
            }
            let rows = agents
                .iter()
                .map(|(agent, _)| agent.clone())
                .collect::<Vec<_>>();
            validate_checkpoint_capture_work(current, &rows, &[])?;
            expected.begin_checkpoint_capture(at.clone())
        }
        (Some(capture), Some(submitted))
            if submitted.work.lease.is_some() && submitted.work.lease != capture.work.lease =>
        {
            let lease = submitted.work.lease.as_ref().ok_or_else(invalid)?;
            if capture.work.owns(&lease.holder, lease.fence) {
                expected.renew_checkpoint_capture(
                    &lease.holder,
                    lease.fence,
                    at.clone(),
                    lease.expires_at.clone(),
                )
            } else {
                expected.claim_checkpoint_capture(
                    lease.holder.clone(),
                    at.clone(),
                    lease.expires_at.clone(),
                )
            }
        }
        (Some(capture), submitted) => {
            let lease = capture.work.lease.as_ref().ok_or_else(invalid)?;
            match submitted {
                None => expected.complete_checkpoint_writer_resume(
                    &lease.holder,
                    lease.fence,
                    at.clone(),
                ),
                Some(next) if next.work.attempts == capture.work.attempts.saturating_add(1) => {
                    expected.retry_checkpoint_capture(
                        next.work.ready_at.clone(),
                        &lease.holder,
                        lease.fence,
                        at.clone(),
                    )
                }
                Some(next) if next.phase == SandboxCheckpointCapturePhase::ResumeWriters => {
                    expected.require_checkpoint_writer_resume(
                        &lease.holder,
                        lease.fence,
                        at.clone(),
                    )
                }
                _ => return Err(invalid()),
            }
        }
        _ => return Err(invalid()),
    };
    if result.is_err() || expected != *next {
        return Err(invalid());
    }
    Ok(())
}

const fn invalid() -> SandboxStoreError {
    SandboxStoreError::InvalidTransaction("checkpoint capture transition is not exact")
}

#[cfg(test)]
#[path = "checkpoint_capture_quiet_tests.rs"]
mod quiet_tests;

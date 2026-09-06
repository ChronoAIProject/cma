use crate::sandbox::{
    Agent, AgentStatus, Sandbox, SandboxCheckpointCapturePhase, SandboxEventKind, SandboxStatus,
    SandboxWorkStep, Workspace,
};

use super::{SandboxLifecycleAgentScope, SandboxLifecycleMutation, SandboxStoreError};

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
        })
    {
        return Err(SandboxStoreError::Conflict);
    }
    Ok(())
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
            expected.claim_checkpoint_capture(
                lease.holder.clone(),
                at.clone(),
                lease.expires_at.clone(),
            )
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

use crate::sandbox::queued_message::{
    QUEUED_MESSAGE_RECOVERY_DETAIL, QUEUED_MESSAGE_RECOVERY_EVENT_CLASS,
};
use crate::sandbox::{
    AgentLogKind, AgentStatus, Sandbox, SandboxEventKind, SandboxLifecycleOperation, SandboxStatus,
    SandboxWorkStep,
};

use super::lifecycle_agent_validation::AgentPhase;
use super::{SandboxLifecycleMutation, SandboxStoreError};

pub(super) fn validate_lifecycle_events(
    current: &Sandbox,
    mutation: &SandboxLifecycleMutation,
    phase: AgentPhase,
) -> Result<(), SandboxStoreError> {
    let expected = match phase {
        AgentPhase::Pause | AgentPhase::Reopen | AgentPhase::QueueProgress => {
            let mut events = mutation
                .agents
                .iter()
                .filter(|change| !matches!(phase, AgentPhase::Pause) || !change.logs.is_empty())
                .map(|change| SandboxEventKind::AgentStatusChanged {
                    agent_id: change.agent.id.clone(),
                    from: match phase {
                        AgentPhase::Pause => previous_status(&change.logs),
                        AgentPhase::Reopen => AgentStatus::Paused,
                        AgentPhase::QueueProgress => AgentStatus::Ready,
                        _ => unreachable!(),
                    },
                    to: change.agent.status,
                })
                .collect::<Vec<_>>();
            events.push(work_completed());
            events
        }
        AgentPhase::QueueRecovery => vec![SandboxEventKind::WorkStepFailed {
            step: SandboxWorkStep::RunLifecycleOperation,
            failure: crate::sandbox::TypedFailure {
                class: QUEUED_MESSAGE_RECOVERY_EVENT_CLASS.into(),
                detail: QUEUED_MESSAGE_RECOVERY_DETAIL.into(),
            },
        }],
        AgentPhase::QueueCompletion => vec![work_completed()],
        AgentPhase::SandboxOnly => sandbox_only_events(current, mutation)?,
    };
    if mutation.events != expected {
        return Err(invalid(
            "lifecycle events do not exactly describe the transition",
        ));
    }
    Ok(())
}

fn sandbox_only_events(
    current: &Sandbox,
    mutation: &SandboxLifecycleMutation,
) -> Result<Vec<SandboxEventKind>, SandboxStoreError> {
    let next = &mutation.sandbox;
    if let Some(receipt) = next
        .lifecycle
        .unpublished_recovery
        .as_ref()
        .and_then(|recovery| recovery.quiescence.as_ref())
        .filter(|_| {
            current
                .lifecycle
                .unpublished_recovery
                .as_ref()
                .is_some_and(|recovery| recovery.quiescence.is_none())
        })
    {
        return Ok(vec![
            SandboxEventKind::RuntimeRecoveryQuiesced {
                receipt: Box::new(receipt.clone()),
            },
            work_completed(),
        ]);
    }
    if current.credential.current != next.credential.current {
        return Ok(vec![SandboxEventKind::CredentialChanged {
            key_generation: next.credential.key_generation,
        }]);
    }
    if let Some(failure) = next
        .runtime_update_request()
        .and_then(|request| request.failure.as_ref())
        .filter(|_| {
            current
                .runtime_update_request()
                .is_some_and(|request| request.failure.is_none())
        })
    {
        return Ok(vec![SandboxEventKind::WorkStepFailed {
            step: SandboxWorkStep::RunLifecycleOperation,
            failure: failure.clone(),
        }]);
    }
    if next.status == SandboxStatus::Failed {
        let failure = next
            .failure
            .clone()
            .ok_or_else(|| invalid("failed lifecycle has no failure"))?;
        return Ok(vec![SandboxEventKind::WorkStepFailed {
            step: SandboxWorkStep::RunLifecycleOperation,
            failure,
        }]);
    }
    if next.work.attempts == current.work.attempts.saturating_add(1) {
        return match mutation.events.as_slice() {
            [SandboxEventKind::WorkStepFailed {
                step: SandboxWorkStep::RunLifecycleOperation,
                failure,
            }] => Ok(vec![SandboxEventKind::WorkStepFailed {
                step: SandboxWorkStep::RunLifecycleOperation,
                failure: failure.clone(),
            }]),
            _ => Err(invalid("lifecycle retry has invalid failure evidence")),
        };
    }
    if current.status != next.status {
        let mut events = vec![SandboxEventKind::StatusChanged {
            from: current.status,
            to: next.status,
        }];
        if next.status == SandboxStatus::Active {
            match current.lifecycle.operation.as_ref() {
                Some(SandboxLifecycleOperation::Revive { .. }) => {
                    events.push(SandboxEventKind::Revived)
                }
                Some(SandboxLifecycleOperation::Pause {
                    phase: crate::sandbox::SandboxPausePhase::ResumeWriters,
                    ..
                }) => {}
                Some(SandboxLifecycleOperation::ImageRotation {
                    target_runtime_version,
                    ..
                }) => events.push(SandboxEventKind::Rotated {
                    runtime_version: target_runtime_version.clone(),
                }),
                _ => return Err(invalid("active publication has no recovery operation")),
            }
        }
        return Ok(events);
    }
    Ok(vec![work_completed()])
}

fn previous_status(logs: &[AgentLogKind]) -> AgentStatus {
    match logs.first() {
        Some(AgentLogKind::StatusChanged { from, .. }) => *from,
        _ => AgentStatus::Paused,
    }
}

fn work_completed() -> SandboxEventKind {
    SandboxEventKind::WorkStepCompleted {
        step: SandboxWorkStep::RunLifecycleOperation,
    }
}

const fn invalid(detail: &'static str) -> SandboxStoreError {
    SandboxStoreError::InvalidTransaction(detail)
}

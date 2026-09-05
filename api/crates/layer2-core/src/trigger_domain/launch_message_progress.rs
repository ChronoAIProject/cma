use crate::sandbox::{FirstMessageDelivery, FirstMessageRecord};
use crate::{seconds_from_rfc3339, AgentId};

use super::launch_message_evidence::{
    observe_first_message, validate_first_message_record, EvidenceMode,
};
use super::launch_progress::{progress_error, require_command_time, settle_worker_command};
use super::{
    Launch, LaunchBlockedReason, LaunchEffectState, LaunchEventKind, LaunchFailure, LaunchState,
    LaunchStepKey, LaunchStepStatus, LaunchWorkStep, TriggerError,
};

impl Launch {
    pub fn record_worker_agent_ready(
        &mut self,
        holder: &str,
        fence: u64,
        agent_id: &AgentId,
        first_message: Option<&FirstMessageRecord>,
        at: String,
    ) -> Result<Vec<LaunchEventKind>, TriggerError> {
        let command_at = at.clone();
        let original = self.clone();
        require_command_time(&original, &command_at)?;
        let result = (|| {
            self.require_worker(holder, fence, LaunchWorkStep::WaitForAgent)?;
            if self.state != LaunchState::AgentPending
                || self.resolution.agent.id() != Some(agent_id)
                || self.first_message_evidence.is_some()
            {
                return Err(progress_error());
            }
            if let Some(record) = first_message {
                validate_first_message_record(record)?;
            }
            let expects_message = self.first_message == FirstMessageDelivery::Queued;
            if expects_message != first_message.is_some() {
                return Err(progress_error());
            }

            let agent_step = self.set_step(
                LaunchStepKey::Agent,
                LaunchStepStatus::Completed,
                "Agent is ready",
                &at,
            )?;
            if !expects_message {
                let ready_step = self.set_step(
                    LaunchStepKey::Ready,
                    LaunchStepStatus::Completed,
                    "Agent is ready",
                    &at,
                )?;
                self.transition(LaunchState::Ready, at)?;
                return Ok(vec![
                    agent_step,
                    LaunchEventKind::StateChanged {
                        from: LaunchState::AgentPending,
                        to: LaunchState::Ready,
                    },
                    ready_step,
                ]);
            }

            self.work.advance(
                holder,
                fence,
                LaunchWorkStep::DeliverFirstMessage,
                at.clone(),
            )?;
            self.transition(LaunchState::MessagePending, at.clone())?;
            let message_step = self.set_step(
                LaunchStepKey::FirstMessage,
                LaunchStepStatus::Running,
                "Waiting for the first message",
                &at,
            )?;
            Ok(vec![
                agent_step,
                LaunchEventKind::StateChanged {
                    from: LaunchState::AgentPending,
                    to: LaunchState::MessagePending,
                },
                message_step,
            ])
        })();
        settle_worker_command(self, original, &command_at, result)
    }

    pub fn record_worker_first_message(
        &mut self,
        holder: &str,
        fence: u64,
        agent_id: &AgentId,
        record: &FirstMessageRecord,
        retry_at: String,
        at: String,
    ) -> Result<Vec<LaunchEventKind>, TriggerError> {
        let command_at = at.clone();
        let original = self.clone();
        require_command_time(&original, &command_at)?;
        require_retry_time(&command_at, &retry_at)?;
        let result = (|| {
            let work_step = self.work.step;
            if !message_work_step(work_step) {
                return Err(progress_error());
            }
            self.require_worker(holder, fence, work_step)?;
            if self.state != LaunchState::MessagePending {
                return Err(progress_error());
            }

            let mut events = observe_first_message(self, agent_id, record, EvidenceMode::Worker)?
                .into_iter()
                .collect::<Vec<_>>();
            settle_observed_message(
                self,
                holder,
                fence,
                work_step,
                record,
                retry_at,
                at,
                &mut events,
            )?;
            Ok(events)
        })();
        settle_worker_command(self, original, &command_at, result)
    }

    pub fn reconcile_first_message_recovery(
        &mut self,
        agent_id: &AgentId,
        record: &FirstMessageRecord,
        retry_at: String,
        at: String,
    ) -> Result<Vec<LaunchEventKind>, TriggerError> {
        let command_at = at.clone();
        let original = self.clone();
        require_command_time(&original, &command_at)?;
        require_retry_time(&command_at, &retry_at)?;
        let result = (|| {
            if self.state != LaunchState::MessagePending
                || self.first_message != FirstMessageDelivery::RecoveryRequired
                || self.blocked_reason != Some(LaunchBlockedReason::RecoveryRequired)
                || self.work.lease.is_some()
                || !message_work_step(self.work.step)
            {
                return Err(progress_error());
            }

            let mut events = observe_first_message(self, agent_id, record, EvidenceMode::Recovery)?
                .into_iter()
                .collect::<Vec<_>>();
            settle_recovered_message(self, record, retry_at, at, &mut events)?;
            Ok(events)
        })();
        settle_worker_command(self, original, &command_at, result)
    }

    pub fn finish_worker_launch(
        &mut self,
        holder: &str,
        fence: u64,
        at: String,
    ) -> Result<Vec<LaunchEventKind>, TriggerError> {
        let command_at = at.clone();
        let original = self.clone();
        require_command_time(&original, &command_at)?;
        let result = (|| {
            self.require_worker(holder, fence, LaunchWorkStep::Finish)?;
            if self.state != LaunchState::MessagePending
                || !accepted(self.first_message)
                || self.first_message_evidence.is_none()
            {
                return Err(progress_error());
            }
            let step = self.set_step(
                LaunchStepKey::Ready,
                LaunchStepStatus::Completed,
                "Agent is ready",
                &at,
            )?;
            self.transition(LaunchState::Ready, at)?;
            Ok(vec![
                LaunchEventKind::StateChanged {
                    from: LaunchState::MessagePending,
                    to: LaunchState::Ready,
                },
                step,
            ])
        })();
        settle_worker_command(self, original, &command_at, result)
    }

    pub fn block_worker_recovery(
        &mut self,
        holder: &str,
        fence: u64,
        at: String,
    ) -> Result<Vec<LaunchEventKind>, TriggerError> {
        let command_at = at.clone();
        let original = self.clone();
        require_command_time(&original, &command_at)?;
        let result = (|| {
            let step = self.work.step;
            self.require_worker(holder, fence, step)?;
            let effect = self.work.effect.as_ref().ok_or_else(progress_error)?;
            if effect.state == LaunchEffectState::Executing && effect.fence == fence {
                self.work
                    .mark_effect_recovery_required(holder, fence, at.clone())?;
            } else if effect.state != LaunchEffectState::RecoveryRequired {
                return Err(progress_error());
            }
            let progress = self.set_step(
                provisioning_progress_key(step)?,
                LaunchStepStatus::Blocked,
                "Provisioning outcome needs review",
                &at,
            )?;
            self.block(LaunchBlockedReason::RecoveryRequired, at)?;
            Ok(vec![
                progress,
                LaunchEventKind::Blocked {
                    reason: LaunchBlockedReason::RecoveryRequired,
                },
            ])
        })();
        settle_worker_command(self, original, &command_at, result)
    }
}

#[allow(clippy::too_many_arguments)]
fn settle_observed_message(
    launch: &mut Launch,
    holder: &str,
    fence: u64,
    work_step: LaunchWorkStep,
    record: &FirstMessageRecord,
    retry_at: String,
    at: String,
    events: &mut Vec<LaunchEventKind>,
) -> Result<(), TriggerError> {
    match record.state {
        FirstMessageDelivery::Queued => {
            if work_step == LaunchWorkStep::WaitForFirstMessage {
                launch.work.advance(
                    holder,
                    fence,
                    LaunchWorkStep::DeliverFirstMessage,
                    retry_at,
                )?;
            } else {
                launch.work.retry(holder, fence, retry_at)?;
            }
            let (status, detail) = if record.attempt == 0 {
                (LaunchStepStatus::Running, "Waiting for the first message")
            } else {
                (
                    LaunchStepStatus::Retryable,
                    "First-message delivery will be retried",
                )
            };
            events.push(launch.set_step(LaunchStepKey::FirstMessage, status, detail, &at)?);
            finish_in_place(launch, at)
        }
        FirstMessageDelivery::DispatchFenced => {
            launch
                .work
                .advance(holder, fence, LaunchWorkStep::WaitForFirstMessage, retry_at)?;
            events.push(launch.set_step(
                LaunchStepKey::FirstMessage,
                LaunchStepStatus::Running,
                "Sending the first message",
                &at,
            )?);
            finish_in_place(launch, at)
        }
        FirstMessageDelivery::WriteConfirmed
        | FirstMessageDelivery::ExecutionObserved
        | FirstMessageDelivery::Completed => {
            launch
                .work
                .advance(holder, fence, LaunchWorkStep::Finish, at.clone())?;
            events.push(launch.set_step(
                LaunchStepKey::FirstMessage,
                LaunchStepStatus::Completed,
                "First message was sent",
                &at,
            )?);
            finish_in_place(launch, at)
        }
        FirstMessageDelivery::RecoveryRequired => {
            events.push(launch.set_step(
                LaunchStepKey::FirstMessage,
                LaunchStepStatus::Blocked,
                "First-message delivery needs review",
                &at,
            )?);
            launch.block(LaunchBlockedReason::RecoveryRequired, at)?;
            events.push(LaunchEventKind::Blocked {
                reason: LaunchBlockedReason::RecoveryRequired,
            });
            Ok(())
        }
        FirstMessageDelivery::Failed => fail_launch(launch, record, at, events),
    }
}

fn settle_recovered_message(
    launch: &mut Launch,
    record: &FirstMessageRecord,
    retry_at: String,
    at: String,
    events: &mut Vec<LaunchEventKind>,
) -> Result<(), TriggerError> {
    match record.state {
        FirstMessageDelivery::Queued => {
            schedule_without_lease(launch, LaunchWorkStep::DeliverFirstMessage, retry_at)?;
            events.push(launch.set_step(
                LaunchStepKey::FirstMessage,
                LaunchStepStatus::Retryable,
                "First-message delivery was authorized to retry",
                &at,
            )?);
            resume_in_place(launch, at, events)
        }
        FirstMessageDelivery::DispatchFenced => {
            schedule_without_lease(launch, LaunchWorkStep::WaitForFirstMessage, retry_at)?;
            events.push(launch.set_step(
                LaunchStepKey::FirstMessage,
                LaunchStepStatus::Running,
                "Sending the first message",
                &at,
            )?);
            resume_in_place(launch, at, events)
        }
        FirstMessageDelivery::WriteConfirmed
        | FirstMessageDelivery::ExecutionObserved
        | FirstMessageDelivery::Completed => {
            schedule_without_lease(launch, LaunchWorkStep::Finish, at.clone())?;
            events.push(launch.set_step(
                LaunchStepKey::FirstMessage,
                LaunchStepStatus::Completed,
                "First message was sent",
                &at,
            )?);
            resume_in_place(launch, at, events)
        }
        FirstMessageDelivery::Failed => {
            launch.blocked_reason = None;
            fail_launch(launch, record, at, events)
        }
        FirstMessageDelivery::RecoveryRequired => {
            events.push(launch.set_step(
                LaunchStepKey::FirstMessage,
                LaunchStepStatus::Blocked,
                "First-message delivery needs review",
                &at,
            )?);
            finish_in_place(launch, at)
        }
    }
}

fn fail_launch(
    launch: &mut Launch,
    record: &FirstMessageRecord,
    at: String,
    events: &mut Vec<LaunchEventKind>,
) -> Result<(), TriggerError> {
    let durable = record.failure.as_ref().ok_or_else(progress_error)?;
    let failure = LaunchFailure {
        code: "first_message_failed".into(),
        safe_detail: "The first message could not be delivered".into(),
        retryable: false,
        occurred_at: durable.occurred_at.clone(),
    };
    launch.failure = Some(failure.clone());
    events.push(launch.set_step(
        LaunchStepKey::FirstMessage,
        LaunchStepStatus::Failed,
        "The first message could not be delivered",
        &at,
    )?);
    launch.transition(LaunchState::FailedTerminal, at)?;
    events.extend([
        LaunchEventKind::StateChanged {
            from: LaunchState::MessagePending,
            to: LaunchState::FailedTerminal,
        },
        LaunchEventKind::Failed { failure },
    ]);
    Ok(())
}

fn resume_in_place(
    launch: &mut Launch,
    at: String,
    events: &mut Vec<LaunchEventKind>,
) -> Result<(), TriggerError> {
    launch.blocked_reason = None;
    events.push(LaunchEventKind::Resumed {
        reason: LaunchBlockedReason::RecoveryRequired,
    });
    finish_in_place(launch, at)
}

fn schedule_without_lease(
    launch: &mut Launch,
    step: LaunchWorkStep,
    ready_at: String,
) -> Result<(), TriggerError> {
    if launch.work.lease.is_some() || launch.work.effect.is_some() {
        return Err(progress_error());
    }
    launch.work.step = step;
    launch.work.ready_at = ready_at;
    launch.work.attempts = 0;
    Ok(())
}

fn finish_in_place(launch: &mut Launch, at: String) -> Result<(), TriggerError> {
    launch.updated_at = at;
    launch.bump_revision()
}

fn require_retry_time(at: &str, retry_at: &str) -> Result<(), TriggerError> {
    match (seconds_from_rfc3339(at), seconds_from_rfc3339(retry_at)) {
        (Some(command), Some(retry)) if retry >= command => Ok(()),
        _ => Err(progress_error()),
    }
}

fn message_work_step(step: LaunchWorkStep) -> bool {
    matches!(
        step,
        LaunchWorkStep::DeliverFirstMessage | LaunchWorkStep::WaitForFirstMessage
    )
}

pub(super) fn accepted(state: FirstMessageDelivery) -> bool {
    matches!(
        state,
        FirstMessageDelivery::WriteConfirmed
            | FirstMessageDelivery::ExecutionObserved
            | FirstMessageDelivery::Completed
    )
}

fn provisioning_progress_key(step: LaunchWorkStep) -> Result<LaunchStepKey, TriggerError> {
    match step {
        LaunchWorkStep::ResolveSandbox | LaunchWorkStep::WaitForSandbox => {
            Ok(LaunchStepKey::Sandbox)
        }
        LaunchWorkStep::ResolveWorkspace | LaunchWorkStep::WaitForWorkspace => {
            Ok(LaunchStepKey::Workspace)
        }
        LaunchWorkStep::ResolveAgent | LaunchWorkStep::WaitForAgent => Ok(LaunchStepKey::Agent),
        LaunchWorkStep::DeliverFirstMessage
        | LaunchWorkStep::WaitForFirstMessage
        | LaunchWorkStep::Finish
        | LaunchWorkStep::None => Err(progress_error()),
    }
}

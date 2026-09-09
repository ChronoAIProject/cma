use crate::sandbox::AgentStatus;
use crate::{seconds_from_rfc3339, AgentId, ServiceId};

use super::launch_progress::{progress_error, require_command_time, settle_worker_command};
use super::{
    Launch, LaunchBlockedReason, LaunchEventKind, LaunchFailure, LaunchState, LaunchStepKey,
    LaunchStepStatus, LaunchWorkStep, ServiceRequirementState, TriggerError,
    MAX_LAUNCH_RETRY_DELAY_SECONDS, MAX_LAUNCH_WORK_ATTEMPTS,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchRequirementRefresh {
    pub service: ServiceId,
    pub state: ServiceRequirementState,
    pub checked_at: String,
}

impl Launch {
    pub fn defer_worker_for_checkpoint(
        &mut self,
        sandbox: &crate::sandbox::Sandbox,
        holder: &str,
        fence: u64,
        retry_at: String,
        at: String,
    ) -> Result<Vec<LaunchEventKind>, TriggerError> {
        let command_at = at.clone();
        let original = self.clone();
        require_command_time(&original, &command_at)?;
        require_bounded_retry_time(&command_at, &retry_at)?;
        let result = (|| {
            self.require_worker(holder, fence, self.work.step)?;
            if self.resolution.sandbox.id() != Some(&sandbox.id)
                || self.owner != sandbox.owner
                || sandbox.status != crate::sandbox::SandboxStatus::Active
                || sandbox.lifecycle.checkpoint_capture.is_none()
                || sandbox.validate().is_err()
                || !matches!(
                    self.work.step,
                    LaunchWorkStep::ResolveWorkspace
                        | LaunchWorkStep::WaitForWorkspace
                        | LaunchWorkStep::ResolveAgent
                )
                || self
                    .work
                    .effect
                    .as_ref()
                    .is_some_and(|effect| effect.state == super::LaunchEffectState::Applied)
            {
                return Err(progress_error());
            }
            if self
                .work
                .effect
                .as_ref()
                .is_some_and(|effect| effect.state == super::LaunchEffectState::Executing)
            {
                self.work
                    .mark_effect_recovery_required(holder, fence, at.clone())
                    .map_err(|_| progress_error())?;
            }
            // The effect identity and recovery debt survive the wait; no attempt is spent.
            self.work.ready_at = retry_at.clone();
            self.work.lease = None;
            let step = self.set_step(
                worker_step_key(self.work.step)?,
                LaunchStepStatus::Running,
                "Waiting for Sandbox checkpoint",
                &at,
            )?;
            finish_in_place(self, at)?;
            Ok(vec![step, LaunchEventKind::Deferred { retry_at }])
        })();
        settle_worker_command(self, original, &command_at, result)
    }

    pub fn defer_worker_launch(
        &mut self,
        holder: &str,
        fence: u64,
        retry_at: String,
        at: String,
    ) -> Result<Vec<LaunchEventKind>, TriggerError> {
        let command_at = at.clone();
        let original = self.clone();
        require_command_time(&original, &command_at)?;
        require_bounded_retry_time(&command_at, &retry_at)?;
        let result = (|| {
            self.require_worker(holder, fence, self.work.step)?;
            self.work
                .defer(holder, fence, retry_at.clone())
                .map_err(|_| progress_error())?;
            finish_in_place(self, at)?;
            Ok(vec![LaunchEventKind::Deferred { retry_at }])
        })();
        settle_worker_command(self, original, &command_at, result)
    }

    pub fn block_worker_launch(
        &mut self,
        holder: &str,
        fence: u64,
        reason: LaunchBlockedReason,
        requirement: Option<LaunchRequirementRefresh>,
        retry_at: String,
        at: String,
    ) -> Result<Vec<LaunchEventKind>, TriggerError> {
        let command_at = at.clone();
        let original = self.clone();
        require_command_time(&original, &command_at)?;
        require_bounded_retry_time(&command_at, &retry_at)?;
        let result = (|| {
            require_wakeable_claim(self, holder, fence)?;
            if !reason.is_wakeable() || self.state == LaunchState::Ready {
                return Err(progress_error());
            }
            let mut events = requirement_event(self, &reason, requirement, false, &at)?
                .into_iter()
                .collect::<Vec<_>>();
            events.extend(self.sync_requirement_step(&at)?);
            let step = self.work.step;
            self.work
                .defer(holder, fence, retry_at.clone())
                .map_err(|_| progress_error())?;
            self.blocked_reason = Some(reason.clone());
            events.push(self.set_step(
                worker_step_key(step)?,
                LaunchStepStatus::Blocked,
                blocked_detail(&reason),
                &at,
            )?);
            finish_in_place(self, at)?;
            events.extend([
                LaunchEventKind::Blocked { reason },
                LaunchEventKind::Deferred { retry_at },
            ]);
            Ok(events)
        })();
        settle_worker_command(self, original, &command_at, result)
    }

    pub fn resume_worker_launch(
        &mut self,
        holder: &str,
        fence: u64,
        expected_reason: LaunchBlockedReason,
        requirement: Option<LaunchRequirementRefresh>,
        at: String,
    ) -> Result<Vec<LaunchEventKind>, TriggerError> {
        let command_at = at.clone();
        let original = self.clone();
        require_command_time(&original, &command_at)?;
        let result = (|| {
            require_wakeable_claim(self, holder, fence)?;
            if self.blocked_reason.as_ref() != Some(&expected_reason)
                || !expected_reason.is_wakeable()
            {
                return Err(progress_error());
            }
            let mut events = requirement_event(self, &expected_reason, requirement, true, &at)?
                .into_iter()
                .collect::<Vec<_>>();
            events.extend(self.sync_requirement_step(&at)?);
            self.blocked_reason = None;
            events.push(self.set_step(
                worker_step_key(self.work.step)?,
                LaunchStepStatus::Running,
                resumed_detail(self.work.step),
                &at,
            )?);
            finish_in_place(self, at)?;
            events.push(LaunchEventKind::Resumed {
                reason: expected_reason,
            });
            Ok(events)
        })();
        settle_worker_command(self, original, &command_at, result)
    }

    pub fn retry_worker_launch(
        &mut self,
        holder: &str,
        fence: u64,
        code: String,
        safe_detail: String,
        retry_at: String,
        at: String,
    ) -> Result<Vec<LaunchEventKind>, TriggerError> {
        let command_at = at.clone();
        let original = self.clone();
        require_command_time(&original, &command_at)?;
        require_bounded_retry_time(&command_at, &retry_at)?;
        let result = (|| {
            self.require_worker(holder, fence, self.work.step)?;
            if self.state == LaunchState::Ready {
                return Err(progress_error());
            }
            if self
                .work
                .effect
                .as_ref()
                .is_some_and(|effect| effect.state == super::LaunchEffectState::Executing)
            {
                self.work
                    .mark_effect_recovery_required(holder, fence, at.clone())
                    .map_err(|_| progress_error())?;
            }
            let failure = worker_failure(code, safe_detail, &self.created_at, &at)?;
            let attempt = self
                .work
                .attempts
                .checked_add(1)
                .ok_or_else(progress_error)?;
            if attempt >= MAX_LAUNCH_WORK_ATTEMPTS {
                return fail_in_place(self, failure, at);
            }
            self.work
                .retry(holder, fence, retry_at.clone())
                .map_err(|_| progress_error())?;
            let step = self.set_step(
                worker_step_key(self.work.step)?,
                LaunchStepStatus::Retryable,
                "Launch work will be retried",
                &at,
            )?;
            finish_in_place(self, at)?;
            Ok(vec![
                step,
                LaunchEventKind::RetryScheduled { attempt, retry_at },
            ])
        })();
        settle_worker_command(self, original, &command_at, result)
    }

    pub fn fail_worker_launch(
        &mut self,
        holder: &str,
        fence: u64,
        code: String,
        safe_detail: String,
        at: String,
    ) -> Result<Vec<LaunchEventKind>, TriggerError> {
        let command_at = at.clone();
        let original = self.clone();
        require_command_time(&original, &command_at)?;
        let result = (|| {
            self.require_worker(holder, fence, self.work.step)?;
            if self.state == LaunchState::Ready {
                return Err(progress_error());
            }
            let failure = worker_failure(code, safe_detail, &self.created_at, &at)?;
            fail_in_place(self, failure, at)
        })();
        settle_worker_command(self, original, &command_at, result)
    }

    pub fn complete_worker_launch(
        &mut self,
        holder: &str,
        fence: u64,
        agent_id: &AgentId,
        status: AgentStatus,
        at: String,
    ) -> Result<Vec<LaunchEventKind>, TriggerError> {
        let command_at = at.clone();
        let original = self.clone();
        require_command_time(&original, &command_at)?;
        let result = (|| {
            self.require_worker(holder, fence, LaunchWorkStep::Finish)?;
            if self.state != LaunchState::Ready
                || self.resolution.agent.id() != Some(agent_id)
                || !matches!(status, AgentStatus::Stopped | AgentStatus::Deleted)
            {
                return Err(progress_error());
            }
            self.transition(LaunchState::Completed, at)?;
            self.validate()?;
            Ok(vec![
                LaunchEventKind::StateChanged {
                    from: LaunchState::Ready,
                    to: LaunchState::Completed,
                },
                LaunchEventKind::Completed,
            ])
        })();
        settle_worker_command(self, original, &command_at, result)
    }
}

fn require_wakeable_claim(launch: &Launch, holder: &str, fence: u64) -> Result<(), TriggerError> {
    if launch.state.is_terminal()
        || !launch.work.step.is_pending()
        || !launch.work.owns(holder, fence)
        || launch
            .blocked_reason
            .as_ref()
            .is_some_and(|reason| !reason.is_wakeable())
    {
        return Err(progress_error());
    }
    Ok(())
}

fn requirement_event(
    launch: &mut Launch,
    reason: &LaunchBlockedReason,
    refresh: Option<LaunchRequirementRefresh>,
    resuming: bool,
    at: &str,
) -> Result<Option<LaunchEventKind>, TriggerError> {
    let service = match reason {
        LaunchBlockedReason::ConnectionRequired { service } => service,
        _ if refresh.is_none() => return Ok(None),
        _ => return Err(progress_error()),
    };
    let Some(refresh) = refresh else {
        return Ok(None);
    };
    if &refresh.service != service
        || resuming != (refresh.state == ServiceRequirementState::Satisfied)
    {
        return Err(progress_error());
    }
    launch
        .requirements
        .validate()
        .map_err(|_| progress_error())?;
    let prior_check = seconds_from_rfc3339(&launch.requirements.checked_at);
    let refreshed_check = seconds_from_rfc3339(&refresh.checked_at);
    let command_time = seconds_from_rfc3339(at);
    if prior_check.is_none()
        || refreshed_check.is_none()
        || refreshed_check < prior_check
        || refreshed_check > command_time
    {
        return Err(progress_error());
    }
    let mut matches = launch
        .requirements
        .services
        .iter_mut()
        .filter(|requirement| requirement.service == refresh.service);
    let requirement = matches.next().ok_or_else(progress_error)?;
    if matches.next().is_some() {
        return Err(progress_error());
    }
    requirement.state = refresh.state;
    launch.requirements.checked_at = refresh.checked_at;
    Ok(Some(LaunchEventKind::RequirementChanged {
        service: refresh.service,
        state: refresh.state,
    }))
}

fn worker_failure(
    code: String,
    safe_detail: String,
    created_at: &str,
    at: &str,
) -> Result<LaunchFailure, TriggerError> {
    let failure = LaunchFailure {
        code,
        safe_detail,
        retryable: false,
        occurred_at: at.to_owned(),
    };
    failure.validate(created_at, at)?;
    Ok(failure)
}

fn fail_in_place(
    launch: &mut Launch,
    failure: LaunchFailure,
    at: String,
) -> Result<Vec<LaunchEventKind>, TriggerError> {
    let from = launch.state;
    let step = launch.set_step(
        worker_step_key(launch.work.step)?,
        LaunchStepStatus::Failed,
        &failure.safe_detail,
        &at,
    )?;
    launch.failure = Some(failure.clone());
    launch.transition(LaunchState::FailedTerminal, at)?;
    launch.validate()?;
    Ok(vec![
        step,
        LaunchEventKind::StateChanged {
            from,
            to: LaunchState::FailedTerminal,
        },
        LaunchEventKind::Failed { failure },
    ])
}

pub(super) fn finish_in_place(launch: &mut Launch, at: String) -> Result<(), TriggerError> {
    launch.updated_at = at;
    launch.bump_revision()?;
    launch.validate()
}

pub(super) fn require_bounded_retry_time(at: &str, retry_at: &str) -> Result<(), TriggerError> {
    match (seconds_from_rfc3339(at), seconds_from_rfc3339(retry_at)) {
        (Some(command), Some(retry))
            if retry >= command
                && retry <= command.saturating_add(MAX_LAUNCH_RETRY_DELAY_SECONDS) =>
        {
            Ok(())
        }
        _ => Err(progress_error()),
    }
}

fn worker_step_key(step: LaunchWorkStep) -> Result<LaunchStepKey, TriggerError> {
    match step {
        LaunchWorkStep::ResolveSandbox | LaunchWorkStep::WaitForSandbox => {
            Ok(LaunchStepKey::Sandbox)
        }
        LaunchWorkStep::ResolveWorkspace | LaunchWorkStep::WaitForWorkspace => {
            Ok(LaunchStepKey::Workspace)
        }
        LaunchWorkStep::ResolveAgent | LaunchWorkStep::WaitForAgent => Ok(LaunchStepKey::Agent),
        LaunchWorkStep::DeliverFirstMessage | LaunchWorkStep::WaitForFirstMessage => {
            Ok(LaunchStepKey::FirstMessage)
        }
        LaunchWorkStep::Finish => Ok(LaunchStepKey::Ready),
        LaunchWorkStep::None => Err(progress_error()),
    }
}

fn blocked_detail(reason: &LaunchBlockedReason) -> &'static str {
    match reason {
        LaunchBlockedReason::ConnectionRequired { .. } => "Waiting for a service connection",
        LaunchBlockedReason::OrchestrationPaused => "Launch orchestration is paused",
        LaunchBlockedReason::SandboxPausedReviving => "Waiting for the sandbox to revive",
        LaunchBlockedReason::RecoveryRequired => "Launch recovery is required",
    }
}

fn resumed_detail(step: LaunchWorkStep) -> &'static str {
    match step {
        LaunchWorkStep::ResolveSandbox => "Resolving sandbox",
        LaunchWorkStep::WaitForSandbox => "Preparing sandbox",
        LaunchWorkStep::ResolveWorkspace => "Resolving workspace",
        LaunchWorkStep::WaitForWorkspace => "Preparing workspace",
        LaunchWorkStep::ResolveAgent => "Resolving agent",
        LaunchWorkStep::WaitForAgent => "Starting agent",
        LaunchWorkStep::DeliverFirstMessage | LaunchWorkStep::WaitForFirstMessage => {
            "Sending the first message"
        }
        LaunchWorkStep::Finish => "Finishing launch",
        LaunchWorkStep::None => "",
    }
}

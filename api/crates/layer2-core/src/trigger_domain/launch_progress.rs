use crate::{seconds_from_rfc3339, AgentId, SandboxId, WorkspaceId};

use super::{
    Launch, LaunchEffectKind, LaunchEffectState, LaunchEventKind, LaunchState, LaunchStepKey,
    LaunchStepStatus, LaunchWorkStep, ResolvedResource, TriggerError,
};

impl Launch {
    pub fn prepare_worker_effect(
        &mut self,
        holder: &str,
        fence: u64,
        kind: LaunchEffectKind,
        at: String,
    ) -> Result<Vec<LaunchEventKind>, TriggerError> {
        let command_at = at.clone();
        let original = self.clone();
        require_command_time(&original, &command_at)?;
        let result = (|| {
            self.require_worker(holder, fence, self.work.step)?;
            let intent_key = kind.intent_key(&self.id);
            self.work
                .prepare_effect(holder, fence, kind, intent_key, at.clone())?;
            let event = self.set_step(
                progress_step(kind),
                LaunchStepStatus::Running,
                running_detail(kind),
                &at,
            )?;
            self.updated_at = at;
            self.bump_revision()?;
            self.validate()?;
            Ok(vec![event])
        })();
        settle_worker_command(self, original, &command_at, result)
    }

    pub fn mark_worker_effect_executing(
        &mut self,
        holder: &str,
        fence: u64,
        at: String,
    ) -> Result<(), TriggerError> {
        let command_at = at.clone();
        let original = self.clone();
        require_command_time(&original, &command_at)?;
        let result = (|| {
            self.require_worker(holder, fence, self.work.step)?;
            self.work.mark_effect_executing(holder, fence, at.clone())?;
            self.updated_at = at;
            self.bump_revision()?;
            self.validate()
        })();
        settle_worker_command(self, original, &command_at, result)
    }

    pub fn mark_worker_effect_applied(
        &mut self,
        holder: &str,
        fence: u64,
        result_id: String,
        at: String,
    ) -> Result<(), TriggerError> {
        let command_at = at.clone();
        let original = self.clone();
        require_command_time(&original, &command_at)?;
        let result = (|| {
            self.require_worker(holder, fence, self.work.step)?;
            self.work
                .mark_effect_applied(holder, fence, result_id, at.clone())?;
            self.updated_at = at;
            self.bump_revision()?;
            self.validate()
        })();
        settle_worker_command(self, original, &command_at, result)
    }

    pub fn record_worker_sandbox(
        &mut self,
        holder: &str,
        fence: u64,
        sandbox_id: SandboxId,
        reused: bool,
        at: String,
    ) -> Result<Vec<LaunchEventKind>, TriggerError> {
        let command_at = at.clone();
        let original = self.clone();
        require_command_time(&original, &command_at)?;
        let result = (|| {
            self.require_worker(holder, fence, LaunchWorkStep::ResolveSandbox)?;
            if self.state != LaunchState::Resolving {
                return Err(progress_error());
            }
            require_effect_result(
                self,
                &sandbox_id.to_string(),
                reused,
                &[
                    LaunchEffectKind::CreateSandbox,
                    LaunchEffectKind::ReviveSandbox,
                ],
            )?;
            let desired = resolved(sandbox_id.clone(), reused);
            let newly_resolved = set_resolution(&mut self.resolution.sandbox, desired)?;
            self.work
                .advance(holder, fence, LaunchWorkStep::WaitForSandbox, at.clone())?;
            let step = self.set_step(
                LaunchStepKey::Sandbox,
                LaunchStepStatus::Running,
                "Preparing sandbox",
                &at,
            )?;
            self.transition(LaunchState::SandboxPending, at)?;
            self.validate()?;
            let mut events = Vec::with_capacity(3);
            if newly_resolved {
                events.push(LaunchEventKind::SandboxResolved { sandbox_id, reused });
            }
            events.push(LaunchEventKind::StateChanged {
                from: LaunchState::Resolving,
                to: LaunchState::SandboxPending,
            });
            events.push(step);
            Ok(events)
        })();
        settle_worker_command(self, original, &command_at, result)
    }

    pub fn record_worker_sandbox_ready(
        &mut self,
        holder: &str,
        fence: u64,
        discovered_workspace: Option<WorkspaceId>,
        at: String,
    ) -> Result<Vec<LaunchEventKind>, TriggerError> {
        let command_at = at.clone();
        let original = self.clone();
        require_command_time(&original, &command_at)?;
        let result = (|| {
            self.require_worker(holder, fence, LaunchWorkStep::WaitForSandbox)?;
            if self.state != LaunchState::SandboxPending || self.resolution.sandbox.is_pending() {
                return Err(progress_error());
            }
            let mut events = Vec::with_capacity(4);
            if let Some(workspace_id) = discovered_workspace {
                let newly_resolved = set_resolution(
                    &mut self.resolution.workspace,
                    ResolvedResource::Reused(workspace_id.clone()),
                )?;
                if newly_resolved {
                    events.push(LaunchEventKind::WorkspaceResolved {
                        workspace_id,
                        reused: true,
                    });
                }
            }
            self.work
                .advance(holder, fence, LaunchWorkStep::ResolveWorkspace, at.clone())?;
            let step = self.set_step(
                LaunchStepKey::Sandbox,
                LaunchStepStatus::Completed,
                "Sandbox is ready",
                &at,
            )?;
            self.transition(LaunchState::WorkspacePending, at)?;
            self.validate()?;
            events.push(LaunchEventKind::StateChanged {
                from: LaunchState::SandboxPending,
                to: LaunchState::WorkspacePending,
            });
            events.push(step);
            Ok(events)
        })();
        settle_worker_command(self, original, &command_at, result)
    }

    pub fn record_worker_workspace(
        &mut self,
        holder: &str,
        fence: u64,
        workspace_id: WorkspaceId,
        reused: bool,
        at: String,
    ) -> Result<Vec<LaunchEventKind>, TriggerError> {
        let command_at = at.clone();
        let original = self.clone();
        require_command_time(&original, &command_at)?;
        let result = (|| {
            self.require_worker(holder, fence, LaunchWorkStep::ResolveWorkspace)?;
            if self.state != LaunchState::WorkspacePending {
                return Err(progress_error());
            }
            require_effect_result(
                self,
                &workspace_id.to_string(),
                reused,
                &[LaunchEffectKind::CreateWorkspace],
            )?;
            let desired = resolved(workspace_id.clone(), reused);
            let newly_resolved = set_resolution(&mut self.resolution.workspace, desired)?;
            self.work
                .advance(holder, fence, LaunchWorkStep::WaitForWorkspace, at.clone())?;
            let step = self.set_step(
                LaunchStepKey::Workspace,
                LaunchStepStatus::Running,
                "Preparing workspace",
                &at,
            )?;
            self.updated_at = at;
            self.bump_revision()?;
            self.validate()?;
            let mut events = Vec::with_capacity(2);
            if newly_resolved {
                events.push(LaunchEventKind::WorkspaceResolved {
                    workspace_id,
                    reused,
                });
            }
            events.push(step);
            Ok(events)
        })();
        settle_worker_command(self, original, &command_at, result)
    }

    pub fn record_worker_workspace_ready(
        &mut self,
        holder: &str,
        fence: u64,
        at: String,
    ) -> Result<Vec<LaunchEventKind>, TriggerError> {
        let command_at = at.clone();
        let original = self.clone();
        require_command_time(&original, &command_at)?;
        let result = (|| {
            self.require_worker(holder, fence, LaunchWorkStep::WaitForWorkspace)?;
            if self.state != LaunchState::WorkspacePending || self.resolution.workspace.is_pending()
            {
                return Err(progress_error());
            }
            self.work
                .advance(holder, fence, LaunchWorkStep::ResolveAgent, at.clone())?;
            let step = self.set_step(
                LaunchStepKey::Workspace,
                LaunchStepStatus::Completed,
                "Workspace is ready",
                &at,
            )?;
            self.transition(LaunchState::AgentPending, at)?;
            self.validate()?;
            Ok(vec![
                LaunchEventKind::StateChanged {
                    from: LaunchState::WorkspacePending,
                    to: LaunchState::AgentPending,
                },
                step,
            ])
        })();
        settle_worker_command(self, original, &command_at, result)
    }

    pub fn record_worker_agent(
        &mut self,
        holder: &str,
        fence: u64,
        agent_id: AgentId,
        reused: bool,
        at: String,
    ) -> Result<Vec<LaunchEventKind>, TriggerError> {
        let command_at = at.clone();
        let original = self.clone();
        require_command_time(&original, &command_at)?;
        let result = (|| {
            self.require_worker(holder, fence, LaunchWorkStep::ResolveAgent)?;
            if self.state != LaunchState::AgentPending {
                return Err(progress_error());
            }
            require_effect_result(
                self,
                &agent_id.to_string(),
                reused,
                &[LaunchEffectKind::CreateAgent],
            )?;
            let desired = resolved(agent_id.clone(), reused);
            let newly_resolved = set_resolution(&mut self.resolution.agent, desired)?;
            self.work
                .advance(holder, fence, LaunchWorkStep::WaitForAgent, at.clone())?;
            let step = self.set_step(
                LaunchStepKey::Agent,
                LaunchStepStatus::Running,
                "Starting agent",
                &at,
            )?;
            self.updated_at = at;
            self.bump_revision()?;
            self.validate()?;
            let mut events = Vec::with_capacity(2);
            if newly_resolved {
                events.push(LaunchEventKind::AgentResolved { agent_id, reused });
            }
            events.push(step);
            Ok(events)
        })();
        settle_worker_command(self, original, &command_at, result)
    }

    pub(super) fn require_worker(
        &self,
        holder: &str,
        fence: u64,
        step: LaunchWorkStep,
    ) -> Result<(), TriggerError> {
        if self.state.is_terminal()
            || self.blocked_reason.is_some()
            || self.work.step != step
            || !self.work.owns(holder, fence)
        {
            return Err(progress_error());
        }
        Ok(())
    }

    pub(super) fn set_step(
        &mut self,
        key: LaunchStepKey,
        status: LaunchStepStatus,
        detail: &str,
        at: &str,
    ) -> Result<LaunchEventKind, TriggerError> {
        let step = self
            .steps
            .iter_mut()
            .find(|step| step.key == key)
            .ok_or_else(progress_error)?;
        step.status = status;
        step.safe_detail = detail.to_string();
        if status != LaunchStepStatus::Pending {
            step.started_at.get_or_insert_with(|| at.to_string());
        }
        step.completed_at = (status == LaunchStepStatus::Completed).then(|| at.to_string());
        Ok(LaunchEventKind::StepChanged { step: step.clone() })
    }
}

fn progress_step(kind: LaunchEffectKind) -> LaunchStepKey {
    match kind {
        LaunchEffectKind::CreateSandbox | LaunchEffectKind::ReviveSandbox => LaunchStepKey::Sandbox,
        LaunchEffectKind::CreateWorkspace => LaunchStepKey::Workspace,
        LaunchEffectKind::CreateAgent => LaunchStepKey::Agent,
    }
}

fn running_detail(kind: LaunchEffectKind) -> &'static str {
    match kind {
        LaunchEffectKind::CreateSandbox => "Creating sandbox",
        LaunchEffectKind::ReviveSandbox => "Reviving sandbox",
        LaunchEffectKind::CreateWorkspace => "Creating workspace",
        LaunchEffectKind::CreateAgent => "Creating agent",
    }
}

fn require_effect_result(
    launch: &Launch,
    result_id: &str,
    reused: bool,
    allowed: &[LaunchEffectKind],
) -> Result<(), TriggerError> {
    match launch.work.effect.as_ref() {
        None if reused => Ok(()),
        Some(effect)
            if effect.state == LaunchEffectState::Applied
                && effect.result_id.as_deref() == Some(result_id)
                && allowed.contains(&effect.kind) =>
        {
            Ok(())
        }
        _ => Err(progress_error()),
    }
}

fn resolved<T>(id: T, reused: bool) -> ResolvedResource<T> {
    if reused {
        ResolvedResource::Reused(id)
    } else {
        ResolvedResource::Created(id)
    }
}

fn set_resolution<T: PartialEq>(
    current: &mut ResolvedResource<T>,
    desired: ResolvedResource<T>,
) -> Result<bool, TriggerError> {
    if current.is_pending() {
        *current = desired;
        Ok(true)
    } else if *current == desired {
        Ok(false)
    } else {
        Err(progress_error())
    }
}

pub(super) fn progress_error() -> TriggerError {
    TriggerError::new("invalid_launch_worker_progress", "work")
}

pub(super) fn require_command_time(launch: &Launch, at: &str) -> Result<(), TriggerError> {
    match (
        seconds_from_rfc3339(&launch.updated_at),
        seconds_from_rfc3339(at),
    ) {
        (Some(current), Some(next)) if next >= current => Ok(()),
        _ => Err(progress_error()),
    }
}

pub(super) fn settle_worker_command<T>(
    launch: &mut Launch,
    original: Launch,
    at: &str,
    result: Result<T, TriggerError>,
) -> Result<T, TriggerError> {
    let expected_revision = original.revision.checked_add(1);
    match result {
        Ok(value)
            if expected_revision == Some(launch.revision)
                && launch.updated_at == at
                && launch.validate().is_ok() =>
        {
            Ok(value)
        }
        Ok(_) => {
            *launch = original;
            Err(progress_error())
        }
        Err(error) => {
            *launch = original;
            Err(error)
        }
    }
}

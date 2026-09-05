use crate::{SandboxId, WorkspaceId};

use super::launch_progress::{progress_error, require_command_time, settle_worker_command};
use super::launch_worker_control::finish_in_place;
use super::{
    Launch, LaunchEffectKind, LaunchEffectState, LaunchEventKind, LaunchState, LaunchStepKey,
    LaunchStepStatus, LaunchWorkStep, ResolvedResource, TriggerError,
};

impl Launch {
    pub fn reconcile_worker_effect(
        &mut self,
        holder: &str,
        fence: u64,
        kind: LaunchEffectKind,
        result_id: String,
        at: String,
    ) -> Result<Vec<LaunchEventKind>, TriggerError> {
        let command_at = at.clone();
        let original = self.clone();
        require_command_time(&original, &command_at)?;
        let result = (|| {
            self.require_worker(holder, fence, self.work.step)?;
            self.work
                .reconcile_effect_applied(holder, fence, kind, result_id.clone(), at.clone())
                .map_err(|_| progress_error())?;
            finish_in_place(self, at)?;
            Ok(vec![LaunchEventKind::EffectReconciled { kind, result_id }])
        })();
        settle_worker_command(self, original, &command_at, result)
    }

    pub fn replace_worker_reused_sandbox(
        &mut self,
        holder: &str,
        fence: u64,
        previous_id: &SandboxId,
        sandbox_id: SandboxId,
        at: String,
    ) -> Result<Vec<LaunchEventKind>, TriggerError> {
        let command_at = at.clone();
        let original = self.clone();
        require_command_time(&original, &command_at)?;
        let result = (|| {
            self.require_worker(holder, fence, LaunchWorkStep::WaitForSandbox)?;
            if self.state != LaunchState::SandboxPending
                || previous_id == &sandbox_id
                || self.resolution.sandbox != ResolvedResource::Reused(previous_id.clone())
                || !applied_result(self, LaunchEffectKind::CreateSandbox, sandbox_id.as_str())
            {
                return Err(progress_error());
            }
            self.resolution.sandbox = ResolvedResource::Created(sandbox_id.clone());
            self.resolution.workspace = ResolvedResource::Pending;
            self.resolution.agent = ResolvedResource::Pending;
            self.resolution.choice_required = false;
            self.work
                .advance(holder, fence, LaunchWorkStep::WaitForSandbox, at.clone())?;
            let step = self.set_step(
                LaunchStepKey::Sandbox,
                LaunchStepStatus::Running,
                "Preparing replacement sandbox",
                &at,
            )?;
            finish_in_place(self, at)?;
            Ok(vec![
                LaunchEventKind::SandboxReplaced {
                    previous_sandbox_id: previous_id.clone(),
                    sandbox_id,
                },
                step,
            ])
        })();
        settle_worker_command(self, original, &command_at, result)
    }

    pub fn replace_worker_reused_workspace(
        &mut self,
        holder: &str,
        fence: u64,
        previous_id: &WorkspaceId,
        workspace_id: WorkspaceId,
        at: String,
    ) -> Result<Vec<LaunchEventKind>, TriggerError> {
        let command_at = at.clone();
        let original = self.clone();
        require_command_time(&original, &command_at)?;
        let result = (|| {
            self.require_worker(holder, fence, LaunchWorkStep::WaitForWorkspace)?;
            if self.state != LaunchState::WorkspacePending
                || previous_id == &workspace_id
                || self.resolution.workspace != ResolvedResource::Reused(previous_id.clone())
                || !applied_result(
                    self,
                    LaunchEffectKind::CreateWorkspace,
                    workspace_id.as_str(),
                )
            {
                return Err(progress_error());
            }
            self.resolution.workspace = ResolvedResource::Created(workspace_id.clone());
            self.resolution.agent = ResolvedResource::Pending;
            self.resolution.choice_required = false;
            self.work
                .advance(holder, fence, LaunchWorkStep::WaitForWorkspace, at.clone())?;
            let step = self.set_step(
                LaunchStepKey::Workspace,
                LaunchStepStatus::Running,
                "Preparing replacement workspace",
                &at,
            )?;
            finish_in_place(self, at)?;
            Ok(vec![
                LaunchEventKind::WorkspaceReplaced {
                    previous_workspace_id: previous_id.clone(),
                    workspace_id,
                },
                step,
            ])
        })();
        settle_worker_command(self, original, &command_at, result)
    }
}

fn applied_result(launch: &Launch, kind: LaunchEffectKind, result_id: &str) -> bool {
    launch.work.effect.as_ref().is_some_and(|effect| {
        effect.kind == kind
            && effect.state == LaunchEffectState::Applied
            && effect.result_id.as_deref() == Some(result_id)
    })
}

//! Reconcile one atomic permanent exclusion plan before reconnect or Active.
use std::collections::BTreeSet;
use std::time::{Instant, SystemTime};

use super::super::{
    support::{adapter_failure, runtime, WorkerFailure},
    SandboxWorker,
};
use layer2_core::sandbox::{
    store::{RetiredRuntimeCleanupPlan, SandboxStore},
    Sandbox, SandboxWorkStep,
};
use layer3_adapter::{
    AgentCloseRequest, AgentHomeRemoveRequest, AgentSlot, WorkspaceRemovalOutcome,
    WorkspaceRemoveRequest, WorktreeRef,
};

impl SandboxWorker {
    pub(super) async fn reconcile_retired_runtime_paths(
        &self,
        sandbox: &Sandbox,
        now: SystemTime,
    ) -> Result<(), WorkerFailure> {
        let started = Instant::now();
        let plan = self
            .dependencies
            .store
            .retired_runtime_cleanup_plan(sandbox)
            .await
            .map_err(|_| held())?;
        validate_plan(&plan)?;
        if plan.agents.is_empty() && plan.workspaces.is_empty() {
            return Ok(());
        }
        let runtime = runtime(sandbox).ok_or_else(held)?;
        // A retained image that cannot provide current removal evidence must
        // hold the complete restore before the first child filesystem effect.
        let writer = if plan.workspaces.is_empty() {
            None
        } else {
            let support = self
                .dependencies
                .adapter
                .workspace_removal_support(&runtime)
                .await
                .map_err(adapter_failure)?;
            if support.protocol_version != layer3_adapter::WORKSPACE_REMOVAL_PROTOCOL
                || !support.runtime.installation_ready
                || !support.runtime.credential_ready
                || support.runtime.restore_pending
                || !sandbox
                    .runtime_process_matches(&support.runtime)
                    .map_err(|_| held())?
                || sandbox
                    .runtime
                    .as_ref()
                    .and_then(|binding| binding.boot_id.as_ref())
                    != Some(&support.runtime.boot_id)
            {
                return Err(held());
            }
            Some(support.runtime)
        };
        let removed = plan
            .workspaces
            .iter()
            .map(|path| &path.workspace_id)
            .collect::<BTreeSet<_>>();
        for path in &plan.agents {
            self.require_cleanup_plan_current(sandbox, &plan, now, started)
                .await?;
            self.dependencies
                .registry
                .fence(&path.agent_id, None, path.authority_fence);
            self.dependencies
                .adapter
                .close_agent(
                    &runtime,
                    path.authority_fence,
                    &AgentCloseRequest::new(
                        format!("agent:{}:close:v1", path.agent_id),
                        AgentSlot::new(path.agent_id.to_string()),
                    ),
                )
                .await
                .map_err(adapter_failure)?;
        }
        for path in &plan.agents {
            self.require_cleanup_plan_current(sandbox, &plan, now, started)
                .await?;
            self.dependencies
                .adapter
                .remove_agent_home(
                    &runtime,
                    path.authority_fence,
                    &AgentHomeRemoveRequest::new(
                        format!("agent:{}:home:remove:v1", path.agent_id),
                        AgentSlot::new(path.agent_id.to_string()),
                    ),
                )
                .await
                .map_err(adapter_failure)?;
        }
        for path in &plan.agents {
            if removed.contains(&path.workspace_id) {
                continue;
            }
            self.require_cleanup_plan_current(sandbox, &plan, now, started)
                .await?;
            self.dependencies
                .adapter
                .remove_worktree(
                    &runtime,
                    path.authority_fence,
                    &WorktreeRef::new(
                        format!(
                            "worktree:{}:{}:remove:v1",
                            path.workspace_name, path.worktree_name
                        ),
                        &path.workspace_name,
                        &path.worktree_name,
                    ),
                )
                .await
                .map_err(adapter_failure)?;
        }
        for path in &plan.workspaces {
            self.require_cleanup_plan_current(sandbox, &plan, now, started)
                .await?;
            let request = WorkspaceRemoveRequest::new(
                format!("workspace:{}:remove:v1", path.workspace_id),
                path.workspace_id.to_string(),
                &path.workspace_name,
            );
            let expected = writer.as_ref().ok_or_else(held)?;
            let observation = self
                .dependencies
                .adapter
                .reconcile_workspace_removal(&runtime, expected, &request)
                .await
                .map_err(adapter_failure)?;
            observation
                .validate_for(&layer3_adapter::WorkspaceRemovalRequest {
                    runtime: expected.clone(),
                    request,
                })
                .map_err(|_| held())?;
            if observation.outcome != WorkspaceRemovalOutcome::CurrentlyAbsent {
                return Err(held());
            }
        }
        self.require_cleanup_plan_current(sandbox, &plan, now, started)
            .await
    }

    async fn require_cleanup_plan_current(
        &self,
        sandbox: &Sandbox,
        plan: &RetiredRuntimeCleanupPlan,
        now: SystemTime,
        started: Instant,
    ) -> Result<(), WorkerFailure> {
        let stamp = layer2_core::rfc3339_at(now.checked_add(started.elapsed()).ok_or_else(held)?);
        if sandbox.work.step != SandboxWorkStep::RunLifecycleOperation
            || !matches!(
                sandbox.lifecycle.operation.as_ref(),
                Some(layer2_core::sandbox::SandboxLifecycleOperation::Revive {
                    phase: layer2_core::sandbox::SandboxRevivePhase::ConversationReopen,
                    ..
                }) | Some(
                    layer2_core::sandbox::SandboxLifecycleOperation::ImageRotation {
                        phase: layer2_core::sandbox::SandboxImageRotationPhase::ConversationReopen,
                        ..
                    }
                )
            )
            || sandbox.work.lease.as_ref().is_none_or(|lease| {
                lease.holder != self.config.holder
                    || (!layer2_core::sandbox::store::WorkAuthority::active()
                        && lease.expires_at <= stamp)
            })
            || layer2_core::sandbox::store::WorkAuthority::current().is_err()
            || SandboxStore::get_sandbox(self.dependencies.store.as_ref(), &sandbox.id)
                .await
                .map_err(|_| held())?
                .as_ref()
                .is_none_or(|current| {
                    !layer2_core::sandbox::store::sandbox_content_matches(Some(current), sandbox)
                })
            || self
                .dependencies
                .store
                .retired_runtime_cleanup_plan(sandbox)
                .await
                .map_err(|_| held())?
                != *plan
        {
            return Err(held());
        }
        Ok(())
    }
}

fn validate_plan(plan: &RetiredRuntimeCleanupPlan) -> Result<(), WorkerFailure> {
    if plan.agents.len() > 256 || plan.workspaces.len() > 256 {
        return Err(held());
    }
    let mut agents = BTreeSet::new();
    let mut workspaces = BTreeSet::new();
    let mut names = BTreeSet::new();
    for path in &plan.agents {
        if !agents.insert(&path.agent_id) || path.authority_fence == 0 {
            return Err(held());
        }
    }
    for path in &plan.workspaces {
        let request = WorkspaceRemoveRequest::new(
            format!("workspace:{}:remove:v1", path.workspace_id),
            path.workspace_id.to_string(),
            &path.workspace_name,
        );
        if !workspaces.insert(&path.workspace_id)
            || !names.insert(&path.workspace_name)
            || path.removal_request_sha256 != request.request_sha256
            || !sha256(&path.cleanup_sha256)
            || path.cleanup_receipt_id.is_empty()
            || layer2_core::seconds_from_rfc3339(&path.workspace_created_at).is_none()
            || !plan
                .agents
                .iter()
                .any(|agent| agent.workspace_id == path.workspace_id)
            || plan.agents.iter().any(|agent| {
                (agent.workspace_id == path.workspace_id)
                    != (agent.workspace_name == path.workspace_name)
            })
        {
            return Err(held());
        }
    }
    Ok(())
}

fn sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn held() -> WorkerFailure {
    WorkerFailure::retryable("permanent runtime cleanup plan, current lifecycle authority or exact Workspace absence is unavailable")
}

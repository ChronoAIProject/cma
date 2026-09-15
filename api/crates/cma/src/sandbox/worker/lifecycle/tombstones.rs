//! Reconcile permanent child exclusions after archive/retained-home restore,
//! before any conversation is reconnected or the parent is published Active.
use super::super::{
    support::{adapter_failure, runtime, WorkerFailure},
    SandboxWorker,
};
use layer2_core::sandbox::Sandbox;
use layer3_adapter::{AgentCloseRequest, AgentHomeRemoveRequest, AgentSlot, WorktreeRef};

impl SandboxWorker {
    pub(super) async fn reconcile_retired_runtime_paths(
        &self,
        sandbox: &Sandbox,
    ) -> Result<(), WorkerFailure> {
        let paths = self
            .dependencies
            .store
            .retired_agent_runtime_paths(sandbox)
            .await
            .map_err(|_| {
                WorkerFailure::retryable("permanent child restore exclusions are unavailable")
            })?;
        if paths.is_empty() {
            return Ok(());
        }
        let runtime = runtime(sandbox)
            .ok_or_else(|| WorkerFailure::terminal_security("restore runtime is absent"))?;
        for path in paths {
            let slot = AgentSlot::new(path.agent_id.to_string());
            self.dependencies
                .registry
                .fence(&path.agent_id, None, path.authority_fence);
            self.dependencies
                .adapter
                .close_agent(
                    &runtime,
                    path.authority_fence,
                    &AgentCloseRequest::new(
                        format!("agent:{}:close:v1", slot.as_str()),
                        slot.clone(),
                    ),
                )
                .await
                .map_err(adapter_failure)?;
            self.dependencies
                .adapter
                .remove_agent_home(
                    &runtime,
                    path.authority_fence,
                    &AgentHomeRemoveRequest::new(
                        format!("agent:{}:home:remove:v1", slot.as_str()),
                        slot,
                    ),
                )
                .await
                .map_err(adapter_failure)?;
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
                        path.workspace_name,
                        path.worktree_name,
                    ),
                )
                .await
                .map_err(adapter_failure)?;
        }
        Ok(())
    }
}

//! Parent validation admits one inspection; independent reads may start earlier.
use std::future::Future;

use super::*;
use layer3_adapter::{InspectionAdmission, RuntimeStatus};
use tokio::sync::oneshot;

type Attachment = layer3_adapter::Result<(RuntimeHandle, Option<RuntimeStatus>)>;
type Read<T> = Result<Option<T>, SandboxStoreError>;
type Parents = (Sandbox, Workspace, Worktree);

impl AgentWorker {
    pub(super) async fn overlapped_open_context(
        &self,
        agent: &Agent,
    ) -> Result<AgentContext, WorkerFailure> {
        let started = tokio::time::Instant::now();
        let ((sandbox, workspace, worktree), attached) = read_and_inspect(
            agent,
            self.dependencies.store.get_sandbox(&agent.sandbox_id),
            self.dependencies.store.get_workspace(&agent.workspace_id),
            self.dependencies.store.get_worktree(&agent.worktree_id),
            |sandbox, admission| async move {
                let inspection = self.inspection_correlation(agent);
                let inspection_started = std::time::Instant::now();
                let attached = super::super::super::inspection_park::observe(
                    agent,
                    &sandbox,
                    None,
                    crate::sandbox::runtime_attachment::inspect_bound_runtime_after(
                        self.dependencies.adapter.as_ref(),
                        &sandbox,
                        &self.config.deployment_id,
                        &admission,
                    ),
                )
                .await;
                report_inspection(
                    &inspection,
                    agent.work.step,
                    inspection_started.elapsed(),
                    true,
                    attached.as_ref().err(),
                );
                attached
            },
        )
        .await?;
        self.observe_runtime_startup(&sandbox, started, &attached);
        let fields = self.inspection_correlation(agent);
        with_root(|| {
            tracing::info!(target: TARGET, parent: None,
                agent_sha256 = %fields.agent_sha256, sandbox_sha256 = fields.sandbox_sha256.as_deref(),
                instance_sha256 = %fields.instance_sha256, receiver_sha256 = fields.receiver_sha256.as_deref(),
                epoch = fields.epoch, elapsed_ms = started.elapsed().as_millis() as u64,
                succeeded = attached.is_ok(), parent_overlap = true,
                "Agent parent reads and runtime inspection completed");
        });
        let (runtime, status) = attached.map_err(adapter_failure)?;
        self.confirm_inspection(agent, &sandbox).await?;
        Ok(AgentContext {
            sandbox,
            workspace,
            worktree,
            runtime,
            agent_resource_preparation: status
                .is_some_and(|status| status.agent_resource_preparation),
        })
    }
}

async fn read_and_inspect<I, F>(
    agent: &Agent,
    sandbox: impl Future<Output = Read<Sandbox>>,
    workspace: impl Future<Output = Read<Workspace>>,
    worktree: impl Future<Output = Read<Worktree>>,
    inspect: I,
) -> Result<(Parents, Attachment), WorkerFailure>
where
    I: FnOnce(Sandbox, InspectionAdmission) -> F,
    F: Future<Output = Attachment>,
{
    let (observed, observation) = oneshot::channel();
    let (release, admission) = InspectionAdmission::pending();
    let parents = async {
        let (sandbox, workspace, worktree) = tokio::join!(
            async {
                let result = sandbox.await;
                if let Ok(Some(sandbox)) = &result {
                    if observed.send(sandbox.clone()).is_err() {
                        tracing::debug!("Runtime inspection no longer awaits parent observation");
                    }
                }
                result
            },
            workspace,
            worktree,
        );
        let sandbox = sandbox
            .map_err(parent_store_failure)?
            .ok_or_else(missing_parent)?;
        let workspace = workspace
            .map_err(parent_store_failure)?
            .ok_or_else(missing_parent)?;
        let worktree = worktree
            .map_err(parent_store_failure)?
            .ok_or_else(missing_parent)?;
        validate_parents(agent, &sandbox, &workspace, &worktree)?;
        release.allow();
        Ok((sandbox, workspace, worktree))
    };
    let inspection = async {
        let result = match observation.await {
            Ok(sandbox) => inspect(sandbox, admission).await,
            Err(_) => Err(AdapterError::Unavailable(
                "runtime parent is unavailable".into(),
            )),
        };
        // Preserve parent-error precedence even if preparation fails first.
        // Conversely, a parent failure cancels any still-pending preparation.
        Ok::<_, WorkerFailure>(result)
    };
    tokio::try_join!(parents, inspection)
}

#[cfg(test)]
#[path = "context_overlap_tests.rs"]
mod tests;

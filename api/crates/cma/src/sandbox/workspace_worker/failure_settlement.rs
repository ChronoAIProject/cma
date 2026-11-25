//! Publish failed Workspace preparation under the original work fence.
use super::*;
use layer2_core::sandbox::store::RepositoryPreparationRuntime;

impl WorkspaceWorker {
    pub(super) async fn handle_failure(
        &self,
        workspace: Workspace,
        failure: WorkerFailure,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        self.settle_failure(workspace, failure, now, None).await
    }

    pub(super) async fn handle_prepare_failure(
        &self,
        workspace: Workspace,
        failure: WorkerFailure,
        now: SystemTime,
        sandbox: &Sandbox,
    ) -> Result<(), SandboxStoreError> {
        let repository_runtime = sandbox
            .runtime
            .as_ref()
            .filter(|binding| {
                workspace.work.step == WorkspaceWorkStep::Prepare
                    && matches!(
                        workspace.source,
                        WorkspaceSource::RepositorySnapshot { .. }
                            | WorkspaceSource::PublicRepositorySnapshot { .. }
                    )
                    && !failure.retryable
                    && matches!(
                        failure.class,
                        "repository_connection_required"
                            | "repository_unavailable"
                            | "repository_ref_not_found"
                            | "repository_archive_unavailable"
                            | "repository_source_invalid"
                    )
                    && binding.provision_generation
                        == Some(sandbox.lifecycle.runtime_generation_high_water)
            })
            .map(|binding| {
                Box::new(RepositoryPreparationRuntime {
                    binding: binding.clone(),
                    generation: sandbox.lifecycle.runtime_generation_high_water,
                    key_generation: sandbox.credential.key_generation,
                })
            });
        self.settle_failure(workspace, failure, now, repository_runtime)
            .await
    }

    async fn settle_failure(
        &self,
        mut workspace: Workspace,
        failure: WorkerFailure,
        now: SystemTime,
        repository_runtime: Option<Box<RepositoryPreparationRuntime>>,
    ) -> Result<(), SandboxStoreError> {
        let expected = workspace.revision;
        let fence = claimed_fence(&workspace, &self.config.holder)?;
        let stamp = rfc3339_at(now);
        if failure.retryable {
            workspace
                .work
                .retry(
                    &self.config.holder,
                    fence,
                    rfc3339_at(now + self.config.retry_delay),
                )
                .map_err(|_| SandboxStoreError::Conflict)?;
            workspace.revision = expected.checked_add(1).ok_or(SandboxStoreError::Conflict)?;
            return self
                .dependencies
                .store
                .save_workspace(expected, workspace)
                .await;
        }
        let recovering_prepare = workspace.work.step == WorkspaceWorkStep::RecoverPrepare;
        workspace
            .work
            .advance(
                &self.config.holder,
                fence,
                WorkspaceWorkStep::None,
                stamp.clone(),
            )
            .map_err(|_| SandboxStoreError::Conflict)?;
        let typed = failure.typed();
        if recovering_prepare {
            let attempt = workspace
                .work
                .prepare_recovery
                .as_mut()
                .and_then(|recovery| recovery.attempts.last_mut())
                .ok_or(SandboxStoreError::WorkspaceRecoveryConflict)?;
            attempt.failed = Some(typed.clone());
        }
        workspace
            .transition(WorkspaceStatus::Failed, stamp.clone(), Some(typed.clone()))
            .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
        let id = workspace.id.clone();
        self.dependencies
            .store
            .commit(SandboxTransaction::SaveWorkspaceAndAppend {
                repository_runtime,
                expected_revision: expected,
                workspace: Box::new(workspace),
                event_at: stamp,
                event: SandboxEventKind::WorkspaceFailed {
                    workspace_id: id,
                    failure: typed,
                },
            })
            .await
    }
}

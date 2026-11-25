use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

mod authority;
mod config;
mod execute;

use layer2_core::sandbox::store::{SandboxStoreError, SandboxTransaction, WorkspaceLifecycleStore};
use layer2_core::sandbox::{
    Sandbox, SandboxEventKind, SandboxStatus, Workspace, WorkspaceSource, WorkspaceStatus,
    WorkspaceWorkStep,
};
use layer2_core::{rfc3339_at, WorkspaceId};
use layer3_adapter::{
    RuntimeHandle, SandboxAdapter, WorkspaceRemoveRequest, WorkspaceRequest, WorkspaceSetupRequest,
    WorkspaceSource as RuntimeWorkspaceSource,
};

use self::support::{
    claimed_fence, failure, valid_prepare_receipt, valid_setup_receipt, WorkerFailure,
};

mod execution;
mod failure_settlement;
mod prepare_recovery;
mod prepared;
mod profile;
mod removal;
mod setup;
mod support;
pub(crate) use profile::DurableProfileWorkspaceProvisioner;

const WORK_BATCH_SIZE: u16 = 50;

pub(crate) struct WorkspaceWorkerDependencies {
    pub store: Arc<dyn WorkspaceLifecycleStore>,
    pub adapter: Arc<dyn SandboxAdapter>,
}

pub(crate) use config::WorkspaceWorkerConfig;

pub(crate) struct WorkspaceWorker {
    dependencies: WorkspaceWorkerDependencies,
    config: WorkspaceWorkerConfig,
}

pub(crate) struct WorkspaceDrain {
    pub workspace: Workspace,
    pub executed_work: bool,
}

impl WorkspaceWorker {
    pub fn new(
        dependencies: WorkspaceWorkerDependencies,
        config: WorkspaceWorkerConfig,
    ) -> Arc<Self> {
        Arc::new(Self {
            dependencies,
            config,
        })
    }

    pub async fn run_due(&self, now: SystemTime) -> Result<usize, SandboxStoreError> {
        let started = tokio::time::Instant::now();
        let due = self
            .dependencies
            .store
            .due_workspace_work(&rfc3339_at(now), WORK_BATCH_SIZE)
            .await?;
        let mut completed = 0;
        let mut first_error = None;
        for id in due {
            let at = if self.config.renewable_work {
                now + started.elapsed()
            } else {
                now
            };
            match self.run_one(&id, at).await {
                Ok(true) => completed += 1,
                Ok(false) => {}
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        first_error.map_or(Ok(completed), Err)
    }

    /// Run one claimed workspace behind a boxed boundary so the scheduler and
    /// test callers do not inherit the full execution/renewal state machine.
    pub fn run_one(
        &self,
        id: &WorkspaceId,
        now: SystemTime,
    ) -> Pin<Box<dyn Future<Output = Result<bool, SandboxStoreError>> + Send + '_>> {
        let id = id.clone();
        Box::pin(async move { self.run_one_inner(&id, now).await })
    }

    async fn run_one_inner(
        &self,
        id: &WorkspaceId,
        now: SystemTime,
    ) -> Result<bool, SandboxStoreError> {
        let claiming = tokio::time::Instant::now();
        let claiming_work = self.dependencies.store.claim_workspace_work(
            id,
            &self.config.holder,
            rfc3339_at(now),
            rfc3339_at(now + self.config.lease_duration),
        );
        let granted = if self.config.renewable_work {
            tokio::time::timeout(
                Duration::from_secs(layer2_core::sandbox::store::WORK_RENEW_SECONDS),
                claiming_work,
            )
            .await
            .map_err(|_| {
                SandboxStoreError::Unavailable("work acquisition acknowledgement timed out".into())
            })??
        } else {
            claiming_work.await?
        };
        let Some(workspace) = granted else {
            return Ok(false);
        };
        if self.config.renewable_work || workspace.work.authority_protocol.is_some() {
            return self.run_renewable(workspace, now, claiming).await;
        }
        self.execute(workspace, now).await?;
        Ok(true)
    }

    async fn prepare(
        &self,
        workspace: Workspace,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let workspace = self.pin_execution(workspace).await?;
        if self.transfer_prepared(&workspace, now).await? {
            return Ok(());
        }
        let (sandbox, runtime) = match self.runtime(&workspace).await {
            Ok(value) => value,
            Err(failure) => return self.handle_failure(workspace, failure, now).await,
        };
        let request = workspace.original_prepare_request()?;
        layer2_core::sandbox::store::WorkAuthority::current()?;
        match self
            .dependencies
            .adapter
            .prepare_workspace(&runtime, &request)
            .await
        {
            Ok(receipt) if valid_prepare_receipt(&request, &receipt, &workspace.path) => {
                let mut next = workspace;
                if let WorkspaceSource::RepositorySnapshot { source_commit, .. }
                | WorkspaceSource::PublicRepositorySnapshot { source_commit, .. } =
                    &mut next.source
                {
                    *source_commit = receipt.source_commit;
                }
                if next
                    .record_resolved_commit(receipt.resolved_commit)
                    .is_err()
                {
                    return self
                        .handle_failure(next, WorkerFailure::invalid_receipt(), now)
                        .await;
                }
                self.advance(next, WorkspaceWorkStep::Setup, now).await
            }
            Ok(_) => {
                self.handle_failure(workspace, WorkerFailure::invalid_receipt(), now)
                    .await
            }
            Err(error) => {
                self.handle_prepare_failure(workspace, failure(error), now, &sandbox)
                    .await
            }
        }
    }

    async fn remove(&self, workspace: Workspace, now: SystemTime) -> Result<(), SandboxStoreError> {
        match self.remove_child_agents(&workspace, now).await {
            Ok(true) => {}
            Ok(false) => {
                return self
                    .advance(
                        workspace,
                        WorkspaceWorkStep::Remove,
                        now + Duration::from_secs(1),
                    )
                    .await
            }
            Err(failure) => return self.handle_failure(workspace, failure, now).await,
        }
        let parent = self
            .dependencies
            .store
            .get_sandbox(&workspace.sandbox_id)
            .await?
            .ok_or(SandboxStoreError::NotFound)?;
        // Retirement already removed the entire home. Never revive it merely to delete a directory.
        if parent.owner == workspace.owner
            && parent.status == SandboxStatus::Retired
            && parent.retired_at.is_some()
            && parent.runtime.is_none()
            && parent.lifecycle.checkpoint_capture.is_none()
            && parent.lifecycle.operation.is_none()
            && parent.lifecycle.runtime_provision.is_none()
            && parent.validate().is_ok()
        {
            match self.dependencies.adapter.runtime_inventory().await {
                Ok(inventory)
                    if inventory
                        .iter()
                        .all(|runtime| runtime.sandbox_id != parent.id.as_str()) =>
                {
                    return self.mark_removed(workspace, now).await
                }
                Ok(_) => {
                    return self
                        .handle_failure(
                            workspace,
                            WorkerFailure::retryable("retired sandbox runtime is still present"),
                            now,
                        )
                        .await
                }
                Err(error) => return self.handle_failure(workspace, failure(error), now).await,
            }
        }
        let (_, runtime) = match self.runtime(&workspace).await {
            Ok(value) => value,
            Err(failure) => return self.handle_failure(workspace, failure, now).await,
        };
        let request = WorkspaceRemoveRequest::new(
            workspace.operation_id(WorkspaceWorkStep::Remove),
            workspace.id.to_string(),
            workspace.name.clone(),
        );
        layer2_core::sandbox::store::WorkAuthority::current()?;
        match self
            .dependencies
            .adapter
            .remove_workspace(&runtime, &request)
            .await
        {
            Ok(()) => self.mark_removed(workspace, now).await,
            Err(error) => self.handle_failure(workspace, failure(error), now).await,
        }
    }

    async fn runtime(
        &self,
        workspace: &Workspace,
    ) -> Result<(Sandbox, RuntimeHandle), WorkerFailure> {
        let sandbox = self
            .dependencies
            .store
            .get_sandbox(&workspace.sandbox_id)
            .await
            .map_err(|_| WorkerFailure::retryable("sandbox store unavailable"))?
            .filter(|sandbox| {
                sandbox.owner == workspace.owner
                    && sandbox.lifecycle.checkpoint_capture.is_none()
                    && matches!(
                        sandbox.status,
                        SandboxStatus::Installing | SandboxStatus::Active
                    )
                    && sandbox.credential.delivered
            })
            .ok_or_else(|| WorkerFailure::retryable("sandbox runtime is not ready"))?;
        let authorities = layer2_core::sandbox::store::WorkAuthority::current()
            .map_err(|_| WorkerFailure::retryable("Workspace work authority expired"))?;
        if authorities
            .iter()
            .filter_map(|authority| authority.parent())
            .any(|original| {
                original.id == sandbox.id
                    && (original.owner != sandbox.owner
                        || original.status != sandbox.status
                        || original.runtime != sandbox.runtime
                        || original.credential.key_generation != sandbox.credential.key_generation
                        || original.lifecycle.runtime_generation_high_water
                            != sandbox.lifecycle.runtime_generation_high_water
                        || original.lifecycle.operation != sandbox.lifecycle.operation)
            })
        {
            return Err(WorkerFailure::retryable(
                "Workspace parent runtime authority changed",
            ));
        }
        let inspection_started = std::time::Instant::now();
        let attached = super::runtime_attachment::adopt_bound_runtime(
            self.dependencies.adapter.as_ref(),
            &sandbox,
            &self.config.deployment_id,
        )
        .await;
        tracing::info!(
            workspace_id = %workspace.id,
            sandbox_id = %sandbox.id,
            step = ?workspace.work.step,
            elapsed_ms = inspection_started.elapsed().as_millis() as u64,
            succeeded = attached.is_ok(),
            "Workspace runtime inspection completed"
        );
        let attached = attached.map_err(failure)?;
        Ok((sandbox, attached))
    }

    async fn advance(
        &self,
        mut workspace: Workspace,
        step: WorkspaceWorkStep,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let expected = workspace.revision;
        let fence = claimed_fence(&workspace, &self.config.holder)?;
        workspace
            .work
            .advance(&self.config.holder, fence, step, rfc3339_at(now))
            .map_err(|_| SandboxStoreError::Conflict)?;
        workspace.revision = expected.checked_add(1).ok_or(SandboxStoreError::Conflict)?;
        self.dependencies
            .store
            .save_workspace(expected, workspace)
            .await
    }

    async fn mark_ready(
        &self,
        mut workspace: Workspace,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let expected = workspace.revision;
        let fence = claimed_fence(&workspace, &self.config.holder)?;
        let stamp = rfc3339_at(now);
        workspace
            .work
            .advance(
                &self.config.holder,
                fence,
                WorkspaceWorkStep::None,
                stamp.clone(),
            )
            .map_err(|_| SandboxStoreError::Conflict)?;
        let commit = workspace
            .resolved_commit
            .clone()
            .ok_or_else(|| SandboxStoreError::Corrupt("workspace commit is missing".into()))?;
        workspace
            .transition(WorkspaceStatus::Ready, stamp.clone(), None)
            .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
        let id = workspace.id.clone();
        self.commit(
            expected,
            workspace,
            stamp,
            SandboxEventKind::WorkspaceReady {
                workspace_id: id,
                resolved_commit: commit,
            },
        )
        .await
    }

    async fn mark_removed(
        &self,
        mut workspace: Workspace,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let expected = workspace.revision;
        let fence = claimed_fence(&workspace, &self.config.holder)?;
        let stamp = rfc3339_at(now);
        workspace
            .work
            .advance(
                &self.config.holder,
                fence,
                WorkspaceWorkStep::None,
                stamp.clone(),
            )
            .map_err(|_| SandboxStoreError::Conflict)?;
        workspace
            .transition(WorkspaceStatus::Removed, stamp.clone(), None)
            .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
        let id = workspace.id.clone();
        self.commit(
            expected,
            workspace,
            stamp,
            SandboxEventKind::WorkspaceRemoved { workspace_id: id },
        )
        .await
    }

    async fn commit(
        &self,
        expected_revision: u64,
        workspace: Workspace,
        event_at: String,
        event: SandboxEventKind,
    ) -> Result<(), SandboxStoreError> {
        self.dependencies
            .store
            .commit(SandboxTransaction::SaveWorkspaceAndAppend {
                repository_runtime: None,
                expected_revision,
                workspace: Box::new(workspace),
                event_at,
                event,
            })
            .await
    }
}

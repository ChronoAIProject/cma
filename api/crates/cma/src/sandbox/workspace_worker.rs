use std::sync::Arc;
use std::time::{Duration, SystemTime};

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
mod profile;
mod support;
pub(crate) use profile::DurableProfileWorkspaceProvisioner;

const DEFAULT_LEASE: Duration = Duration::from_secs(15 * 60);
const DEFAULT_RETRY: Duration = Duration::from_secs(30);
const WORK_BATCH_SIZE: u16 = 50;

pub(crate) struct WorkspaceWorkerDependencies {
    pub store: Arc<dyn WorkspaceLifecycleStore>,
    pub adapter: Arc<dyn SandboxAdapter>,
}

#[derive(Clone)]
pub(crate) struct WorkspaceWorkerConfig {
    pub execution: layer2_core::sandbox::ProvisioningExecution,
    pub holder: String,
    pub deployment_id: String,
    pub lease_duration: Duration,
    pub retry_delay: Duration,
}

impl WorkspaceWorkerConfig {
    pub fn new(holder: impl Into<String>, deployment_id: impl Into<String>) -> Self {
        Self {
            execution: Default::default(),
            holder: holder.into(),
            deployment_id: deployment_id.into(),
            lease_duration: DEFAULT_LEASE,
            retry_delay: DEFAULT_RETRY,
        }
    }

    pub fn with_execution(
        mut self,
        execution: layer2_core::sandbox::ProvisioningExecution,
    ) -> Self {
        self.execution = execution;
        self.lease_duration = self.lease_duration.max(Duration::from_secs(
            layer3_adapter::DEFAULT_TOTAL_TIMEOUT_SECONDS + 60,
        ));
        self
    }
}

pub(crate) struct WorkspaceWorker {
    dependencies: WorkspaceWorkerDependencies,
    config: WorkspaceWorkerConfig,
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
        let due = self
            .dependencies
            .store
            .due_workspace_work(&rfc3339_at(now), WORK_BATCH_SIZE)
            .await?;
        let mut completed = 0;
        let mut first_error = None;
        for id in due {
            match self.run_one(&id, now).await {
                Ok(true) => completed += 1,
                Ok(false) => {}
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        first_error.map_or(Ok(completed), Err)
    }

    pub async fn run_one(
        &self,
        id: &WorkspaceId,
        now: SystemTime,
    ) -> Result<bool, SandboxStoreError> {
        let Some(workspace) = self
            .dependencies
            .store
            .claim_workspace_work(
                id,
                &self.config.holder,
                rfc3339_at(now),
                rfc3339_at(now + self.config.lease_duration),
            )
            .await?
        else {
            return Ok(false);
        };
        self.execute(workspace, now).await?;
        Ok(true)
    }

    pub async fn run_until_ready(
        &self,
        id: &WorkspaceId,
        now: SystemTime,
    ) -> Result<Workspace, SandboxStoreError> {
        for _ in 0..4 {
            let current = self
                .dependencies
                .store
                .get_workspace(id)
                .await?
                .ok_or(SandboxStoreError::NotFound)?;
            match current.status {
                WorkspaceStatus::Ready => return Ok(current),
                WorkspaceStatus::Failed | WorkspaceStatus::Removing | WorkspaceStatus::Removed => {
                    return Ok(current)
                }
                WorkspaceStatus::Preparing => {
                    if !self.run_one(id, now).await? {
                        return Ok(current);
                    }
                }
            }
        }
        self.dependencies
            .store
            .get_workspace(id)
            .await?
            .ok_or(SandboxStoreError::NotFound)
    }

    async fn execute(
        &self,
        workspace: Workspace,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        match workspace.work.step {
            WorkspaceWorkStep::Prepare => self.prepare(workspace, now).await,
            WorkspaceWorkStep::Setup => self.setup(workspace, now).await,
            WorkspaceWorkStep::Remove => self.remove(workspace, now).await,
            WorkspaceWorkStep::None => Err(SandboxStoreError::Conflict),
        }
    }

    async fn prepare(
        &self,
        workspace: Workspace,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let workspace = self.pin_execution(workspace).await?;
        let (_, runtime) = match self.runtime(&workspace).await {
            Ok(value) => value,
            Err(failure) => return self.handle_failure(workspace, failure, now).await,
        };
        let source = match &workspace.source {
            WorkspaceSource::GitRepository { url, reference } => RuntimeWorkspaceSource::Clone {
                url: url.clone(),
                reference: reference.clone(),
            },
            WorkspaceSource::Empty => RuntimeWorkspaceSource::Init,
        };
        let request = WorkspaceRequest::with_clone_depth(
            workspace.operation_id(WorkspaceWorkStep::Prepare),
            workspace.id.to_string(),
            workspace.name.clone(),
            source,
            workspace
                .work
                .execution
                .expect("execution policy was persisted")
                .clone_depth,
        );
        match self
            .dependencies
            .adapter
            .prepare_workspace(&runtime, &request)
            .await
        {
            Ok(receipt) if valid_prepare_receipt(&request, &receipt, &workspace.path) => {
                let mut next = workspace;
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
            Err(error) => self.handle_failure(workspace, failure(error), now).await,
        }
    }

    async fn setup(&self, workspace: Workspace, now: SystemTime) -> Result<(), SandboxStoreError> {
        let workspace = self.pin_execution(workspace).await?;
        let (_, runtime) = match self.runtime(&workspace).await {
            Ok(value) => value,
            Err(failure) => return self.handle_failure(workspace, failure, now).await,
        };
        let execution = workspace
            .work
            .execution
            .expect("execution policy was persisted");
        let request = WorkspaceSetupRequest::with_timeouts(
            workspace.operation_id(WorkspaceWorkStep::Setup),
            workspace.id.to_string(),
            workspace.name.clone(),
            workspace.profile.content.setup_commands.clone(),
            execution.command_timeout_seconds,
            execution.total_timeout_seconds,
            layer3_adapter::DEFAULT_COMMAND_OUTPUT_LIMIT_BYTES,
        );
        match self
            .dependencies
            .adapter
            .run_workspace_setup(&runtime, &request)
            .await
        {
            Ok(receipt) if valid_setup_receipt(&request, &receipt) => {
                self.mark_ready(workspace, now).await
            }
            Ok(_) => {
                self.handle_failure(workspace, WorkerFailure::invalid_receipt(), now)
                    .await
            }
            Err(error) => self.handle_failure(workspace, failure(error), now).await,
        }
    }

    async fn remove(&self, workspace: Workspace, now: SystemTime) -> Result<(), SandboxStoreError> {
        let (_, runtime) = match self.runtime(&workspace).await {
            Ok(value) => value,
            Err(failure) => return self.handle_failure(workspace, failure, now).await,
        };
        let request = WorkspaceRemoveRequest::new(
            workspace.operation_id(WorkspaceWorkStep::Remove),
            workspace.id.to_string(),
            workspace.name.clone(),
        );
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
        let attached = super::runtime_attachment::adopt_bound_runtime(
            self.dependencies.adapter.as_ref(),
            &sandbox,
            &self.config.deployment_id,
        )
        .await
        .map_err(failure)?;
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

    async fn handle_failure(
        &self,
        mut workspace: Workspace,
        failure: WorkerFailure,
        now: SystemTime,
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
        workspace
            .transition(WorkspaceStatus::Failed, stamp.clone(), Some(typed.clone()))
            .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
        let id = workspace.id.clone();
        self.commit(
            expected,
            workspace,
            stamp,
            SandboxEventKind::WorkspaceFailed {
                workspace_id: id,
                failure: typed,
            },
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
                expected_revision,
                workspace: Box::new(workspace),
                event_at,
                event,
            })
            .await
    }
}

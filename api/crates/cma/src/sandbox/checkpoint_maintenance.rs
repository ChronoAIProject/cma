use futures_util::{stream, StreamExt};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use layer2_core::rfc3339_at;
use layer2_core::sandbox::store::{
    checkpoint_capture::validate_checkpoint_capture_work, PageRequest, SandboxLifecycleAgentScope,
    SandboxLifecycleMutation, SandboxLifecycleStore, SandboxMutationGuard,
    SandboxReconciliationStore, SandboxStoreError,
};
use layer2_core::sandbox::{
    sandbox_checkpoint_due_with_interval, Agent, Sandbox, SandboxCheckpointCapturePhase,
    SandboxEventKind, SandboxPolicySource, SandboxStatus, SandboxWorkStep, Workspace,
};
use layer3_adapter::{writer_resume_tokens_match, CheckpointWriterResumeRequest, SandboxAdapter};

use super::runtime_attachment::adopt_bound_runtime;

mod capture;
mod export;
mod lease;

use lease::CaptureClock;

const CLAIM_DURATION: Duration = Duration::from_secs(5 * 60);
const RETRY_DELAY: Duration = Duration::from_secs(30);
const PAGE_SIZE: u16 = 200;

pub(crate) trait SandboxCheckpointMaintenanceStore:
    SandboxLifecycleStore + SandboxReconciliationStore
{
}
impl<T> SandboxCheckpointMaintenanceStore for T where
    T: SandboxLifecycleStore + SandboxReconciliationStore
{
}

pub(crate) struct SandboxCheckpointMaintenance {
    store: Arc<dyn SandboxCheckpointMaintenanceStore>,
    adapter: Arc<dyn SandboxAdapter>,
    deployment_id: String,
    holder: String,
    policy: Arc<dyn SandboxPolicySource>,
    export: Option<ExportDependencies>,
}

#[derive(Clone)]
pub(crate) struct ExportDependencies {
    pub homes: Arc<dyn SnapshotExportStore>,
    pub objects: Arc<dyn layer3_adapter::hosted::CheckpointArchiveStoreFactory>,
    pub image: String,
}

pub(crate) trait SnapshotExportStore:
    layer3_adapter::HomeSnapshotStore
    + layer3_adapter::HomeSnapshotViewStore
    + layer3_adapter::HomeSnapshotExportJobStore
{
}
impl<T> SnapshotExportStore for T where
    T: layer3_adapter::HomeSnapshotStore
        + layer3_adapter::HomeSnapshotViewStore
        + layer3_adapter::HomeSnapshotExportJobStore
{
}

impl SandboxCheckpointMaintenance {
    #[cfg(test)]
    pub(crate) fn new(
        store: Arc<dyn SandboxCheckpointMaintenanceStore>,
        adapter: Arc<dyn SandboxAdapter>,
        deployment_id: String,
        holder: String,
    ) -> Arc<Self> {
        Self::with_policy_source(
            store,
            adapter,
            deployment_id,
            holder,
            Arc::new(layer2_core::sandbox::SandboxPolicy::default()),
        )
    }

    pub(crate) fn with_policy_source(
        store: Arc<dyn SandboxCheckpointMaintenanceStore>,
        adapter: Arc<dyn SandboxAdapter>,
        deployment_id: String,
        holder: String,
        policy: Arc<dyn SandboxPolicySource>,
    ) -> Arc<Self> {
        Arc::new(Self {
            store,
            adapter,
            deployment_id,
            holder,
            policy,
            export: None,
        })
    }

    pub(crate) fn with_export_dependencies(
        store: Arc<dyn SandboxCheckpointMaintenanceStore>,
        adapter: Arc<dyn SandboxAdapter>,
        deployment_id: String,
        holder: String,
        policy: Arc<dyn SandboxPolicySource>,
        export: ExportDependencies,
    ) -> Arc<Self> {
        Arc::new(Self {
            store,
            adapter,
            deployment_id,
            holder,
            policy,
            export: Some(export),
        })
    }

    pub(crate) async fn run_once(&self, now: SystemTime) -> Result<usize, SandboxStoreError> {
        let clock = CaptureClock::new(now);
        let mut cursor = None;
        let mut progressed = 0;
        let mut first_error = None;
        loop {
            let page = self
                .store
                .sandbox_runtime_authority(
                    &PageRequest::new(PAGE_SIZE, cursor)
                        .map_err(|_| SandboxStoreError::InvalidPage)?,
                )
                .await?;
            let (background, held): (Vec<_>, Vec<_>) =
                page.items.into_iter().partition(|sandbox| {
                    self.export.is_some()
                        && (sandbox.lifecycle.background_checkpoint.is_some()
                            || (sandbox.lifecycle.checkpoint_capture.is_none()
                                && sandbox.lifecycle.home.is_some()))
                });
            let background = stream::iter(background)
                .map(|sandbox| {
                    layer2_core::agent_ownership::transport_observation_scope(
                        self.process(sandbox, &clock),
                    )
                })
                .buffer_unordered(4);
            let held = stream::iter(held).then(|sandbox| {
                layer2_core::agent_ownership::transport_observation_scope(
                    self.process(sandbox, &clock),
                )
            });
            let results = background.chain(held);
            tokio::pin!(results);
            while let Some(result) = results.next().await {
                match result {
                    Ok(changed) => progressed += usize::from(changed),
                    Err(SandboxStoreError::Conflict | SandboxStoreError::NotFound) => {}
                    Err(error) => {
                        first_error.get_or_insert(error);
                    }
                }
            }
            let Some(next) = page.next else { break };
            cursor = Some(next);
        }
        first_error.map_or(Ok(progressed), Err)
    }

    async fn process(
        &self,
        sandbox: Sandbox,
        clock: &CaptureClock,
    ) -> Result<bool, SandboxStoreError> {
        let now = clock.now();
        let stamp = rfc3339_at(now);
        if sandbox.lifecycle.background_checkpoint.is_some() {
            return self.process_export(&sandbox, clock).await;
        }
        let Some(capture) = sandbox.lifecycle.checkpoint_capture.as_ref() else {
            let policy = self
                .policy
                .current_sandbox_policy()
                .await
                .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
            if !sandbox_checkpoint_due_with_interval(
                &sandbox,
                &stamp,
                policy.checkpoint_interval_seconds,
            ) {
                return Ok(false);
            }
            if self.export.is_some()
                && sandbox
                    .lifecycle
                    .home
                    .as_ref()
                    .is_some_and(|home| home.attachment().is_some())
            {
                self.begin_export(&sandbox, clock).await?;
                return Ok(true);
            }
            let agents = self.agents(&sandbox).await?;
            let workspaces = self.workspaces(&sandbox).await?;
            if validate_checkpoint_capture_work(&sandbox, &agents, &workspaces).is_err() {
                return Ok(false);
            }
            let mut next = sandbox.clone();
            let stamp = rfc3339_at(clock.now());
            if next.begin_checkpoint_capture(stamp.clone()).is_err() {
                return Ok(false);
            }
            self.commit(&sandbox, next, stamp).await?;
            return Ok(true);
        };
        if !capture.work.is_due(&stamp) {
            return Ok(false);
        }
        let abandoned = capture.work.lease.is_some();
        let mut claimed = sandbox.clone();
        claimed
            .claim_checkpoint_capture(
                self.holder.clone(),
                stamp.clone(),
                rfc3339_at(now + CLAIM_DURATION),
            )
            .map_err(model_error)?;
        self.commit(&sandbox, claimed.clone(), stamp.clone())
            .await?;
        let capture = claimed
            .lifecycle
            .checkpoint_capture
            .as_ref()
            .expect("claimed capture remains present");
        if capture.phase == SandboxCheckpointCapturePhase::Capture {
            if abandoned || claimed.status != SandboxStatus::Active {
                self.require_resume(&claimed, clock.now()).await?;
            } else {
                self.capture_and_publish(claimed, clock).await?;
            }
        } else {
            self.resume(&claimed, clock).await?;
        }
        Ok(true)
    }

    async fn resume(
        &self,
        claimed: &Sandbox,
        clock: &CaptureClock,
    ) -> Result<(), SandboxStoreError> {
        let capture = claimed
            .lifecycle
            .checkpoint_capture
            .as_ref()
            .ok_or(SandboxStoreError::Conflict)?;
        let lease = capture
            .work
            .lease
            .as_ref()
            .ok_or(SandboxStoreError::Conflict)?;
        let token = capture
            .authority
            .writer_resume_token
            .as_ref()
            .ok_or(SandboxStoreError::Conflict)?;
        let result = tokio::time::timeout(lease::FINALIZE_TIMEOUT, async {
            let handle =
                adopt_bound_runtime(self.adapter.as_ref(), claimed, &self.deployment_id).await?;
            self.adapter
                .resume_checkpoint_writers(
                    &handle,
                    &CheckpointWriterResumeRequest {
                        writer_resume_token: token.clone(),
                    },
                )
                .await
        })
        .await;
        let now = clock.now();
        let mut next = claimed.clone();
        let stamp = rfc3339_at(now);
        match result {
            Ok(Ok(receipt))
                if receipt.writers_resumed
                    && writer_resume_tokens_match(&receipt.writer_resume_token, token) =>
            {
                next.complete_checkpoint_writer_resume(&lease.holder, lease.fence, stamp.clone())
                    .map_err(model_error)?;
            }
            _ => {
                next.retry_checkpoint_capture(
                    rfc3339_at(now + RETRY_DELAY),
                    &lease.holder,
                    lease.fence,
                    stamp.clone(),
                )
                .map_err(model_error)?;
            }
        }
        self.commit(claimed, next, stamp).await
    }

    async fn require_resume(
        &self,
        attempted: &Sandbox,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let current = self
            .store
            .get_sandbox(&attempted.id)
            .await?
            .ok_or(SandboxStoreError::NotFound)?;
        let attempted_capture = attempted
            .lifecycle
            .checkpoint_capture
            .as_ref()
            .ok_or(SandboxStoreError::Conflict)?;
        let capture = current
            .lifecycle
            .checkpoint_capture
            .as_ref()
            .ok_or(SandboxStoreError::Conflict)?;
        if capture.authority != attempted_capture.authority
            || capture.runtime != attempted_capture.runtime
            || capture.work.lease != attempted_capture.work.lease
        {
            return Err(SandboxStoreError::Conflict);
        }
        let lease = capture
            .work
            .lease
            .as_ref()
            .ok_or(SandboxStoreError::Conflict)?;
        let stamp = rfc3339_at(now);
        let mut next = current.clone();
        next.require_checkpoint_writer_resume(&lease.holder, lease.fence, stamp.clone())
            .map_err(model_error)?;
        self.commit(&current, next, stamp).await
    }

    async fn commit(
        &self,
        current: &Sandbox,
        next: Sandbox,
        at: String,
    ) -> Result<(), SandboxStoreError> {
        let result = self
            .store
            .commit_sandbox_lifecycle(SandboxLifecycleMutation {
                guard: SandboxMutationGuard::capture(current),
                sandbox: next.clone(),
                agents: Vec::new(),
                agent_scope: SandboxLifecycleAgentScope::Touched,
                appended_at: at,
                events: vec![SandboxEventKind::WorkStepCompleted {
                    step: SandboxWorkStep::RunLifecycleOperation,
                }],
            })
            .await;
        if result.is_err() && self.store.get_sandbox(&current.id).await?.as_ref() == Some(&next) {
            return Ok(());
        }
        result
    }

    async fn agents(&self, sandbox: &Sandbox) -> Result<Vec<Agent>, SandboxStoreError> {
        let mut rows = Vec::new();
        let mut cursor = None;
        loop {
            let page = self
                .store
                .sandbox_agents(
                    &sandbox.id,
                    &PageRequest::new(PAGE_SIZE, cursor)
                        .map_err(|_| SandboxStoreError::InvalidPage)?,
                )
                .await?;
            rows.extend(page.items);
            if rows.len() > 256 {
                return Err(SandboxStoreError::Conflict);
            }
            let Some(next) = page.next else { break };
            cursor = Some(next);
        }
        Ok(rows)
    }

    async fn workspaces(&self, sandbox: &Sandbox) -> Result<Vec<Workspace>, SandboxStoreError> {
        let mut rows = Vec::new();
        let mut cursor = None;
        loop {
            let page = self
                .store
                .list_workspaces(
                    &sandbox.id,
                    &PageRequest::new(PAGE_SIZE, cursor)
                        .map_err(|_| SandboxStoreError::InvalidPage)?,
                )
                .await?;
            rows.extend(page.items);
            if rows.len() > 256 {
                return Err(SandboxStoreError::Conflict);
            }
            let Some(next) = page.next else { break };
            cursor = Some(next);
        }
        Ok(rows)
    }
}

fn model_error(error: layer2_core::sandbox::SandboxModelError) -> SandboxStoreError {
    SandboxStoreError::Corrupt(error.to_string())
}

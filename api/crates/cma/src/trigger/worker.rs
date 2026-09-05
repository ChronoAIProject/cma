use std::sync::Arc;
use std::time::{Duration, SystemTime};

use layer2_core::trigger_domain::{Launch, LaunchStore, LaunchStoreError, LaunchWorkStep};
use layer2_core::{rfc3339_at, LaunchId};

mod agent;
mod control;
mod driver;
mod effects;
mod sandbox;
mod sandbox_progress;
mod support;
mod workspace;

#[allow(
    unused_imports,
    reason = "used when the public Launch route composition activates this worker"
)]
pub(crate) use driver::{
    LaunchResourceDriver, LaunchResourceEffect, LaunchResourceError,
    ServiceLaunchResourceDependencies, ServiceLaunchResourceDriver,
};
use support::FrozenLaunchContext;

const DEFAULT_LEASE: Duration = Duration::from_secs(15 * 60);
const DEFAULT_POLL: Duration = Duration::from_secs(5);
const DEFAULT_RETRY: Duration = Duration::from_secs(5);
const WORK_BATCH_SIZE: u16 = 50;

#[derive(Clone)]
pub(crate) struct LaunchWorkerConfig {
    pub holder: String,
    pub lease_duration: Duration,
    pub poll_delay: Duration,
    pub retry_delay: Duration,
}

impl LaunchWorkerConfig {
    pub(crate) fn new(holder: impl Into<String>) -> Self {
        Self {
            holder: holder.into(),
            lease_duration: DEFAULT_LEASE,
            poll_delay: DEFAULT_POLL,
            retry_delay: DEFAULT_RETRY,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum LaunchWorkerError {
    #[error(transparent)]
    Store(#[from] LaunchStoreError),
    #[error("launch worker state is not usable: {0}")]
    Corrupt(String),
}

pub(crate) struct LaunchWorker {
    launches: Arc<dyn LaunchStore>,
    resources: Arc<dyn LaunchResourceDriver>,
    config: LaunchWorkerConfig,
}

impl LaunchWorker {
    pub(crate) fn new<S, D>(
        launches: Arc<S>,
        resources: Arc<D>,
        config: LaunchWorkerConfig,
    ) -> Arc<Self>
    where
        S: LaunchStore + 'static,
        D: LaunchResourceDriver + 'static,
    {
        assert!(!config.holder.trim().is_empty());
        assert!(!config.lease_duration.is_zero());
        assert!(!config.poll_delay.is_zero());
        assert!(!config.retry_delay.is_zero());
        Arc::new(Self {
            launches,
            resources,
            config,
        })
    }

    pub(crate) async fn run_due(&self, now: SystemTime) -> Result<usize, LaunchWorkerError> {
        let due = self
            .launches
            .due_launches(&rfc3339_at(now), WORK_BATCH_SIZE)
            .await?;
        let mut completed = 0;
        let mut first_error = None;
        for id in due {
            match self.run_one(&id, now).await {
                Ok(true) => completed += 1,
                Ok(false) => {}
                Err(error) => {
                    tracing::error!(launch_id = %id, error = %error, "launch work failed");
                    first_error.get_or_insert(error);
                }
            }
        }
        first_error.map_or(Ok(completed), Err)
    }

    pub(crate) async fn run_one(
        &self,
        id: &LaunchId,
        now: SystemTime,
    ) -> Result<bool, LaunchWorkerError> {
        let Some(claimed) = self
            .launches
            .claim_launch(
                id,
                &self.config.holder,
                rfc3339_at(now),
                rfc3339_at(now + self.config.lease_duration),
            )
            .await?
        else {
            return Ok(false);
        };
        self.execute(claimed, now).await?;
        Ok(true)
    }

    async fn execute(&self, launch: Launch, now: SystemTime) -> Result<(), LaunchWorkerError> {
        let Some(launch) = self.prepare_boundary(launch, now).await? else {
            return Ok(());
        };
        if launch.work.step == LaunchWorkStep::Finish {
            return self.finish(launch, now).await;
        }
        let Some((launch, context)) = self.load_frozen_context(launch, now).await? else {
            return Ok(());
        };
        self.execute_step(launch, &context, now).await
    }

    async fn execute_step(
        &self,
        launch: Launch,
        context: &FrozenLaunchContext,
        now: SystemTime,
    ) -> Result<(), LaunchWorkerError> {
        match launch.work.step {
            LaunchWorkStep::ResolveSandbox => self.resolve_sandbox(launch, context, now).await,
            LaunchWorkStep::WaitForSandbox => self.wait_for_sandbox(launch, context, now).await,
            LaunchWorkStep::ResolveWorkspace => self.resolve_workspace(launch, context, now).await,
            LaunchWorkStep::WaitForWorkspace => self.wait_for_workspace(launch, context, now).await,
            LaunchWorkStep::ResolveAgent => self.resolve_agent(launch, context, now).await,
            LaunchWorkStep::WaitForAgent => self.wait_for_agent(launch, context, now).await,
            LaunchWorkStep::DeliverFirstMessage | LaunchWorkStep::WaitForFirstMessage => {
                self.observe_first_message(launch, context, now).await
            }
            LaunchWorkStep::Finish => self.finish(launch, now).await,
            LaunchWorkStep::None => Err(LaunchWorkerError::Corrupt(
                "a non-pending launch was claimed".into(),
            )),
        }
    }
}

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use layer2_core::trigger_domain::{Launch, LaunchStore, LaunchStoreError, LaunchWorkStep};
use layer2_core::{rfc3339_at, LaunchId};

mod agent;
mod control;
mod driver;
mod effects;
mod existing;
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
use support::{FrozenContextCache, FrozenLaunchContext};

const DEFAULT_LEASE: Duration = Duration::from_secs(15 * 60);
const DEFAULT_POLL: Duration = Duration::from_secs(1);
const DEFAULT_RETRY: Duration = Duration::from_secs(5);

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

    pub(crate) async fn due_work(
        &self,
        now: SystemTime,
        limit: u16,
    ) -> Result<Vec<LaunchId>, LaunchWorkerError> {
        Ok(self.launches.due_launches(&rfc3339_at(now), limit).await?)
    }

    #[cfg(test)]
    pub(crate) async fn run_due(&self, now: SystemTime) -> Result<usize, LaunchWorkerError> {
        let due = self.due_work(now, 50).await?;
        let mut completed = 0;
        let mut first_error = None;
        for id in due {
            match self.run_scheduled(&id, now).await {
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

    #[cfg(test)]
    pub(crate) async fn run_one(
        &self,
        id: &LaunchId,
        now: SystemTime,
    ) -> Result<bool, LaunchWorkerError> {
        self.run_cached(id, now, &mut FrozenContextCache::default())
            .await
    }

    async fn run_cached(
        &self,
        id: &LaunchId,
        now: SystemTime,
        context: &mut FrozenContextCache,
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
        let step = claimed.work.step;
        let started = tokio::time::Instant::now();
        self.execute(claimed, now, context).await?;
        if step != LaunchWorkStep::Finish {
            tracing::info!(launch_id = %id, ?step, elapsed_ms = started.elapsed().as_millis(), "Launch step completed");
        }
        Ok(true)
    }

    pub(crate) async fn run_scheduled(
        &self,
        id: &LaunchId,
        now: SystemTime,
    ) -> Result<bool, LaunchWorkerError> {
        let started = tokio::time::Instant::now();
        let mut progressed = false;
        let mut context = FrozenContextCache::default();
        // Follow committed transitions immediately. A new durable claim on
        // every step preserves cancellation, retry times and exclusive work.
        for _ in 0..16 {
            if !self
                .run_cached(id, now + started.elapsed(), &mut context)
                .await?
            {
                break;
            }
            progressed = true;
        }
        Ok(progressed)
    }

    async fn execute(
        &self,
        launch: Launch,
        now: SystemTime,
        cache: &mut FrozenContextCache,
    ) -> Result<(), LaunchWorkerError> {
        let Some(launch) = self.prepare_boundary(launch, now).await? else {
            return Ok(());
        };
        if launch.work.step == LaunchWorkStep::Finish {
            return self.finish(launch, now).await;
        }
        let Some((launch, context)) = self.cached_frozen_context(launch, now, cache).await? else {
            return Ok(());
        };
        self.execute_step(launch, &context, now).await
    }

    // Allocate only the selected step. Embedding every step's future in this
    // dispatcher inflates the stack of nested admission/publication calls.
    fn execute_step<'a>(
        &'a self,
        launch: Launch,
        context: &'a FrozenLaunchContext,
        now: SystemTime,
    ) -> futures_util::future::BoxFuture<'a, Result<(), LaunchWorkerError>> {
        match launch.work.step {
            LaunchWorkStep::ResolveSandbox => Box::pin(self.resolve_sandbox(launch, context, now)),
            LaunchWorkStep::WaitForSandbox => Box::pin(self.wait_for_sandbox(launch, context, now)),
            LaunchWorkStep::ResolveWorkspace => {
                Box::pin(self.resolve_workspace(launch, context, now))
            }
            LaunchWorkStep::WaitForWorkspace => {
                Box::pin(self.wait_for_workspace(launch, context, now))
            }
            LaunchWorkStep::ResolveAgent => Box::pin(self.resolve_agent(launch, context, now)),
            LaunchWorkStep::WaitForAgent => Box::pin(self.wait_for_agent(launch, context, now)),
            LaunchWorkStep::DeliverFirstMessage | LaunchWorkStep::WaitForFirstMessage => {
                Box::pin(self.observe_first_message(launch, context, now))
            }
            LaunchWorkStep::Finish => Box::pin(self.finish(launch, now)),
            LaunchWorkStep::None => Box::pin(std::future::ready(Err(LaunchWorkerError::Corrupt(
                "a non-pending launch was claimed".into(),
            )))),
        }
    }
}

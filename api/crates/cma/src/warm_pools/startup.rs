//! Prevent the first allocation after promotion from taking an avoidable cold path.
use super::{
    apply_claimed, NativePoolConfiguration, NativePoolSpec, NativePoolStatus, NativeWarmPools,
    PoolTarget, PoolTargetStore, WarmPoolError,
};
use layer3_adapter::agent_sandbox::NativePoolControl;
use std::sync::Arc;
use std::time::Duration;
use tokio::{sync::watch, time::Instant};

use crate::startup::Admission;

pub(crate) type StartupWriter<'a> = (
    &'a crate::store::released_runtimes::MongoReleasedRuntimes,
    &'a crate::store::released_runtimes::BootWriter,
);

pub(crate) async fn boot_effect<T>(
    writer: Option<StartupWriter<'_>>,
    effect: impl std::future::Future<Output = Result<T, WarmPoolError>>,
) -> Result<T, WarmPoolError> {
    if let Some((lanes, writer)) = writer {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        lanes
            .protect_boot_effect(writer)
            .await
            .map_err(|_| WarmPoolError::Conflict)?;
        tokio::select! {
            biased;
            _ = tokio::time::sleep_until(deadline) => Err(WarmPoolError::Unavailable),
            result = effect => result,
        }
    } else {
        effect.await
    }
}

// Outlast the release settle window plus drain, scheduling and image pull:
// with 120 s each, a promotion onto a full node pool always gave up exactly
// when older capacity first became eligible to drain, so every runtime
// promotion restarted the Agent Manager once. The 240 s total leaves time for
// socket bind and sandbox initialization inside the default 300 s startup probe.
const STARTUP_WARMUP_LIMIT: Duration = Duration::from_secs(super::RELEASE_SETTLE.as_secs() + 120);

#[derive(Debug)]
pub(crate) enum WarmPoolStartup {
    Ready,
    ZeroTarget,
    CapacityDeferred(ConfirmedSpareShortage),
    Stopping,
}

#[derive(Debug)]
pub(crate) struct ConfirmedSpareShortage(Box<PoolTarget>);

impl ConfirmedSpareShortage {
    pub(crate) fn desired(&self) -> u32 {
        self.0.target_replicas
    }
}

impl NativeWarmPools {
    pub(crate) fn with_empty_pool_startup(mut self: Arc<Self>, enabled: bool) -> Arc<Self> {
        Arc::make_mut(&mut self).allow_empty_startup = enabled;
        self
    }

    /// Release preparation should prewarm this plan before the Recreate rollout.
    /// This bounded guard never serves allocations before readiness and does not
    /// imply that the old API remains available during a Recreate deployment.
    ///
    /// Only the default runtime plan gates startup. The shared desktop pool
    /// merely shortens machine startup, which still works cold, and a new
    /// machine image's first pull alone can outlast this guard; the supervisor
    /// fills that pool in the background.
    #[cfg(test)]
    pub async fn await_default_ready(&self) -> Result<WarmPoolStartup, WarmPoolError> {
        self.await_default_ready_until(None).await
    }

    pub(crate) async fn await_default_ready_until(
        &self,
        admission: Option<&Admission>,
    ) -> Result<WarmPoolStartup, WarmPoolError> {
        self.await_default_ready_as(admission, None).await
    }

    pub(crate) async fn await_default_ready_as(
        &self,
        admission: Option<&Admission>,
        writer: Option<StartupWriter<'_>>,
    ) -> Result<WarmPoolStartup, WarmPoolError> {
        let deadline = Instant::now() + STARTUP_WARMUP_LIMIT;
        if let Some(admission) = admission {
            let (settled, retiring) = watch::channel(false);
            let stopping = || {
                admission.is_draining()
                    || *retiring.borrow()
                    || (self.allow_empty_startup && Instant::now() >= deadline)
            };
            let (result, retirement_ok) = tokio::join!(
                async {
                    let result = self
                        .await_default_plan_ready(deadline, Some(admission), writer)
                        .await;
                    settled.send_replace(true);
                    result
                },
                async {
                    // Each admitted retirement must settle before either startup
                    // or shutdown continues, even if readiness wins the race.
                    let mut retirement_ok = true;
                    while !stopping() {
                        retirement_ok &= self.retire_during_startup(&stopping, writer).await;
                        tokio::select! {
                            () = crate::shutdown::signalled(retiring.clone()) => break,
                            () = admission.stopping() => break,
                            () = tokio::time::sleep(Duration::from_secs(5)) => (),
                        }
                    }
                    retirement_ok
                },
            );
            return if admission.is_draining() {
                Ok(WarmPoolStartup::Stopping)
            } else if !retirement_ok && matches!(result, Ok(WarmPoolStartup::CapacityDeferred(_))) {
                Err(WarmPoolError::Unavailable)
            } else {
                result
            };
        }
        // The supervisor that retires superseded pools starts only after this
        // gate, yet on a full node pool the new default schedules only once
        // their spare capacity drains. Retire superseded targets while waiting,
        // so a starting pod frees the room it needs.
        let drain = async {
            loop {
                self.retire_during_startup(&|| false, writer).await;
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        };
        tokio::select! {
            result = self.await_default_plan_ready(deadline, None, writer) => result,
            _ = drain => unreachable!("superseded pool draining never completes"),
        }
    }

    async fn retire_during_startup(
        &self,
        stopping: &impl Fn() -> bool,
        writer: Option<StartupWriter<'_>>,
    ) -> bool {
        let succeeded = self.retire_superseded(stopping, writer).await.is_ok();
        if !succeeded {
            tracing::warn!("superseded native pool retirement during startup will retry");
        }
        succeeded
    }

    /// Claims only retiring or superseded targets, never a current plan: the
    /// generic reconciler's unbound path would configure a freshly inserted
    /// default at zero replicas and empty a pool that release preparation
    /// prewarmed, which `step` deliberately adopts instead.
    async fn retire_superseded(
        &self,
        stopping: &impl Fn() -> bool,
        writer: Option<StartupWriter<'_>>,
    ) -> Result<(), WarmPoolError> {
        let Some(provider) = self.adapter.native_pools() else {
            return Ok(());
        };
        for target in self.store.list().await? {
            if stopping() {
                break;
            }
            if target.retired
                || (!target.retiring
                    && (self.current_plan(&target)?
                        || self.store.candidate_protected(&target).await?))
            {
                continue;
            }
            // Plans are immutable and retirement only advances, so a claimed
            // superseded target is still superseded. A target an earlier pod
            // marked retiring and deferred is resumed, not skipped.
            boot_effect(writer, async {
                if !stopping() {
                    if let Some(claimed) = self.store.claim_retirement(&target.id).await? {
                        self.retire_claimed(provider, claimed).await?;
                    }
                }
                Ok(())
            })
            .await?;
        }
        Ok(())
    }

    async fn await_default_plan_ready(
        &self,
        deadline: Instant,
        admission: Option<&Admission>,
        writer: Option<StartupWriter<'_>>,
    ) -> Result<WarmPoolStartup, WarmPoolError> {
        let provider = self
            .adapter
            .native_pools()
            .ok_or(WarmPoolError::Unavailable)?;
        let plan = provider
            .plan(&NativePoolSpec {
                cold_canary: false,
                image: self.released_images().agent,
                cpu_millis: self.default_capacity.cpu_millis,
                memory_mib: self.default_capacity.memory_mib,
                home: self.home.clone().ok_or(WarmPoolError::Unavailable)?,
            })
            .map_err(|_| WarmPoolError::Unavailable)?;
        await_ready_with_fencing(
            self.store.as_ref(),
            provider,
            &plan,
            deadline,
            StartupPoolPolicy {
                target_fencing: self.target_fencing,
                allow_empty: self.allow_empty_startup,
            },
            admission,
            writer,
        )
        .await
    }
}

#[derive(Clone, Copy, Default)]
pub(super) struct StartupPoolPolicy {
    pub(super) target_fencing: bool,
    pub(super) allow_empty: bool,
}

#[cfg(test)]
pub(super) async fn await_ready(
    store: &dyn PoolTargetStore,
    provider: &dyn NativePoolControl,
    plan: &NativePoolConfiguration,
    limit: Duration,
) -> Result<(), WarmPoolError> {
    await_ready_with_fencing(
        store,
        provider,
        plan,
        Instant::now() + limit,
        StartupPoolPolicy::default(),
        None,
        None,
    )
    .await
    .map(|_| ())
}

pub(super) async fn await_ready_with_fencing(
    store: &dyn PoolTargetStore,
    provider: &dyn NativePoolControl,
    plan: &NativePoolConfiguration,
    deadline: Instant,
    policy: StartupPoolPolicy,
    admission: Option<&Admission>,
    writer: Option<StartupWriter<'_>>,
) -> Result<WarmPoolStartup, WarmPoolError> {
    let readiness = async {
        let mut failed_attempt = false;
        loop {
            if admission.is_some_and(Admission::is_draining) {
                return Ok(WarmPoolStartup::Stopping);
            }
            if tokio::time::Instant::now() >= deadline {
                break;
            }
            match boot_effect(writer, step(store, provider, plan, policy.target_fencing)).await {
                Ok(Some(outcome)) => return Ok(outcome),
                Ok(None) => {}
                Err(WarmPoolError::Conflict) => return Err(WarmPoolError::Conflict),
                Err(_) => {
                    failed_attempt = true;
                    tracing::warn!("default native pool startup readiness will retry");
                }
            }
            let retry = (tokio::time::Instant::now() + Duration::from_secs(1)).min(deadline);
            if let Some(admission) = admission {
                tokio::select! {
                    () = admission.stopping() => return Ok(WarmPoolStartup::Stopping),
                    () = tokio::time::sleep_until(retry) => (),
                }
            } else {
                tokio::time::sleep_until(retry).await;
            }
        }
        let Some(admission) = admission.filter(|_| policy.allow_empty && !failed_attempt) else {
            return Err(WarmPoolError::Unavailable);
        };
        tokio::select! {
            biased;
            () = admission.stopping() => Ok(WarmPoolStartup::Stopping),
            result = tokio::time::timeout(Duration::from_secs(8), confirm_spare_shortage(store, provider, plan)) => {
                result.map_err(|_| WarmPoolError::Unavailable)?
            }
        }
    };
    if admission.is_some() {
        // Once a step claims a target, its provider and publication deadlines
        // govern that attempt. The warmup limit bounds admission to new steps.
        readiness.await
    } else {
        tokio::time::timeout_at(deadline, readiness)
            .await
            .map_err(|_| WarmPoolError::Unavailable)?
    }
}

async fn confirm_spare_shortage(
    store: &dyn PoolTargetStore,
    provider: &dyn NativePoolControl,
    plan: &NativePoolConfiguration,
) -> Result<WarmPoolStartup, WarmPoolError> {
    let mut targets = store.list().await?.into_iter().filter(|target| {
        !target.machine && !target.retiring && !target.retired && &target.plan == plan
    });
    let target = targets.next().ok_or(WarmPoolError::Unavailable)?;
    let binding = target.binding.as_ref().ok_or(WarmPoolError::Unavailable)?;
    if targets.next().is_some()
        || target.target_replicas == 0
        || target.lease.is_some()
        || binding.spec != plan.spec
        || binding.pod_policy != plan.pod_policy
        || binding.config_sha256 != plan.config_sha256
    {
        return Err(WarmPoolError::Unavailable);
    }
    let inventory = provider
        .inventory()
        .await
        .map_err(|_| WarmPoolError::Unavailable)?;
    if observed_capacity(&target, &inventory) != Some(0) {
        return Err(WarmPoolError::Unavailable);
    }
    let confirmed =
        provider
            .observe_admitting_pool(binding)
            .await
            .map_err(|error| match error {
                layer3_adapter::AdapterError::Rejected(_) => WarmPoolError::Conflict,
                _ => WarmPoolError::Unavailable,
            })?;
    if observed_capacity(&target, &[confirmed]) != Some(0) {
        return Err(WarmPoolError::Unavailable);
    }
    let current = store
        .find(&target.id)
        .await?
        .ok_or(WarmPoolError::Unavailable)?;
    if current.revision != target.revision
        || current.plan != target.plan
        || current.binding != target.binding
        || current.target_replicas != target.target_replicas
        || current.applied_revision != current.revision
        || current.error.is_some()
        || current.lease.is_some()
        || current.machine
        || current.retiring
        || current.retired
    {
        return Err(WarmPoolError::Unavailable);
    }
    Ok(WarmPoolStartup::CapacityDeferred(ConfirmedSpareShortage(
        Box::new(current),
    )))
}

async fn step(
    store: &dyn PoolTargetStore,
    provider: &dyn NativePoolControl,
    plan: &NativePoolConfiguration,
    target_fencing: bool,
) -> Result<Option<WarmPoolStartup>, WarmPoolError> {
    let target = store
        .list()
        .await?
        .into_iter()
        .find(|target| &target.plan == plan)
        .ok_or(WarmPoolError::Unavailable)?;
    if target.retiring || target.retired {
        return Err(WarmPoolError::Conflict);
    }
    // The durable operator override wins over the deployment's configured target.
    if target.target_replicas == 0 {
        return Ok(Some(WarmPoolStartup::ZeroTarget));
    }
    if target.binding.is_some() && target.applied_revision == target.revision {
        let inventory = provider
            .inventory()
            .await
            .map_err(|_| WarmPoolError::Unavailable)?;
        if observed_capacity(&target, &inventory).is_some_and(|ready| ready > 0) {
            // An operator may resize while the provider observation is in flight.
            let current = store
                .find(&target.id)
                .await?
                .ok_or(WarmPoolError::Unavailable)?;
            if current.revision == target.revision
                && current.plan == target.plan
                && current.binding == target.binding
                && current.applied_revision == current.revision
                && current.target_replicas == target.target_replicas
                && current.error.is_none()
                && !current.retiring
                && !current.retired
            {
                return Ok(Some(WarmPoolStartup::Ready));
            }
            return Ok(None);
        }
    }
    if let Some(mut claimed) = store.claim_target(&target.id).await? {
        if &claimed.plan != plan || claimed.retiring || claimed.retired {
            return Err(WarmPoolError::Conflict);
        }
        if claimed.binding.is_none() {
            // Release preparation can create the exact pool before promotion.
            // Bind its original UIDs under the durable lease so normal initial
            // configure(0) cannot drain already-warm members during startup.
            let inventory = provider
                .inventory()
                .await
                .map_err(|_| WarmPoolError::Unavailable)?;
            let mut matching = inventory.into_iter().filter(|status| {
                status.binding.spec == plan.spec
                    && status.binding.pod_policy == plan.pod_policy
                    && status.binding.config_sha256 == plan.config_sha256
            });
            if let Some(existing) = matching.next() {
                if matching.next().is_some() {
                    return Err(WarmPoolError::Unavailable);
                }
                store.bind(&claimed, &existing.binding).await?;
                claimed.binding = Some(existing.binding);
            }
        }
        if !apply_claimed(store, provider, claimed, target_fencing).await? {
            return Err(WarmPoolError::Unavailable);
        }
    }
    Ok(None)
}

#[cfg(test)]
pub(super) fn observed_ready(target: &PoolTarget, inventory: &[NativePoolStatus]) -> bool {
    observed_capacity(target, inventory).is_some_and(|ready| ready > 0)
}

pub(super) fn observed_capacity(
    target: &PoolTarget,
    inventory: &[NativePoolStatus],
) -> Option<u32> {
    if target.error.is_some() || target.applied_revision != target.revision {
        return None;
    }
    let mut matching = inventory
        .iter()
        .filter(|status| target.binding.as_ref() == Some(&status.binding));
    let status = matching.next()?;
    (matching.next().is_none()
        && status.observed
        && status.desired_replicas == target.target_replicas)
        .then_some(status.ready_replicas)
}

//! Prevent the first allocation after promotion from taking an avoidable cold path.
use super::retirement::RetirementAttempt;
use super::{
    reconcile_claimed, NativePoolConfiguration, NativePoolSpec, NativePoolStatus, NativeWarmPools,
    PoolApply, PoolTarget, PoolTargetStore, WarmPoolError,
};
use layer3_adapter::{agent_sandbox::NativePoolControl, AdapterError};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;
use tokio::{sync::watch, time::Instant};

use crate::startup::Admission;

mod shortage;
#[cfg(test)]
pub(super) use shortage::await_ready_with_fencing;

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
    ConfigurationDrift,
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
        self.await_default_ready_by(admission, writer, Instant::now() + STARTUP_WARMUP_LIMIT)
            .await
    }

    pub(super) async fn await_default_ready_by(
        &self,
        admission: Option<&Admission>,
        writer: Option<StartupWriter<'_>>,
        deadline: Instant,
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
        let readiness = await_ready_steps(
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
        );
        if let Some(admission) = admission {
            let (settled, retiring) = watch::channel(false);
            let stopping = || {
                admission.is_draining()
                    || *retiring.borrow()
                    || (self.allow_empty_startup && Instant::now() >= deadline)
            };
            let (result, unsettled_retirement) = tokio::join!(
                async {
                    let result = readiness.await;
                    settled.send_replace(true);
                    result
                },
                async {
                    // Each admitted retirement must settle before either startup
                    // or shutdown continues, even if readiness wins the race.
                    let mut unsettled = BTreeMap::new();
                    while !stopping() {
                        self.retire_during_startup(&stopping, writer, &mut unsettled)
                            .await;
                        tokio::select! {
                            () = crate::shutdown::signalled(retiring.clone()) => break,
                            () = admission.stopping() => break,
                            () = tokio::time::sleep(Duration::from_secs(5)) => (),
                        }
                    }
                    unsettled
                },
            );
            if admission.is_draining() {
                return Ok(WarmPoolStartup::Stopping);
            }
            return match result? {
                Some(outcome) => Ok(outcome),
                None => {
                    shortage::confirm_after_retirement(
                        self.store.as_ref(),
                        provider,
                        &plan,
                        self.target_fencing,
                        admission,
                        &unsettled_retirement,
                    )
                    .await
                }
            };
        }
        // The supervisor that retires superseded pools starts only after this
        // gate, yet on a full node pool the new default schedules only once
        // their spare capacity drains. Retire superseded targets while waiting,
        // so a starting pod frees the room it needs.
        let drain = async {
            let mut unsettled = BTreeMap::new();
            loop {
                self.retire_during_startup(&|| false, writer, &mut unsettled)
                    .await;
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        };
        tokio::select! {
            result = readiness => result?.ok_or(WarmPoolError::Unavailable),
            _ = drain => unreachable!("superseded pool draining never completes"),
        }
    }

    async fn retire_during_startup(
        &self,
        stopping: &impl Fn() -> bool,
        writer: Option<StartupWriter<'_>>,
        unsettled: &mut BTreeMap<String, RetirementAttempt>,
    ) {
        if self
            .retire_superseded(stopping, writer, unsettled)
            .await
            .is_err()
        {
            tracing::warn!("superseded native pool retirement during startup will retry");
        }
    }

    /// Claims only retiring or superseded targets, never a current plan: the
    /// generic reconciler's unbound path would configure a freshly inserted
    /// default at zero replicas and empty a pool that release preparation
    /// prewarmed, which `step` deliberately adopts instead.
    async fn retire_superseded(
        &self,
        stopping: &impl Fn() -> bool,
        writer: Option<StartupWriter<'_>>,
        unsettled: &mut BTreeMap<String, RetirementAttempt>,
    ) -> Result<(), WarmPoolError> {
        let Some(provider) = self.adapter.native_pools() else {
            return Ok(());
        };
        for target in self.store.list().await? {
            if stopping() {
                break;
            }
            if self.default_holds.lane(target.machine)
                || target.retired
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
                    let previous =
                        unsettled.insert(target.id.clone(), RetirementAttempt::Unacknowledged);
                    if let Some(claimed) = self.store.claim_retirement(&target.id).await? {
                        let attempt = self.retire_claimed(provider, claimed).await?;
                        unsettled.insert(target.id.clone(), attempt);
                    } else if let Some(attempt) = previous {
                        unsettled.insert(target.id.clone(), attempt);
                    } else {
                        unsettled.remove(&target.id);
                    }
                }
                Ok(())
            })
            .await?;
        }
        Ok(())
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

async fn await_ready_steps(
    store: &dyn PoolTargetStore,
    provider: &dyn NativePoolControl,
    plan: &NativePoolConfiguration,
    deadline: Instant,
    policy: StartupPoolPolicy,
    admission: Option<&Admission>,
    writer: Option<StartupWriter<'_>>,
) -> Result<Option<WarmPoolStartup>, WarmPoolError> {
    let readiness = async {
        let mut unsettled = BTreeSet::new();
        loop {
            if admission.is_some_and(Admission::is_draining) {
                return Ok(Some(WarmPoolStartup::Stopping));
            }
            if tokio::time::Instant::now() >= deadline {
                break;
            }
            match boot_effect(
                writer,
                step(store, provider, plan, policy.target_fencing, &mut unsettled),
            )
            .await
            {
                Ok(Some(outcome)) => return Ok(Some(outcome)),
                Ok(None) => {}
                Err(WarmPoolError::ProviderConfigurationDrift) => {
                    tracing::warn!(target: "cma_warm_pool_startup", parent: None,
                        reason = "native_pool_configuration_drift", config_sha256 = %plan.config_sha256,
                        "Agent Manager will serve with a degraded default native pool");
                    return Ok(Some(WarmPoolStartup::ConfigurationDrift));
                }
                Err(WarmPoolError::Conflict) => return Err(WarmPoolError::Conflict),
                Err(_) => tracing::warn!("default native pool startup readiness will retry"),
            }
            let retry = (tokio::time::Instant::now() + Duration::from_secs(1)).min(deadline);
            if let Some(admission) = admission {
                tokio::select! {
                    () = admission.stopping() => return Ok(Some(WarmPoolStartup::Stopping)),
                    () = tokio::time::sleep_until(retry) => (),
                }
            } else {
                tokio::time::sleep_until(retry).await;
            }
        }
        if admission.is_some() && policy.allow_empty && unsettled.is_empty() {
            Ok(None)
        } else {
            Err(WarmPoolError::Unavailable)
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

async fn step(
    store: &dyn PoolTargetStore,
    provider: &dyn NativePoolControl,
    plan: &NativePoolConfiguration,
    target_fencing: bool,
    unsettled: &mut BTreeSet<String>,
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
        let provider_target = provider_target(&target);
        let inventory = match provider.inventory().await {
            Ok(inventory) => inventory,
            Err(
                AdapterError::NativePoolConfigurationDrift
                | AdapterError::NativePoolDefinitionAbsent { .. },
            ) => {
                let binding = target.binding.as_ref().ok_or(WarmPoolError::Conflict)?;
                let observation = provider
                    .observe_admitting_pool(binding, &provider_target, target_fencing)
                    .await;
                if matches!(
                    &observation,
                    Err(AdapterError::NativePoolConfigurationDrift)
                ) {
                    if current_target(store, &target).await? {
                        return Err(WarmPoolError::ProviderConfigurationDrift);
                    }
                    return Ok(None);
                }
                vec![observation.map_err(observation_error)?]
            }
            Err(_) => return Err(WarmPoolError::Unavailable),
        };
        if observed_capacity(&target, &inventory).is_some_and(|ready| ready > 0) {
            if current_target(store, &target).await? {
                return Ok(Some(WarmPoolStartup::Ready));
            }
            return Ok(None);
        }
    }
    let new = unsettled.insert(target.id.clone());
    if let Some(mut claimed) = store.claim_target(&target.id).await? {
        if &claimed.plan != plan || claimed.retiring || claimed.retired {
            return Err(WarmPoolError::Conflict);
        }
        if claimed.binding.is_none() {
            // Release preparation can create the exact pool before promotion.
            // Bind its original UIDs under the durable lease so normal initial
            // configure(0) cannot drain already-warm members during startup.
            let target = provider_target(&claimed);
            match provider
                .observe_planned_pool(plan, &target, target_fencing)
                .await
            {
                Ok(Some(existing)) => {
                    store.bind(&claimed, &existing.binding).await?;
                    claimed.binding = Some(existing.binding);
                }
                Ok(None) => {}
                Err(AdapterError::NativePoolConfigurationDrift) => {
                    store.finish(&claimed, false).await?;
                    return Err(WarmPoolError::ProviderConfigurationDrift);
                }
                Err(error) => return Err(observation_error(error)),
            }
        }
        match reconcile_claimed(store, provider, claimed, target_fencing).await? {
            PoolApply::Settled => {
                unsettled.remove(&target.id);
            }
            PoolApply::ConfigurationDrift => return Err(WarmPoolError::ProviderConfigurationDrift),
            PoolApply::Failed => return Err(WarmPoolError::Unavailable),
        }
    } else if new {
        unsettled.remove(&target.id);
    }
    Ok(None)
}

fn provider_target(target: &PoolTarget) -> layer3_adapter::agent_sandbox::NativePoolTarget {
    layer3_adapter::agent_sandbox::NativePoolTarget {
        target_id: target.id.clone(),
        revision: target.revision,
        replicas: target.target_replicas,
    }
}

fn observation_error(error: AdapterError) -> WarmPoolError {
    match error {
        AdapterError::Rejected(_) => WarmPoolError::Conflict,
        _ => WarmPoolError::Unavailable,
    }
}

async fn current_target(
    store: &dyn PoolTargetStore,
    target: &PoolTarget,
) -> Result<bool, WarmPoolError> {
    let current = store
        .find(&target.id)
        .await?
        .ok_or(WarmPoolError::Unavailable)?;
    Ok(current.revision == target.revision
        && current.plan == target.plan
        && current.binding == target.binding
        && current.applied_revision == current.revision
        && current.target_replicas == target.target_replicas
        && current.error.is_none()
        && !current.retiring
        && !current.retired)
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

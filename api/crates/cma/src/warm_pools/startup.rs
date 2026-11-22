//! Give a promoted default pool a bounded head start; warm capacity never gates serving.
//!
//! A ready member only spares the first allocation after promotion a cold
//! start. An allocation against a pool without one still works: the controller
//! creates the claimed Sandbox from the pool's template, and the allocation's
//! durable work retries until the claim is assigned.
use super::{
    reconcile_claimed, NativePoolConfiguration, NativePoolSpec, NativePoolStatus, NativeWarmPools,
    PoolTarget, PoolTargetStore, WarmPoolError,
};
use layer3_adapter::agent_sandbox::NativePoolControl;
use std::time::Duration;

// Wait no longer than the release settle window. Elapsing is not a failure:
// startup serves `Degraded` while the supervisor keeps draining older spares
// and filling the pool. A 120 s bound once "always gave up exactly when older
// capacity became eligible to drain", but that only mattered while giving up
// meant exiting. A longer wait now buys little and costs three things:
// - All of startup must fit inside the default 300 s startup probe:
//   composition and `ensure_default` before this wait, then socket bind,
//   sandbox initialization, worker start and the first `/ready` after it.
// - Once the settle window passes, the previous pool may be retired, and
//   previous pods still serving during the rollout then refuse every new
//   allocation against it. Waiting longer only keeps them serving longer.
// - A rolling update starts replicas one after another, and each pays this
//   wait inside the same rollout deadline.
pub(super) const STARTUP_WARMUP_LIMIT: Duration = super::RELEASE_SETTLE;

/// How the bounded startup wait for the default runtime pool ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StartupReadiness {
    /// The exact plan reported ready capacity, or an operator's durable target is zero.
    Ready,
    /// The bound elapsed first. Serving continues and allocations may start cold.
    Degraded,
}

/// The plan whose startup wait elapsed, until it first reports ready capacity.
pub(super) struct DegradedStart {
    pub(super) config_sha256: String,
    pub(super) at: tokio::time::Instant,
}

/// The default target as the latest readiness step found it. A replaced pool
/// UID or a sticky reconcile error shows here, not as missing capacity.
#[derive(Clone, Copy, Debug, Default)]
struct TargetState {
    recorded: bool,
    bound: bool,
    applied: bool,
    errored: bool,
}

impl TargetState {
    fn of(target: Option<&PoolTarget>) -> Self {
        target.map_or_else(Self::default, |target| Self {
            recorded: true,
            bound: target.binding.is_some(),
            applied: target.applied_revision == target.revision,
            errored: target.error.is_some(),
        })
    }
}

impl NativeWarmPools {
    /// Wait, bounded, for the exact default runtime plan to report ready
    /// capacity. During a rolling update the previous Agent Manager keeps
    /// serving while this waits. Release preparation may have prewarmed the
    /// plan, but nothing guarantees it, so a new image's pull, scheduling and
    /// spare drain can outlast the bound. That returns `Degraded` and the
    /// caller serves anyway: the pool supervisor keeps filling the pool, and
    /// allocations meanwhile take the cold path.
    ///
    /// Only a retiring or retired default target (`Conflict`) or an unusable
    /// plan is an error. A retiring pool refuses every new admission, so
    /// serving against it would turn allocations into failures.
    ///
    /// Only the default runtime plan gates startup. The shared desktop pool
    /// merely shortens machine startup, which still works cold, and a new
    /// machine image's first pull alone can outlast this guard; the supervisor
    /// fills that pool in the background.
    pub async fn await_default_ready(&self) -> Result<StartupReadiness, WarmPoolError> {
        // The supervisor that retires superseded pools starts only after this
        // gate, yet on a full node pool the new default schedules only once
        // their spare capacity drains. Retire superseded targets while waiting,
        // so a starting pod frees the room it needs.
        let drain = async {
            loop {
                if self.retire_superseded().await.is_err() {
                    tracing::warn!("superseded native pool retirement during startup will retry");
                }
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        };
        tokio::select! {
            result = self.await_default_plan_ready() => result,
            _ = drain => unreachable!("superseded pool draining never completes"),
        }
    }

    /// Claims only retiring or superseded targets, never a current plan: the
    /// generic reconciler's unbound path would configure a freshly inserted
    /// default at zero replicas and empty a pool that release preparation
    /// prewarmed, which `step` deliberately adopts instead.
    async fn retire_superseded(&self) -> Result<(), WarmPoolError> {
        let Some(provider) = self.adapter.native_pools() else {
            return Ok(());
        };
        for target in self.store.list().await? {
            if target.retired || (!target.retiring && self.current_plan(&target)?) {
                continue;
            }
            // Plans are immutable and retirement only advances, so a claimed
            // superseded target is still superseded. A target an earlier pod
            // marked retiring and deferred is resumed, not skipped.
            if let Some(claimed) = self.store.claim_retirement(&target.id).await? {
                self.retire_claimed(provider, claimed).await?;
            }
        }
        Ok(())
    }

    async fn await_default_plan_ready(&self) -> Result<StartupReadiness, WarmPoolError> {
        let provider = self
            .adapter
            .native_pools()
            .ok_or(WarmPoolError::Unavailable)?;
        let plan = provider
            .plan(&NativePoolSpec {
                image: self.image.clone(),
                cpu_millis: self.default_capacity.cpu_millis,
                memory_mib: self.default_capacity.memory_mib,
                home: self.home.clone().ok_or(WarmPoolError::Unavailable)?,
            })
            .map_err(|_| WarmPoolError::Unavailable)?;
        let readiness = await_ready_with_fencing(
            self.store.as_ref(),
            provider,
            &plan,
            STARTUP_WARMUP_LIMIT,
            self.target_fencing,
        )
        .await?;
        if readiness == StartupReadiness::Degraded {
            *self.degraded_slot() = Some(DegradedStart {
                config_sha256: plan.config_sha256,
                at: tokio::time::Instant::now(),
            });
        }
        Ok(readiness)
    }

    /// Log once when the plan that missed the startup bound first reports
    /// ready capacity, using the observation sampler's existing inventory.
    pub(super) fn note_default_recovery(
        &self,
        targets: &[PoolTarget],
        inventory: &[NativePoolStatus],
    ) {
        let mut degraded = self.degraded_slot();
        let Some(start) = degraded.as_ref() else {
            return;
        };
        let ready = targets.iter().any(|target| {
            !target.machine
                && target.plan.config_sha256 == start.config_sha256
                && observed_ready(target, inventory)
        });
        if ready {
            tracing::info!(
                config_sha256 = %start.config_sha256,
                elapsed_ms = start.at.elapsed().as_millis(),
                "default native pool became ready after a degraded start"
            );
            *degraded = None;
        }
    }

    fn degraded_slot(&self) -> std::sync::MutexGuard<'_, Option<DegradedStart>> {
        self.degraded_start
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
pub(super) async fn await_ready(
    store: &dyn PoolTargetStore,
    provider: &dyn NativePoolControl,
    plan: &NativePoolConfiguration,
    limit: Duration,
) -> Result<StartupReadiness, WarmPoolError> {
    await_ready_with_fencing(store, provider, plan, limit, false).await
}

async fn await_ready_with_fencing(
    store: &dyn PoolTargetStore,
    provider: &dyn NativePoolControl,
    plan: &NativePoolConfiguration,
    limit: Duration,
    target_fencing: bool,
) -> Result<StartupReadiness, WarmPoolError> {
    let started = tokio::time::Instant::now();
    let mut seen = TargetState::default();
    let waited = tokio::time::timeout(limit, async {
        loop {
            match step(store, provider, plan, target_fencing, &mut seen).await {
                Ok(true) => return Ok(StartupReadiness::Ready),
                Ok(false) => {}
                Err(WarmPoolError::Conflict) => return Err(WarmPoolError::Conflict),
                Err(_) => tracing::warn!("default native pool startup readiness will retry"),
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    })
    .await;
    // Dropping an in-flight step here is the same interruption as a process
    // exit at that point; the durable lease and target record cover both.
    waited.unwrap_or_else(|_| {
        tracing::warn!(
            config_sha256 = %plan.config_sha256,
            image = %plan.spec.image.uri,
            elapsed_ms = started.elapsed().as_millis(),
            target_recorded = seen.recorded,
            target_bound = seen.bound,
            revision_applied = seen.applied,
            target_error = seen.errored,
            "default native pool not ready within startup limit; serving while the supervisor fills it"
        );
        Ok(StartupReadiness::Degraded)
    })
}

async fn step(
    store: &dyn PoolTargetStore,
    provider: &dyn NativePoolControl,
    plan: &NativePoolConfiguration,
    target_fencing: bool,
    seen: &mut TargetState,
) -> Result<bool, WarmPoolError> {
    let target = store
        .list()
        .await?
        .into_iter()
        .find(|target| &target.plan == plan);
    *seen = TargetState::of(target.as_ref());
    let target = target.ok_or(WarmPoolError::Unavailable)?;
    if target.retiring || target.retired {
        return Err(WarmPoolError::Conflict);
    }
    // The durable operator override wins over the deployment's configured target.
    if target.target_replicas == 0 {
        return Ok(true);
    }
    if target.binding.is_some() && target.applied_revision == target.revision {
        let inventory = provider
            .inventory()
            .await
            .map_err(|_| WarmPoolError::Unavailable)?;
        if observed_ready(&target, &inventory) {
            // An operator may resize while the provider observation is in flight.
            let current = store
                .find(&target.id)
                .await?
                .ok_or(WarmPoolError::Unavailable)?;
            if current.revision == target.revision
                && current.plan == target.plan
                && current.binding == target.binding
                && !current.retiring
                && !current.retired
            {
                return Ok(true);
            }
            return Ok(false);
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
        reconcile_claimed(store, provider, claimed, target_fencing).await?;
    }
    Ok(false)
}

pub(super) fn observed_ready(target: &PoolTarget, inventory: &[NativePoolStatus]) -> bool {
    target.error.is_none()
        && target.applied_revision == target.revision
        && inventory.iter().any(|status| {
            target.binding.as_ref() == Some(&status.binding)
                && status.observed
                && status.desired_replicas == target.target_replicas
                && status.ready_replicas > 0
        })
}

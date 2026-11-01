//! Prevent the first allocation after promotion from taking an avoidable cold path.
use super::{
    reconcile_claimed, NativePoolConfiguration, NativePoolSpec, NativePoolStatus, NativeWarmPools,
    PoolTarget, PoolTargetStore, WarmPoolError,
};
use layer3_adapter::agent_sandbox::NativePoolControl;
use std::time::Duration;

const STARTUP_WARMUP_LIMIT: Duration = Duration::from_secs(120);

impl NativeWarmPools {
    /// Release preparation should prewarm this plan before the Recreate rollout.
    /// This bounded guard never serves allocations before readiness and does not
    /// imply that the old API remains available during a Recreate deployment.
    pub async fn await_default_ready(&self) -> Result<(), WarmPoolError> {
        let provider = self
            .adapter
            .native_pools()
            .ok_or(WarmPoolError::Unavailable)?;
        let mut images = vec![&self.image];
        if let Some((image, _)) = &self.machine {
            images.push(image);
        }
        for image in images {
            let plan = provider
                .plan(&NativePoolSpec {
                    image: image.clone(),
                    cpu_millis: self.default_capacity.cpu_millis,
                    memory_mib: self.default_capacity.memory_mib,
                    home: self.home.clone().ok_or(WarmPoolError::Unavailable)?,
                })
                .map_err(|_| WarmPoolError::Unavailable)?;
            await_ready(self.store.as_ref(), provider, &plan, STARTUP_WARMUP_LIMIT).await?;
        }
        Ok(())
    }
}

pub(super) async fn await_ready(
    store: &dyn PoolTargetStore,
    provider: &dyn NativePoolControl,
    plan: &NativePoolConfiguration,
    limit: Duration,
) -> Result<(), WarmPoolError> {
    tokio::time::timeout(limit, async {
        loop {
            match step(store, provider, plan).await {
                Ok(true) => return Ok(()),
                Ok(false) => {}
                Err(WarmPoolError::Conflict) => return Err(WarmPoolError::Conflict),
                Err(_) => tracing::warn!("default native pool startup readiness will retry"),
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    })
    .await
    .map_err(|_| WarmPoolError::Unavailable)?
}

async fn step(
    store: &dyn PoolTargetStore,
    provider: &dyn NativePoolControl,
    plan: &NativePoolConfiguration,
) -> Result<bool, WarmPoolError> {
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
        reconcile_claimed(store, provider, claimed).await?;
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

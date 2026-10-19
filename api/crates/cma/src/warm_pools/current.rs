//! Retire spare capacity without removing original pool or claimed-home authority.
use super::{NativeWarmPools, PoolTarget, PoolTargetStore, ResizeWarmPoolBody, WarmPoolError};
use layer3_adapter::{agent_sandbox::NativePoolControl, HomeVolumePolicy, RuntimeImage};

impl NativeWarmPools {
    pub(super) fn current_plan(&self, target: &PoolTarget) -> Result<bool, WarmPoolError> {
        let provider = self
            .adapter
            .native_pools()
            .ok_or(WarmPoolError::Unavailable)?;
        let home = self.home.as_ref().ok_or(WarmPoolError::Unavailable)?;
        current_plan(provider, &self.image, home, target)
    }

    pub(super) async fn retire_superseded(&self) -> Result<bool, WarmPoolError> {
        retire_superseded(self.store.as_ref(), |target| self.current_plan(target)).await
    }
}

pub(super) fn current_plan(
    provider: &dyn NativePoolControl,
    image: &RuntimeImage,
    home: &HomeVolumePolicy,
    target: &PoolTarget,
) -> Result<bool, WarmPoolError> {
    if &target.plan.spec.image != image || &target.plan.spec.home != home {
        return Ok(false);
    }
    let current = provider
        .plan(&target.plan.spec)
        .map_err(|_| WarmPoolError::Unavailable)?;
    Ok(current == target.plan)
}

async fn retire_superseded(
    store: &dyn PoolTargetStore,
    current: impl Fn(&PoolTarget) -> Result<bool, WarmPoolError>,
) -> Result<bool, WarmPoolError> {
    let mut complete = true;
    for target in store.list().await? {
        if target.target_replicas == 0 || current(&target)? {
            continue;
        }
        // The same revision/lease fence as an operator resize. A running
        // reconciler finishes first; the next tick retries a changed target.
        match store
            .resize(
                &target.id,
                &ResizeWarmPoolBody {
                    expected_revision: target.revision as u64,
                    target_replicas: 0,
                },
            )
            .await
        {
            Ok(_) => {}
            Err(WarmPoolError::Conflict) => complete = false,
            Err(error) => return Err(error),
        }
    }
    Ok(complete)
}

#[cfg(test)]
mod tests;

//! Retire spare capacity without removing original pool or claimed-home authority.
use super::{NativeWarmPools, PoolTarget, WarmPoolError};
use layer3_adapter::{agent_sandbox::NativePoolControl, HomeVolumePolicy, RuntimeImage};

impl NativeWarmPools {
    pub(super) fn current_plan(&self, target: &PoolTarget) -> Result<bool, WarmPoolError> {
        let provider = self
            .adapter
            .native_pools()
            .ok_or(WarmPoolError::Unavailable)?;
        let home = self.home.as_ref().ok_or(WarmPoolError::Unavailable)?;
        let released = self.released_images();
        let image = if target.machine {
            let Some((image, _)) = released.machine else {
                return Ok(false);
            };
            image
        } else {
            released.agent
        };
        current_plan(provider, &image, home, target)
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

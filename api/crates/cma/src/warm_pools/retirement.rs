//! Fence admission, recheck durable users, then drain only unused pool capacity.
use super::*;
use layer3_adapter::agent_sandbox::NativePoolControl;

impl NativeWarmPools {
    pub(super) async fn retire_claimed(
        &self,
        provider: &dyn NativePoolControl,
        target: PoolTarget,
    ) -> Result<bool, WarmPoolError> {
        let image = if target.machine {
            // Removing the configured machine image does not grant an agent-only
            // release authority to retire desktop capacity.
            let Some((image, _)) = &self.machine else {
                self.store.finish(&target, false).await?;
                return Ok(true);
            };
            &image.uri
        } else {
            &self.image.uri
        };
        retire_claimed(self.store.as_ref(), provider, image, target).await
    }
}

pub(super) async fn retire_claimed(
    store: &dyn PoolTargetStore,
    provider: &dyn NativePoolControl,
    image: &str,
    mut target: PoolTarget,
) -> Result<bool, WarmPoolError> {
    let result = tokio::time::timeout(Duration::from_secs(30), async {
        if store
            .retirement_drain_homes(&target, image)
            .await?
            .is_none()
        {
            return Err(WarmPoolError::Unavailable);
        }
        if target.binding.is_none() {
            // Recover an uncertain configure acknowledgement before marking
            // the original definitions. Never create positive old capacity.
            let status = provider
                .configure_planned(&target.plan, 0)
                .await
                .map_err(|_| WarmPoolError::Unavailable)?;
            store.bind(&target, &status.binding).await?;
            target.binding = Some(status.binding);
        }
        target = store.begin_retirement(&target).await?;
        let binding = target.binding.as_ref().ok_or(WarmPoolError::Conflict)?;
        provider
            .begin_retirement(binding)
            .await
            .map_err(|_| WarmPoolError::Unavailable)?;
        let retirement_homes = store.retirement_homes(&target, image).await?;
        let Some(drain_homes) = store.retirement_drain_homes(&target, image).await? else {
            return Err(WarmPoolError::Unavailable);
        };
        // retire_pool handles absent definitions idempotently, and only
        // succeeds after both definitions and all spare graphs are absent.
        if let Some(homes) = retirement_homes {
            if provider
                .retire_pool(binding, &homes)
                .await
                .map_err(|_| WarmPoolError::Unavailable)?
            {
                store.remove_retired(&target).await?;
                return Ok(true);
            }
        }
        // Settled retained homes can prevent definition deletion without
        // keeping spare capacity alive. Admission is fenced and all durable
        // allocation sources have been rechecked after the provider marker.
        provider
            .drain_pool(binding, &drain_homes)
            .await
            .map_err(|_| WarmPoolError::Unavailable)?;
        Ok::<bool, WarmPoolError>(false)
    })
    .await;
    match result {
        Ok(Ok(true)) => (),
        Ok(Ok(false)) => store.finish(&target, true).await?,
        _ => store.finish(&target, false).await?,
    }
    Ok(true)
}

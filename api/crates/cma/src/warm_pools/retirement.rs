//! Fence admission, recheck durable users, then drain only unused pool capacity.
use super::*;
use layer3_adapter::agent_sandbox::NativePoolControl;

impl NativeWarmPools {
    pub(super) async fn retire_claimed(
        &self,
        provider: &dyn NativePoolControl,
        target: PoolTarget,
    ) -> Result<bool, WarmPoolError> {
        retire_claimed(self.store.as_ref(), provider, &self.image.uri, target).await
    }
}

pub(super) async fn retire_claimed(
    store: &dyn PoolTargetStore,
    provider: &dyn NativePoolControl,
    image: &str,
    mut target: PoolTarget,
) -> Result<bool, WarmPoolError> {
    let result = tokio::time::timeout(Duration::from_secs(30), async {
        if store.retirement_homes(&target, image).await?.is_none() {
            return Ok(false);
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
        let Some(homes) = store.retirement_homes(&target, image).await? else {
            return Ok(false);
        };
        // retire_pool handles absent definitions idempotently, and only
        // succeeds after both definitions and all spare graphs are absent.
        if provider
            .retire_pool(binding, &homes)
            .await
            .map_err(|_| WarmPoolError::Unavailable)?
        {
            store.remove_retired(&target).await?;
            return Ok(true);
        }
        provider
            .resize(binding, 0)
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

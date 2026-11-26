//! Fence admission, recheck durable users, then drain only unused pool capacity.
use super::*;
use layer3_adapter::agent_sandbox::NativePoolControl;

impl NativeWarmPools {
    pub(super) async fn retire_claimed(
        &self,
        provider: &dyn NativePoolControl,
        target: PoolTarget,
    ) -> Result<bool, WarmPoolError> {
        let released = self.released_images();
        let image = if target.machine {
            // Removing the configured machine image does not grant an agent-only
            // release authority to retire desktop capacity.
            let Some((image, _)) = released.machine else {
                self.store.finish(&target, false).await?;
                return Ok(true);
            };
            image.uri
        } else {
            released.agent.uri
        };
        retire_claimed(self.store.as_ref(), provider, &image, target).await
    }
}

pub(super) async fn retire_claimed(
    store: &dyn PoolTargetStore,
    provider: &dyn NativePoolControl,
    image: &str,
    mut target: PoolTarget,
) -> Result<bool, WarmPoolError> {
    let result = tokio::time::timeout(Duration::from_secs(30), async {
        // A saved pool request can still need its first Claim. A permanent
        // marker would refuse that admitted operation before it can resume.
        if store
            .retirement_drain_homes(&target, image)
            .await?
            .is_none()
        {
            return Err(WarmPoolError::Unavailable);
        }
        target = store.begin_retirement(&target).await?;
        if target.binding.is_none() {
            // Recover an unacknowledged binding without changing provider replicas.
            let binding = provider
                .inventory()
                .await
                .map_err(|_| WarmPoolError::Unavailable)?
                .into_iter()
                .map(|status| status.binding)
                .find(|binding| {
                    binding.config_sha256 == target.plan.config_sha256
                        && binding.spec == target.plan.spec
                        && binding.pod_policy == target.plan.pod_policy
                })
                .ok_or(WarmPoolError::Unavailable)?;
            store.bind(&target, &binding).await?;
            target.binding = Some(binding);
        }
        let binding = target.binding.as_ref().ok_or(WarmPoolError::Conflict)?;
        provider
            .begin_retirement(binding)
            .await
            .map_err(|_| WarmPoolError::Unavailable)?;
        let retirement_homes = store.retirement_homes(&target, image).await?;
        let Some(drain) = store.retirement_drain_homes(&target, image).await? else {
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
        if target.machine {
            provider.drain_pool(binding, &drain).await
        } else {
            provider.request_spare_drain(binding, &drain).await
        }
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

//! Prepare the next fixed pin while the incumbent remains the only routed image.
use super::*;
use layer3_adapter::agent_sandbox::{NativePoolControl, NativePoolTarget};

impl NativeWarmPools {
    pub(crate) fn with_unbound_retirement_fencing(
        mut self: Arc<Self>,
        epoch: Option<uuid::Uuid>,
    ) -> Arc<Self> {
        Arc::make_mut(&mut self).pool_create_cutover_epoch = epoch;
        self
    }

    pub(super) async fn insert_target(
        &self,
        target: PoolTarget,
    ) -> Result<PoolTarget, WarmPoolError> {
        if self.pool_create_cutover_epoch.is_some() {
            self.store.insert_with_create_fence(target).await
        } else {
            self.store.insert(target).await
        }
    }

    pub(crate) fn with_fixed_pool_prewarm(
        mut self: Arc<Self>,
        images: [Option<String>; 2],
    ) -> Arc<Self> {
        Arc::make_mut(&mut self).fixed_pool_prewarm = images;
        self
    }

    pub(super) async fn ensure_fixed_lane(
        &self,
        image: &RuntimeImage,
        replicas: u32,
        machine: bool,
    ) -> Result<Option<PoolTarget>, WarmPoolError> {
        let Some(candidate) = &self.fixed_pool_prewarm[usize::from(machine)] else {
            self.ensure_fixed_image(image, replicas, machine).await?;
            return Ok(None);
        };
        if candidate != &image.uri {
            self.ensure_fixed_image(image, replicas, machine).await?;
            let active = self.active_default_target(&image.uri, machine).await?;
            if active.target_replicas == 0 {
                tracing::warn!(
                    machine,
                    "fixed pool prewarm requires an operator-authorized positive target"
                );
                return Ok(None);
            }
            let target = self.candidate_target(candidate, &active)?;
            self.store.prepare_fixed_pool(&active, target).await?;
            return Ok(None);
        }

        let target = self.active_default_target(candidate, machine).await?;
        let provider = self
            .adapter
            .native_pools()
            .ok_or(WarmPoolError::Unavailable)?;
        observe_fixed_spare(provider, &target, self.target_fencing).await?;
        Ok(Some(target))
    }

    pub(super) async fn matches_fixed_preparation(
        &self,
        target: &PoolTarget,
    ) -> Result<bool, WarmPoolError> {
        if self.fixed_pool_prewarm[usize::from(target.machine)].as_deref()
            != Some(target.plan.spec.image.uri.as_str())
        {
            return Ok(false);
        }
        let released = self.released_images();
        let incumbent = if target.machine {
            let Some((image, _)) = released.machine else {
                return Ok(false);
            };
            image
        } else {
            released.agent
        };
        let active = self
            .active_default_target(&incumbent.uri, target.machine)
            .await?;
        self.store.fixed_pool_prepared_from(target, &active).await
    }
}

pub(crate) async fn observe_fixed_spare(
    provider: &dyn NativePoolControl,
    target: &PoolTarget,
    target_fencing: bool,
) -> Result<(), WarmPoolError> {
    let binding = target.binding.as_ref().ok_or(WarmPoolError::Conflict)?;
    if target.target_replicas == 0
        || target.error.is_some()
        || target.lease.is_some()
        || target.applied_revision != target.revision
    {
        return Err(WarmPoolError::Unavailable);
    }
    let expected = NativePoolTarget {
        target_id: target.id.clone(),
        revision: target.revision,
        replicas: target.target_replicas,
    };
    let observation = provider.observe_admitting_pool(binding, &expected, target_fencing);
    let observed = tokio::time::timeout(Duration::from_secs(8), observation)
        .await
        .map_err(|_| WarmPoolError::Unavailable)?
        .map_err(|_| WarmPoolError::Unavailable)?;
    if &observed.binding != binding
        || !observed.observed
        || observed.desired_replicas != target.target_replicas
        || observed.ready_replicas == 0
    {
        return Err(WarmPoolError::Unavailable);
    }
    Ok(())
}

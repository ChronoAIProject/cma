//! Persist independently pinned agent and desktop pool defaults.
use super::*;

impl NativeWarmPools {
    /// Persist the deployment-owned default target before the reconciler starts.
    /// The target is keyed by the exact immutable plan, so a runtime image or
    /// Pod-policy change creates a new pool identity and never mutates a claimed
    /// pool. Provider resources are still created only by the normal reconciler.
    pub async fn ensure_default(&self) -> Result<(), WarmPoolError> {
        self.ensure_image(&self.image, self.default_target, false)
            .await?;
        if let Some((image, target)) = &self.machine {
            self.ensure_image(image, *target, true).await?;
        }
        Ok(())
    }

    async fn ensure_image(
        &self,
        image: &RuntimeImage,
        replicas: u32,
        machine: bool,
    ) -> Result<(), WarmPoolError> {
        let provider = self
            .adapter
            .native_pools()
            .ok_or(WarmPoolError::Unavailable)?;
        let home = self.home.clone().ok_or(WarmPoolError::Unavailable)?;
        let body = ConfigureWarmPoolBody {
            cpu_millis: self.default_capacity.cpu_millis,
            memory_mib: self.default_capacity.memory_mib,
            target_replicas: replicas,
        };
        let plan = provider
            .plan(&NativePoolSpec {
                image: image.clone(),
                cpu_millis: body.cpu_millis,
                memory_mib: body.memory_mib,
                home,
            })
            .map_err(|_| WarmPoolError::Invalid)?;
        // An operator may already have configured this exact immutable plan
        // under another idempotency key. Reuse that durable target instead of
        // tripping the collection's unique plan index during startup.
        if let Some(existing) = self
            .store
            .list()
            .await?
            .into_iter()
            .find(|target| target.plan.config_sha256 == plan.config_sha256)
        {
            if existing.machine != machine {
                return Err(WarmPoolError::Conflict);
            }
            return self.store.activate(&existing.id).await;
        }
        let mut target = default_target(plan, body);
        target.machine = machine;
        if machine {
            target.document_schema_version = 2;
        }
        let target = self.store.insert(target).await?;
        self.store.activate(&target.id).await
    }
}

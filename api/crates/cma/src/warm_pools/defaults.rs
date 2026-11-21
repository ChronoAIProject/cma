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
        let plan = provider
            .plan(&NativePoolSpec {
                image: image.clone(),
                cpu_millis: self.default_capacity.cpu_millis,
                memory_mib: self.default_capacity.memory_mib,
                home,
            })
            .map_err(|_| WarmPoolError::Invalid)?;
        let targets = self.store.list().await?;
        // An operator may already have configured this exact immutable plan
        // under another idempotency key. Reuse that durable target instead of
        // tripping the collection's unique plan index during startup.
        if let Some(existing) = targets
            .iter()
            .find(|target| target.plan.config_sha256 == plan.config_sha256)
        {
            if existing.machine != machine {
                return Err(WarmPoolError::Conflict);
            }
            return self.store.activate(&existing.id).await;
        }
        // Machine images change far more often than the deployment does. A new
        // machine release keeps the operator's desired count; the deployment
        // value applies only until an operator first resizes a machine pool.
        let operator_replicas = if machine {
            self.machine_operator_replicas(&targets).await?
        } else {
            None
        };
        let body = ConfigureWarmPoolBody {
            cpu_millis: self.default_capacity.cpu_millis,
            memory_mib: self.default_capacity.memory_mib,
            target_replicas: operator_replicas.map_or(replicas, |choice| choice.replicas),
        };
        let mut target = default_target(plan, body);
        target.machine = machine;
        if machine {
            target.document_schema_version = 2;
            target.operator_replicas = operator_replicas;
        }
        // Concurrent starters derive the same id; the store keeps one insert.
        let target = self.store.insert(target).await?;
        self.store.activate(&target.id).await
    }

    /// The new target copies the decision, so it outlives its source's
    /// retirement and is never taken from an agent (`machine: false`) pool.
    async fn machine_operator_replicas(
        &self,
        targets: &[PoolTarget],
    ) -> Result<Option<OperatorReplicas>, WarmPoolError> {
        if let Some(choice) = self.store.latest_machine_operator_replicas().await? {
            return Ok(Some(choice));
        }
        // Earlier binaries resized without recording a decision. Only an
        // operator resize advances a live system machine target's revision;
        // read the newest release alone, as older ones may have been drained.
        Ok(targets
            .iter()
            .filter(|target| target.machine && !target.retiring)
            .max_by_key(|target| (target.activated_at, target.revision))
            .filter(|target| target.revision > 1)
            .map(|target| OperatorReplicas {
                replicas: target.target_replicas,
                at: mongodb::bson::DateTime::now(),
            }))
    }
}

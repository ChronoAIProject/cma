//! Persist independently pinned agent and desktop pool defaults.
use super::{startup::WarmPoolStartup, *};
use crate::startup::Admission;

impl NativeWarmPools {
    pub(crate) async fn await_fixed_default_ready(
        &self,
        admission: Option<&Admission>,
    ) -> Result<WarmPoolStartup, WarmPoolError> {
        match self.await_default_ready_until(admission).await {
            Err(WarmPoolError::Conflict)
                if self
                    .hold_conflicting_pin(&self.released_images().agent, false)
                    .await? =>
            {
                Ok(WarmPoolStartup::ConfigurationDrift)
            }
            result => result,
        }
    }

    /// Fixed pins can lag the durable release after a control-plane rollback.
    pub(crate) async fn ensure_fixed_defaults(&self) -> Result<bool, WarmPoolError> {
        let released = self.released_images();
        self.ensure_fixed_image(&released.agent, self.default_target, false)
            .await?;
        if let Some((image, target)) = &released.machine {
            self.ensure_fixed_image(image, *target, true).await?;
        }
        Ok(self.default_holds.lane(false) || self.default_holds.lane(true))
    }

    async fn ensure_fixed_image(
        &self,
        image: &RuntimeImage,
        replicas: u32,
        machine: bool,
    ) -> Result<(), WarmPoolError> {
        match self.ensure_image(image, replicas, machine).await {
            Err(WarmPoolError::Conflict) if self.hold_conflicting_pin(image, machine).await? => {
                Ok(())
            }
            result => result,
        }
    }

    pub(crate) async fn hold_conflicting_pin(
        &self,
        image: &RuntimeImage,
        machine: bool,
    ) -> Result<bool, WarmPoolError> {
        let plan = self.default_plan(image)?;
        if !self.store.default_pin_conflicts(&plan, machine).await? {
            return Ok(false);
        }
        self.default_holds.hold(machine, image.uri.clone());
        tracing::warn!(target: "cma_warm_pool_startup", parent: None,
            reason = "durable_default_conflict", machine, config_sha256 = %plan.config_sha256,
            "Agent Manager will serve with the native pool lane held until restart");
        Ok(true)
    }

    pub(crate) fn default_held(&self, machine: bool) -> bool {
        self.default_holds.lane(machine)
    }

    /// Persist the deployment-owned default target before the reconciler starts.
    /// The target is keyed by the exact immutable plan, so a runtime image or
    /// Pod-policy change creates a new pool identity and never mutates a claimed
    /// pool. Provider resources are still created only by the normal reconciler.
    #[cfg(test)]
    pub async fn ensure_default(&self) -> Result<(), WarmPoolError> {
        let released = self.released_images();
        self.ensure_image(&released.agent, self.default_target, false)
            .await?;
        if let Some((image, target)) = &released.machine {
            self.ensure_image(image, *target, true).await?;
        }
        Ok(())
    }

    pub(crate) async fn ensure_released_image(
        &self,
        image: &str,
        machine: bool,
    ) -> Result<(), WarmPoolError> {
        let target = if machine {
            self.machine_target
        } else {
            self.default_target
        };
        self.ensure_image(&deployment_image(image.into()), target, machine)
            .await
    }

    /// A first pointer publishes the prepared target in its own transaction.
    /// Preparation alone must never change the effective release order.
    pub(crate) async fn prepare_released_image(
        &self,
        image: &str,
        machine: bool,
    ) -> Result<PoolTarget, WarmPoolError> {
        let replicas = if machine {
            self.machine_target
        } else {
            self.default_target
        };
        self.prepare_image(&deployment_image(image.into()), replicas, machine)
            .await
    }

    async fn ensure_image(
        &self,
        image: &RuntimeImage,
        replicas: u32,
        machine: bool,
    ) -> Result<(), WarmPoolError> {
        let target = self.prepare_image(image, replicas, machine).await?;
        self.store.activate(&target.id).await
    }

    async fn prepare_image(
        &self,
        image: &RuntimeImage,
        replicas: u32,
        machine: bool,
    ) -> Result<PoolTarget, WarmPoolError> {
        let plan = self.default_plan(image)?;
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
            return Ok(existing.clone());
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
        self.store.insert(target).await
    }

    fn default_plan(&self, image: &RuntimeImage) -> Result<NativePoolConfiguration, WarmPoolError> {
        let provider = self
            .adapter
            .native_pools()
            .ok_or(WarmPoolError::Unavailable)?;
        let home = self.home.clone().ok_or(WarmPoolError::Unavailable)?;
        provider
            .plan(&NativePoolSpec {
                cold_canary: false,
                image: image.clone(),
                cpu_millis: self.default_capacity.cpu_millis,
                memory_mib: self.default_capacity.memory_mib,
                home,
            })
            .map_err(|_| WarmPoolError::Invalid)
    }

    pub(crate) async fn active_default_target(
        &self,
        image: &str,
        machine: bool,
    ) -> Result<PoolTarget, WarmPoolError> {
        let plan = self.default_plan(&deployment_image(image.into()))?;
        self.store
            .list()
            .await?
            .into_iter()
            .find(|target| {
                target.machine == machine
                    && target.plan == plan
                    && !target.retiring
                    && !target.retired
            })
            .ok_or(WarmPoolError::Conflict)
    }

    pub(crate) fn candidate_target(
        &self,
        image: &str,
        active: &PoolTarget,
    ) -> Result<PoolTarget, WarmPoolError> {
        let plan = self.default_plan(&deployment_image(image.into()))?;
        let mut target = default_target(
            plan,
            ConfigureWarmPoolBody {
                cpu_millis: self.default_capacity.cpu_millis,
                memory_mib: self.default_capacity.memory_mib,
                target_replicas: active.target_replicas,
            },
        );
        target.machine = active.machine;
        if target.machine {
            target.document_schema_version = 2;
        }
        Ok(target)
    }

    pub(crate) async fn candidate_inventory(
        &self,
        candidate: &PoolTarget,
    ) -> Result<Option<NativePoolStatus>, WarmPoolError> {
        let provider = self
            .adapter
            .native_pools()
            .ok_or(WarmPoolError::Unavailable)?;
        let inventory = tokio::time::timeout(Duration::from_secs(8), provider.inventory())
            .await
            .map_err(|_| WarmPoolError::Unavailable)?
            .map_err(|_| WarmPoolError::Unavailable)?;
        let mut exact = inventory
            .into_iter()
            .filter(|status| candidate.binding.as_ref() == Some(&status.binding));
        let status = exact.next().filter(|status| {
            status.observed
                && status.desired_replicas == candidate.target_replicas
                && status.ready_replicas >= candidate.target_replicas
        });
        if exact.next().is_some() {
            return Err(WarmPoolError::Conflict);
        }
        Ok(status)
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

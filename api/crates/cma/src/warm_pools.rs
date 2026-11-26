//! Operator-owned native pool targets; Kubernetes observations never replace durable authority.
use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use layer1_public::rest::admin::warm_pools::{
    ConfigureWarmPoolBody, ResizeWarmPoolBody, WarmPoolAdministration, WarmPoolError, WarmPoolList,
    WarmPoolView,
};
use layer2_core::{Caller, Capability};
use layer3_adapter::{
    agent_sandbox::{
        NativePoolBinding, NativePoolConfiguration, NativePoolDrain, NativePoolSpec,
        NativePoolStatus,
    },
    HomeVolumePolicy, RuntimeImage, SandboxAdapter,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

mod candidate;
mod current;
mod defaults;
mod released;
mod retirement;
pub(crate) mod startup;
mod supervision;
pub(crate) mod telemetry;
mod view;

/// Images whose fixed-pin startup failed the durable pool activation fence.
/// Startup records a hold before opening admission or starting any workers.
#[derive(Default)]
pub(crate) struct DefaultPoolHolds(std::sync::RwLock<[Option<String>; 2]>);

impl DefaultPoolHolds {
    fn hold(&self, machine: bool, image: String) {
        self.0.write().expect("default pool holds")[usize::from(machine)] = Some(image);
    }

    pub(crate) fn lane(&self, machine: bool) -> bool {
        self.0.read().expect("default pool holds")[usize::from(machine)].is_some()
    }

    pub(crate) fn image(&self, image: &str) -> bool {
        self.0
            .read()
            .expect("default pool holds")
            .iter()
            .flatten()
            .any(|held| held == image)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct PoolTarget {
    #[serde(rename = "_id")]
    pub id: String,
    #[serde(default)]
    pub activated_at: Option<mongodb::bson::DateTime>,
    /// Separate release order; schema 2 keeps older reconcilers from claiming machine pools.
    #[serde(default)]
    pub machine: bool,
    #[serde(default)]
    pub retiring: bool,
    #[serde(default)]
    pub retired: bool,
    pub document_schema_version: i64,
    pub revision: i64,
    pub actor: String,
    pub requested: ConfigureWarmPoolBody,
    pub plan: NativePoolConfiguration,
    pub binding: Option<NativePoolBinding>,
    pub target_replicas: u32,
    pub applied_revision: i64,
    pub error: Option<String>,
    pub lease: Option<String>,
    pub lease_until: mongodb::bson::DateTime,
    pub reconcile_after: mongodb::bson::DateTime,
    /// The operator's desired machine count, written by a resize of the
    /// current machine plan and inherited by the next machine release.
    /// Retirement zeroes `target_replicas` but keeps this record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operator_replicas: Option<OperatorReplicas>,
}

/// One operator decision about machine capacity; `at` orders decisions
/// across releases, including retired tombstones.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct OperatorReplicas {
    pub replicas: u32,
    pub at: mongodb::bson::DateTime,
}

#[async_trait]
pub(crate) trait PoolTargetStore: Send + Sync {
    async fn candidate_protected(&self, _target: &PoolTarget) -> Result<bool, WarmPoolError> {
        Ok(false)
    }
    async fn begin_candidate_effect(&self, _target: &PoolTarget) -> Result<bool, WarmPoolError> {
        Ok(false)
    }
    async fn candidate_pending(&self, _target: &PoolTarget) -> Result<bool, WarmPoolError> {
        Ok(false)
    }
    async fn confirm_candidate_barrier(&self, _target: &PoolTarget) -> Result<(), WarmPoolError> {
        Err(WarmPoolError::Unavailable)
    }
    async fn activate(&self, _id: &str) -> Result<(), WarmPoolError> {
        Err(WarmPoolError::Unavailable)
    }
    async fn default_pin_conflicts(
        &self,
        _plan: &NativePoolConfiguration,
        _machine: bool,
    ) -> Result<bool, WarmPoolError> {
        Ok(false)
    }
    async fn retirement_homes(
        &self,
        _target: &PoolTarget,
        _image: &str,
    ) -> Result<Option<Vec<layer3_adapter::HomeVolumeAttachment>>, WarmPoolError> {
        Err(WarmPoolError::Unavailable)
    }
    async fn retirement_drain_homes(
        &self,
        _target: &PoolTarget,
        _image: &str,
    ) -> Result<Option<NativePoolDrain>, WarmPoolError> {
        Err(WarmPoolError::Unavailable)
    }
    async fn begin_retirement(&self, _target: &PoolTarget) -> Result<PoolTarget, WarmPoolError> {
        Err(WarmPoolError::Unavailable)
    }
    async fn remove_retired(&self, _target: &PoolTarget) -> Result<(), WarmPoolError> {
        Err(WarmPoolError::Unavailable)
    }
    async fn list(&self) -> Result<Vec<PoolTarget>, WarmPoolError>;
    async fn find(&self, id: &str) -> Result<Option<PoolTarget>, WarmPoolError>;
    async fn insert(&self, target: PoolTarget) -> Result<PoolTarget, WarmPoolError>;
    /// `operator_choice` also records the count as the operator's desired
    /// machine count, in the same fenced write.
    async fn resize(
        &self,
        id: &str,
        body: &ResizeWarmPoolBody,
        operator_choice: bool,
    ) -> Result<PoolTarget, WarmPoolError>;
    /// The newest recorded operator machine decision, retired targets included.
    async fn latest_machine_operator_replicas(
        &self,
    ) -> Result<Option<OperatorReplicas>, WarmPoolError> {
        Err(WarmPoolError::Unavailable)
    }
    async fn claim(&self) -> Result<Option<PoolTarget>, WarmPoolError>;
    async fn claim_target(&self, _id: &str) -> Result<Option<PoolTarget>, WarmPoolError> {
        Err(WarmPoolError::Unavailable)
    }
    /// Lease one target by id for retirement, including one already marked
    /// retiring by an earlier deferred attempt. Never claims a retired target.
    async fn claim_retirement(&self, _id: &str) -> Result<Option<PoolTarget>, WarmPoolError> {
        Err(WarmPoolError::Unavailable)
    }
    async fn bind(
        &self,
        target: &PoolTarget,
        binding: &NativePoolBinding,
    ) -> Result<(), WarmPoolError>;
    async fn finish(&self, target: &PoolTarget, success: bool) -> Result<(), WarmPoolError>;
}

/// Build metadata for the deployment-owned pinned CMA runtime, never a user image.
/// Both cold allocation and administrated warm pools use this same contract.
pub(crate) fn deployment_image(uri: String) -> RuntimeImage {
    layer3_adapter::released_image(uri)
}

const SYSTEM_ACTOR: &str = "cma-system";
/// How long after the newest release older pools of the same kind wait before
/// retiring, so rollout and bounded pre-selection work can settle. Startup
/// readiness waits longer than this, because on a full node pool the new
/// default can only become ready after older spare capacity drains.
pub(crate) const RELEASE_SETTLE: Duration = Duration::from_secs(120);

#[derive(Clone)]
pub(crate) struct NativeWarmPools {
    store: Arc<dyn PoolTargetStore>,
    adapter: Arc<dyn SandboxAdapter>,
    released: Arc<dyn layer3_adapter::ReleasedRuntimeSource>,
    home: Option<HomeVolumePolicy>,
    default_capacity: layer3_adapter::SandboxCapacity,
    default_target: u32,
    machine_target: u32,
    target_fencing: bool,
    allow_empty_startup: bool,
    default_holds: Arc<DefaultPoolHolds>,
}

impl NativeWarmPools {
    pub fn new(
        store: Arc<dyn PoolTargetStore>,
        adapter: Arc<dyn SandboxAdapter>,
        image: String,
        home: Option<HomeVolumePolicy>,
        default_capacity: layer3_adapter::SandboxCapacity,
        default_target: u32,
        machine: Option<(String, u32)>,
    ) -> Arc<Self> {
        let (released, machine_target) = released::pinned(image, machine);
        Arc::new(Self {
            store,
            adapter,
            released,
            home,
            default_capacity,
            default_target,
            machine_target,
            target_fencing: false,
            allow_empty_startup: false,
            default_holds: Arc::default(),
        })
        .with_target_fencing(false)
    }

    /// The integrator binds this to the default-off AM replica writer gate.
    pub(crate) fn with_target_fencing(mut self: Arc<Self>, enabled: bool) -> Arc<Self> {
        Arc::make_mut(&mut self).target_fencing = enabled;
        self
    }

    pub(crate) fn with_default_pool_holds(
        mut self: Arc<Self>,
        holds: Arc<DefaultPoolHolds>,
    ) -> Arc<Self> {
        Arc::make_mut(&mut self).default_holds = holds;
        self
    }

    async fn reconcile_one(&self) -> Result<bool, WarmPoolError> {
        let Some(provider) = self.adapter.native_pools() else {
            return Ok(false);
        };
        let target = if self.default_holds.lane(false) || self.default_holds.lane(true) {
            let mut claimed = None;
            for target in self.store.list().await? {
                if !self.default_holds.lane(target.machine) {
                    claimed = self.store.claim_retirement(&target.id).await?;
                    if claimed.is_some() {
                        break;
                    }
                }
            }
            claimed
        } else {
            self.store.claim().await?
        };
        let Some(target) = target else {
            return Ok(false);
        };
        if target.retiring {
            return self.retire_claimed(provider, target).await;
        }
        if !self.current_plan(&target)? {
            if !self.store.candidate_protected(&target).await? {
                return self.retire_claimed(provider, target).await;
            }
            return self.reconcile_candidate(provider, target).await;
        }
        reconcile_claimed(self.store.as_ref(), provider, target, self.target_fencing)
            .await
            .map(|_| true)
    }
}

pub(super) fn default_config_sha256(
    provider: Option<&dyn layer3_adapter::agent_sandbox::NativePoolControl>,
    image: &RuntimeImage,
    home: Option<&HomeVolumePolicy>,
    capacity: &layer3_adapter::SandboxCapacity,
) -> Option<String> {
    provider?
        .plan(&NativePoolSpec {
            cold_canary: false,
            image: image.clone(),
            cpu_millis: capacity.cpu_millis,
            memory_mib: capacity.memory_mib,
            home: home?.clone(),
        })
        .ok()
        .map(|plan| plan.config_sha256)
}

#[async_trait]
impl WarmPoolAdministration for NativeWarmPools {
    async fn list(&self, caller: &Caller) -> Result<WarmPoolList, WarmPoolError> {
        authorize(caller)?;
        let released = self.released_images();
        let default_config_sha256 = self.default_config_sha256(&released.agent);
        let targets = self.store.list().await?;
        let inventory = match self.adapter.native_pools() {
            Some(provider) if self.home.is_some() => {
                tokio::time::timeout(Duration::from_secs(8), provider.inventory())
                    .await
                    .ok()
                    .and_then(Result::ok)
            }
            _ => None,
        };
        let available = inventory.is_some();
        let live = inventory.unwrap_or_default();
        Ok(WarmPoolList {
            items: targets
                .iter()
                .map(|target| {
                    let status = live
                        .iter()
                        .find(|status| target.binding.as_ref() == Some(&status.binding));
                    target.view(status, available)
                })
                .collect(),
            default_image: released.agent.uri,
            default_cpu_millis: self.default_capacity.cpu_millis,
            default_memory_mib: self.default_capacity.memory_mib,
            default_config_sha256,
            available,
            machine_image: released.machine.map(|(image, _)| image.uri),
        })
    }

    async fn configure(
        &self,
        caller: &Caller,
        key: &str,
        body: ConfigureWarmPoolBody,
    ) -> Result<WarmPoolView, WarmPoolError> {
        authorize(caller)?;
        if key.is_empty()
            || key.len() > 128
            || !key.bytes().all(|byte| byte.is_ascii_graphic())
            || body.target_replicas > 1000
            || !(1..=1_048_576).contains(&body.cpu_millis)
            || !(1..=1_048_576).contains(&body.memory_mib)
        {
            return Err(WarmPoolError::Invalid);
        }
        let id = target_id(&caller.subject.0, key);
        if let Some(existing) = self.store.find(&id).await? {
            return replay(existing, &body).map(|target| target.view(None, true));
        }
        if self.default_holds.lane(false) {
            return Err(WarmPoolError::Conflict);
        }
        let provider = self
            .adapter
            .native_pools()
            .ok_or(WarmPoolError::Unavailable)?;
        let plan = provider
            .plan(&NativePoolSpec {
                cold_canary: false,
                image: self.released_images().agent,
                cpu_millis: body.cpu_millis,
                memory_mib: body.memory_mib,
                home: self.home.clone().ok_or(WarmPoolError::Unavailable)?,
            })
            .map_err(|_| WarmPoolError::Invalid)?;
        let target = PoolTarget {
            id,
            activated_at: None,
            machine: false,
            retiring: false,
            retired: false,
            document_schema_version: 1,
            revision: 1,
            actor: caller.subject.0.clone(),
            target_replicas: body.target_replicas,
            requested: body.clone(),
            plan,
            binding: None,
            applied_revision: 0,
            error: None,
            lease: None,
            lease_until: mongodb::bson::DateTime::from_millis(0),
            reconcile_after: mongodb::bson::DateTime::from_millis(0),
            operator_replicas: None,
        };
        let target = replay(self.store.insert(target).await?, &body)?;
        Ok(target.view(None, true))
    }

    async fn resize(
        &self,
        caller: &Caller,
        id: &str,
        body: ResizeWarmPoolBody,
    ) -> Result<WarmPoolView, WarmPoolError> {
        authorize(caller)?;
        if body.target_replicas > 1000
            || body.expected_revision == 0
            || body.expected_revision >= i64::MAX as u64
        {
            return Err(WarmPoolError::Invalid);
        }
        // Only a resize of the current machine plan is the operator's desired
        // machine count; zeroing superseded capacity merely drains it.
        let target = self.store.find(id).await?.ok_or(WarmPoolError::NotFound)?;
        if self.default_holds.lane(target.machine) {
            return Err(WarmPoolError::Conflict);
        }
        let operator_choice = if body.target_replicas > 0 {
            if !self.current_plan(&target)? {
                return Err(WarmPoolError::Conflict);
            }
            target.machine
        } else {
            target.machine && matches!(self.current_plan(&target), Ok(true))
        };
        let target = self.store.resize(id, &body, operator_choice).await?;
        Ok(target.view(None, true))
    }
}

fn authorize(caller: &Caller) -> Result<(), WarmPoolError> {
    caller
        .require(Capability::ConfigurePlatform)
        .map_err(|_| WarmPoolError::Forbidden)
}

fn default_target(plan: NativePoolConfiguration, requested: ConfigureWarmPoolBody) -> PoolTarget {
    let key = format!("system-default-v1-{}", plan.config_sha256);
    PoolTarget {
        id: target_id(SYSTEM_ACTOR, &key),
        activated_at: None,
        machine: false,
        retiring: false,
        retired: false,
        document_schema_version: 1,
        revision: 1,
        actor: SYSTEM_ACTOR.into(),
        target_replicas: requested.target_replicas,
        requested,
        plan,
        binding: None,
        applied_revision: 0,
        error: None,
        lease: None,
        lease_until: mongodb::bson::DateTime::from_millis(0),
        reconcile_after: mongodb::bson::DateTime::from_millis(0),
        operator_replicas: None,
    }
}

fn target_id(actor: &str, key: &str) -> String {
    let input = serde_json::to_vec(&(actor, key)).expect("two strings serialize");
    format!("wp_{:x}", Sha256::digest(input))
}

fn replay(target: PoolTarget, body: &ConfigureWarmPoolBody) -> Result<PoolTarget, WarmPoolError> {
    if &target.requested == body {
        Ok(target)
    } else {
        Err(WarmPoolError::Conflict)
    }
}

#[cfg(test)]
pub(crate) mod tests;

#[cfg(test)]
async fn reconcile_one(
    store: &dyn PoolTargetStore,
    provider: &dyn layer3_adapter::agent_sandbox::NativePoolControl,
) -> Result<bool, WarmPoolError> {
    let Some(target) = store.claim().await? else {
        return Ok(false);
    };
    reconcile_claimed(store, provider, target, false)
        .await
        .map(|_| true)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PoolApply {
    Settled,
    ConfigurationDrift,
    Failed,
}

fn provider_apply_error(error: layer3_adapter::AdapterError) -> WarmPoolError {
    match error {
        layer3_adapter::AdapterError::NativePoolConfigurationDrift => {
            WarmPoolError::ProviderConfigurationDrift
        }
        _ => WarmPoolError::Unavailable,
    }
}

async fn reconcile_claimed(
    store: &dyn PoolTargetStore,
    provider: &dyn layer3_adapter::agent_sandbox::NativePoolControl,
    mut target: PoolTarget,
    target_fencing: bool,
) -> Result<PoolApply, WarmPoolError> {
    if target.retiring {
        return Err(WarmPoolError::Conflict);
    }
    let result = async {
        if target.binding.is_none() {
            // Zero capacity until the original pool/template UIDs have durable acknowledgement.
            let status = provider
                .configure_planned(&target.plan, 0)
                .await
                .map_err(|_| WarmPoolError::Unavailable)?;
            store.bind(&target, &status.binding).await?;
            target.binding = Some(status.binding);
        }
        let binding = target.binding.as_ref().ok_or(WarmPoolError::Conflict)?;
        if target_fencing {
            provider
                .resize_fenced(
                    binding,
                    &layer3_adapter::agent_sandbox::NativePoolTarget {
                        target_id: target.id.clone(),
                        revision: target.revision,
                        replicas: target.target_replicas,
                    },
                )
                .await
        } else {
            provider.resize(binding, target.target_replicas).await
        }
        .map_err(provider_apply_error)?;
        Ok::<(), WarmPoolError>(())
    };
    let result = tokio::time::timeout(Duration::from_secs(30), result).await;
    let success = matches!(result, Ok(Ok(())));
    store.finish(&target, success).await?;
    telemetry::applied(&target, success);
    Ok(
        if matches!(result, Ok(Err(WarmPoolError::ProviderConfigurationDrift))) {
            PoolApply::ConfigurationDrift
        } else if success {
            PoolApply::Settled
        } else {
            PoolApply::Failed
        },
    )
}

//! Operator-owned native pool targets; Kubernetes observations never replace durable authority.
use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use layer1_public::rest::admin::warm_pools::{
    ConfigureWarmPoolBody, ResizeWarmPoolBody, WarmPoolAdministration, WarmPoolError,
    WarmPoolFailure, WarmPoolList, WarmPoolView,
};
use layer2_core::{Caller, Capability};
use layer3_adapter::{
    agent_sandbox::{NativePoolBinding, NativePoolConfiguration, NativePoolSpec, NativePoolStatus},
    HomeVolumePolicy, RuntimeImage, SandboxAdapter,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

mod apply;
use apply::{reconcile_claimed, PoolApply};
mod candidate;
mod current;
mod defaults;
mod failure;
pub(crate) use failure::PoolFailure;
mod fixed_prewarm;
pub(crate) use fixed_prewarm::observe_fixed_spare;
mod holds;
mod inventory;
pub(crate) use holds::DefaultPoolHolds;
mod released;
mod retirement;
pub(crate) use retirement::RetirementScan;
pub(crate) mod startup;
mod supervision;
pub(crate) mod telemetry;
mod versions;
mod view;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct PoolTarget {
    #[serde(rename = "_id")]
    pub id: String,
    #[serde(default)]
    pub activated_at: Option<mongodb::bson::DateTime>,
    /// Independent order of immutable Desktop plans within a released image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine_plan_activated_at: Option<mongodb::bson::DateTime>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reconciliation_failure: Option<WarmPoolFailure>,
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
    async fn scan_retirement_drain(
        &self,
        _target: &PoolTarget,
        _image: &str,
    ) -> Result<RetirementScan, WarmPoolError> {
        Err(WarmPoolError::Unavailable)
    }
    async fn defer_quarantined_retirement(
        &self,
        _target: &PoolTarget,
    ) -> Result<(), WarmPoolError> {
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
    async fn insert_with_create_fence(
        &self,
        target: PoolTarget,
    ) -> Result<PoolTarget, WarmPoolError> {
        self.insert(target).await
    }
    async fn authorize_provider_create(&self, _target: &PoolTarget) -> Result<(), WarmPoolError> {
        Ok(())
    }
    async fn prepare_fixed_pool(
        &self,
        _active: &PoolTarget,
        _target: Option<PoolTarget>,
        _generation: i64,
    ) -> Result<(), WarmPoolError> {
        Err(WarmPoolError::Unavailable)
    }
    async fn activate_fixed_pools(
        &self,
        _targets: &[PoolTarget],
        _provider: &dyn layer3_adapter::agent_sandbox::NativePoolControl,
        _target_fencing: bool,
        _generation: i64,
    ) -> Result<(), WarmPoolError> {
        Err(WarmPoolError::Unavailable)
    }
    async fn fixed_preparation(&self, _target: &PoolTarget) -> Result<Option<bool>, WarmPoolError> {
        Ok(None)
    }
    async fn begin_fixed_preparation(
        &self,
        _target: &PoolTarget,
        _active: &PoolTarget,
        _generation: i64,
    ) -> Result<bool, WarmPoolError> {
        Ok(false)
    }
    async fn withdraw_fixed_preparation(&self, _target: &PoolTarget) -> Result<(), WarmPoolError> {
        Err(WarmPoolError::Unavailable)
    }
    async fn pool_creates_fenced(&self, _epoch: uuid::Uuid) -> Result<bool, WarmPoolError> {
        Ok(false)
    }
    async fn remove_unbound_retired(
        &self,
        _target: &PoolTarget,
        _epoch: uuid::Uuid,
    ) -> Result<(), WarmPoolError> {
        Err(WarmPoolError::Unavailable)
    }
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
    async fn finish_with_failure(
        &self,
        target: &PoolTarget,
        _failure: PoolFailure,
    ) -> Result<(), WarmPoolError> {
        self.finish(target, false).await
    }
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
    pool_create_cutover_epoch: Option<uuid::Uuid>,
    fixed_pool_prewarm: [Option<String>; 2],
    fixed_prewarm_generation: Option<i64>,
    default_holds: Arc<DefaultPoolHolds>,
    versions: Option<Arc<versions::RuntimeVersions>>,
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
            pool_create_cutover_epoch: None,
            fixed_pool_prewarm: Default::default(),
            fixed_prewarm_generation: None,
            default_holds: Arc::default(),
            versions: None,
        })
        .with_target_fencing(false)
    }

    pub(crate) fn with_runtime_versions(mut self: Arc<Self>) -> Arc<Self> {
        Arc::make_mut(&mut self).versions = versions::RuntimeVersions::new().map(Arc::new);
        self
    }

    /// The integrator binds this to the default-off AM replica writer gate.
    pub(crate) fn with_target_fencing(mut self: Arc<Self>, enabled: bool) -> Arc<Self> {
        Arc::make_mut(&mut self).target_fencing = enabled;
        self
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
        self.inventory().await
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
            machine_plan_activated_at: None,
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
            reconciliation_failure: None,
            lease: None,
            lease_until: mongodb::bson::DateTime::from_millis(0),
            reconcile_after: mongodb::bson::DateTime::from_millis(0),
            operator_replicas: None,
        };
        let target = replay(self.insert_target(target).await?, &body)?;
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
        machine_plan_activated_at: None,
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
        reconciliation_failure: None,
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

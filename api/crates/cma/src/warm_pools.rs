//! Operator-owned native pool targets; Kubernetes observations never replace durable authority.
use std::{collections::BTreeMap, sync::Arc, time::Duration};

use async_trait::async_trait;
use layer1_public::rest::admin::warm_pools::{
    ConfigureWarmPoolBody, ResizeWarmPoolBody, WarmPoolAdministration, WarmPoolError, WarmPoolList,
    WarmPoolView,
};
use layer2_core::{Caller, Capability};
use layer3_adapter::{
    agent_sandbox::{NativePoolBinding, NativePoolConfiguration, NativePoolSpec, NativePoolStatus},
    HomeVolumePolicy, RuntimeImage, SandboxAdapter,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct PoolTarget {
    #[serde(rename = "_id")]
    pub id: String,
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
}

impl PoolTarget {
    pub fn view(&self, live: Option<&NativePoolStatus>, available: bool) -> WarmPoolView {
        let exact = live.filter(|status| self.binding.as_ref() == Some(&status.binding));
        let observed = exact.is_some_and(|status| {
            status.observed && status.desired_replicas == self.target_replicas
        });
        let ready = exact
            .filter(|_| observed)
            .map(|status| status.ready_replicas);
        let status = if !available || self.error.is_some() {
            "unavailable"
        } else if self.applied_revision != self.revision || !observed {
            "pending"
        } else if self.target_replicas == 0 && ready == Some(0) {
            "disabled"
        } else if ready.is_some_and(|count| count >= self.target_replicas) {
            "ready"
        } else {
            "pending"
        };
        WarmPoolView {
            id: self.id.clone(),
            revision: self.revision as u64,
            image: self.plan.spec.image.uri.clone(),
            cpu_millis: self.plan.spec.cpu_millis,
            memory_mib: self.plan.spec.memory_mib,
            target_replicas: self.target_replicas,
            ready_replicas: ready,
            observed,
            status: status.into(),
            resource_name: self.binding.as_ref().map(|binding| binding.name.clone()),
            error: self.error.clone().or_else(|| {
                (!available).then(|| "Native warm pool inventory is unavailable.".into())
            }),
        }
    }
}

#[async_trait]
pub(crate) trait PoolTargetStore: Send + Sync {
    async fn list(&self) -> Result<Vec<PoolTarget>, WarmPoolError>;
    async fn find(&self, id: &str) -> Result<Option<PoolTarget>, WarmPoolError>;
    async fn insert(&self, target: PoolTarget) -> Result<PoolTarget, WarmPoolError>;
    async fn resize(
        &self,
        id: &str,
        body: &ResizeWarmPoolBody,
    ) -> Result<PoolTarget, WarmPoolError>;
    async fn claim(&self) -> Result<Option<PoolTarget>, WarmPoolError>;
    async fn bind(
        &self,
        target: &PoolTarget,
        binding: &NativePoolBinding,
    ) -> Result<(), WarmPoolError>;
    async fn finish(&self, target: &PoolTarget, success: bool) -> Result<(), WarmPoolError>;
}

pub(crate) struct NativeWarmPools {
    store: Arc<dyn PoolTargetStore>,
    adapter: Arc<dyn SandboxAdapter>,
    image: RuntimeImage,
    home: Option<HomeVolumePolicy>,
    default_capacity: layer3_adapter::SandboxCapacity,
}

impl NativeWarmPools {
    pub fn new(
        store: Arc<dyn PoolTargetStore>,
        adapter: Arc<dyn SandboxAdapter>,
        image: String,
        home: Option<HomeVolumePolicy>,
        default_capacity: layer3_adapter::SandboxCapacity,
    ) -> Arc<Self> {
        // This is the deployment-owned CMA runtime build contract, never user-supplied labels.
        let labels = BTreeMap::from([
            (
                layer3_adapter::RUNTIME_CONTRACT_LABEL.into(),
                layer3_adapter::RUNTIME_CONTRACT_VERSION.to_string(),
            ),
            (
                layer3_adapter::runtime_bootstrap::RUNTIME_BOOTSTRAP_LABEL.into(),
                "1".into(),
            ),
        ]);
        Arc::new(Self {
            store,
            adapter,
            image: RuntimeImage { uri: image, labels },
            home,
            default_capacity,
        })
    }

    pub fn spawn(
        self: Arc<Self>,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(5));
            loop {
                tokio::select! {
                    result = shutdown.changed() => {
                        if result.is_err() || *shutdown.borrow() { return; }
                    }
                    _ = tick.tick() => {
                        for _ in 0..20 {
                            match self.reconcile_one().await {
                                Ok(true) => {},
                                Ok(false) => break,
                                Err(_) => { tracing::warn!("native warm pool reconciliation will retry"); break; }
                            }
                        }
                    }
                }
            }
        })
    }

    async fn reconcile_one(&self) -> Result<bool, WarmPoolError> {
        let Some(provider) = self.adapter.native_pools() else {
            return Ok(false);
        };
        reconcile_one(self.store.as_ref(), provider).await
    }
}

#[async_trait]
impl WarmPoolAdministration for NativeWarmPools {
    async fn list(&self, caller: &Caller) -> Result<WarmPoolList, WarmPoolError> {
        authorize(caller)?;
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
            default_image: self.image.uri.clone(),
            default_cpu_millis: self.default_capacity.cpu_millis,
            default_memory_mib: self.default_capacity.memory_mib,
            available,
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
        let provider = self
            .adapter
            .native_pools()
            .ok_or(WarmPoolError::Unavailable)?;
        let plan = provider
            .plan(&NativePoolSpec {
                image: self.image.clone(),
                cpu_millis: body.cpu_millis,
                memory_mib: body.memory_mib,
                home: self.home.clone().ok_or(WarmPoolError::Unavailable)?,
            })
            .map_err(|_| WarmPoolError::Invalid)?;
        let target = PoolTarget {
            id,
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
        let target = self.store.resize(id, &body).await?;
        Ok(target.view(None, true))
    }
}

fn authorize(caller: &Caller) -> Result<(), WarmPoolError> {
    caller
        .require(Capability::ConfigurePlatform)
        .map_err(|_| WarmPoolError::Forbidden)
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
mod tests;

async fn reconcile_one(
    store: &dyn PoolTargetStore,
    provider: &dyn layer3_adapter::agent_sandbox::NativePoolControl,
) -> Result<bool, WarmPoolError> {
    let Some(mut target) = store.claim().await? else {
        return Ok(false);
    };
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
        provider
            .resize(
                target.binding.as_ref().ok_or(WarmPoolError::Conflict)?,
                target.target_replicas,
            )
            .await
            .map_err(|_| WarmPoolError::Unavailable)?;
        Ok::<(), WarmPoolError>(())
    };
    let success = matches!(
        tokio::time::timeout(Duration::from_secs(30), result).await,
        Ok(Ok(()))
    );
    store.finish(&target, success).await?;
    Ok(true)
}

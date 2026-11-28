//! Direct, ownership-confined integration with the infra-owned agent-sandbox API.
//! The controller creates Pods; CMA submits and observes only its own resources.

mod assignment;
mod connection;
pub(crate) mod funding;
mod idle_bootstrap;
mod lifecycle;
mod metric_candidates;
mod metric_observation;
pub use metric_candidates::{MetricCandidate, MetricCandidates};
mod model;
pub use metric_observation::{MetricIdentity, MetricRuntime};
mod capacity;
mod claim_guard;
mod native_assignment;
pub(crate) use capacity::RuntimePermit;
pub use capacity::RuntimePermitKind;
pub use capacity::{
    RuntimeCapacityAuthority, RuntimeCapacityConfig, RuntimeCapacityFailure, RuntimeCapacityReason,
    RuntimeWorkloadClass,
};
pub use claim_guard::NativeClaimGuardReason;
mod readiness_observation;
mod retirement;
pub use readiness_observation::NativeReadinessObservation;
mod receiver_incarnation;
pub(crate) use receiver_incarnation::ReceiverIncarnation;
mod native_runtime_proof;
pub(crate) use native_runtime_proof::NativeRuntimeProof;
pub(crate) use pool::NativeHomeRevision;
mod network_policy;
mod nonexecuting;
mod observation;
mod pods;
mod pool;
mod pool_runtime;
pub(crate) mod provision_timing;
pub(crate) mod resume_timing;
mod runtime_recovery;
mod runtime_stream;
mod startup_termination;
mod suspension;
mod termination;
pub(crate) use startup_termination::{startup_boot_identity, startup_incarnation};
mod upgrade;
pub use crate::{RetainedStartupPolicyPredecessor, RetainedStartupPolicyUpgrade};

use crate::{AdapterError, HomeVolumeScope, Result, RuntimeHandle};
use k8s_openapi::api::core::v1::{PersistentVolumeClaim, Pod};
use kube::api::{DynamicObject, ListParams};
use kube::core::{ApiResource, GroupVersionKind};
use kube::{Api, Client, ResourceExt};
use std::collections::BTreeSet;

pub use connection::{
    resolve_kubernetes_connection, NativeKubernetesConnection, RemoteKubernetesAuth,
    RemoteKubernetesConfig,
};
pub(crate) use model::{handle, parse_handle};
pub use model::{AgentSandboxConfig, PodBinding};
pub use network_policy::NativeTemplateNetworkPolicy;
pub use pool::{
    AgentSandboxPools, NativePoolBinding, NativePoolClaim, NativePoolClaimRequest, NativePoolSpec,
    NativePoolStatus,
};
pub use pool::{NativeFsGroupChangePolicy, NativePoolPodPolicy};
pub use pool::{
    NativeHomeSnapshotObservation, NativeHomeSnapshotRequest, NativeHomeSnapshotSource,
};
pub use pool::{NativeHomeVolumes, NativePoolHomeOrigin};
pub use pool::{NativePoolCleanup, NativePoolMember};
pub use pool::{NativePoolConfiguration, NativePoolControl, NativePoolDrain, NativePoolTarget};
pub(crate) use pool::{NATIVE_HOME_ANNOTATION, NATIVE_HOME_REQUEST_LABEL};
pub use runtime_stream::RuntimeStream;
pub(crate) use runtime_stream::RuntimeStreamKind;
pub(crate) const OWNER_LABEL: &str = "cma/control-plane";
const RUNTIME_LABEL: &str = "cma/runtime-name";
const SPEC_ANNOTATION: &str = "cma/agent-sandbox-spec";

#[derive(Clone)]
pub struct AgentSandboxClient {
    pools: std::sync::Arc<AgentSandboxPools>,
    sandboxes: Api<DynamicObject>,
    pods: Api<Pod>,
    claims: Api<PersistentVolumeClaim>,
    config: AgentSandboxConfig,
    scope: HomeVolumeScope,
    native_home_scope: HomeVolumeScope,
    legacy_home_scope: HomeVolumeScope,
    staging_startup_failure: Option<crate::runtime_bootstrap::StagingStartupFailure>,
    pub(crate) runtime_capacity: std::sync::Arc<RuntimeCapacityAuthority>,
    #[cfg(test)]
    runtime_requests: std::sync::Arc<tokio::sync::Semaphore>,
}

impl AgentSandboxClient {
    pub fn with_staging_startup_failure(
        mut self,
        fault: Option<crate::runtime_bootstrap::StagingStartupFailure>,
    ) -> Self {
        self.staging_startup_failure = fault;
        self
    }

    pub(crate) fn shares_runtime_authority(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.pools, &other.pools)
            && std::sync::Arc::ptr_eq(&self.runtime_capacity, &other.runtime_capacity)
            && self.config == other.config
            && self.native_home_scope == other.native_home_scope
            && self.staging_startup_failure == other.staging_startup_failure
    }

    pub async fn infer(config: AgentSandboxConfig) -> Result<Self> {
        let connection = resolve_kubernetes_connection(None).await?;
        Self::new(connection.client, config, &connection.authority)
    }

    pub fn new(client: Client, config: AgentSandboxConfig, authority: &str) -> Result<Self> {
        Self::new_with_capacity(client, config, authority, RuntimeCapacityConfig::default())
    }

    pub fn new_with_capacity(
        client: Client,
        config: AgentSandboxConfig,
        authority: &str,
        capacity_config: RuntimeCapacityConfig,
    ) -> Result<Self> {
        let capacity =
            RuntimeCapacityAuthority::new(capacity_config).map_err(AdapterError::Rejected)?;
        Self::new_with_authority(client, config, authority, capacity)
    }

    pub fn new_with_authority(
        client: Client,
        config: AgentSandboxConfig,
        authority: &str,
        runtime_capacity: std::sync::Arc<RuntimeCapacityAuthority>,
    ) -> Result<Self> {
        config.validate()?;
        let resource = ApiResource::from_gvk_with_plural(
            &GroupVersionKind::gvk("agents.x-k8s.io", "v1beta1", "Sandbox"),
            "sandboxes",
        );
        let scope = HomeVolumeScope::from_authority(
            "agent-sandbox-v1beta1",
            &[authority, &config.namespace, &config.deployment],
        )?;
        let native_home_scope = HomeVolumeScope::from_authority(
            "native-vct-empty-v1",
            &[authority, &config.namespace, &config.deployment],
        )?;
        Ok(Self {
            pools: std::sync::Arc::new(AgentSandboxPools::new(client.clone(), config.clone())),
            sandboxes: Api::namespaced_with(client.clone(), &config.namespace, &resource),
            pods: Api::namespaced(client.clone(), &config.namespace),
            claims: Api::namespaced(client, &config.namespace),
            legacy_home_scope: HomeVolumeScope::from_authority(
                "kubernetes-pvc-v1",
                &[authority, &config.namespace, &config.deployment],
            )?,
            native_home_scope,
            staging_startup_failure: None,
            scope,
            config,
            #[cfg(test)]
            runtime_requests: runtime_capacity.requests.clone(),
            runtime_capacity,
        })
    }

    pub(crate) fn runtime_capacity(&self) -> std::sync::Arc<RuntimeCapacityAuthority> {
        self.runtime_capacity.clone()
    }

    pub fn warm_pools(&self) -> &AgentSandboxPools {
        &self.pools
    }

    fn selector(&self) -> String {
        format!("{OWNER_LABEL}={}", self.config.deployment)
    }

    fn owned(&self, object: &DynamicObject) -> Result<()> {
        if object.namespace().as_deref() != Some(&self.config.namespace)
            || object.labels().get(OWNER_LABEL) != Some(&self.config.deployment)
            || object.uid().as_deref().is_none_or(str::is_empty)
            || object
                .resource_version()
                .as_deref()
                .is_none_or(str::is_empty)
        {
            return Err(rejected("Sandbox is not owned by this CMA deployment"));
        }
        Ok(())
    }

    pub async fn get(&self, runtime: &RuntimeHandle) -> Result<Option<DynamicObject>> {
        let (name, uid) = parse_handle(runtime)?;
        let Some(object) = self.sandboxes.get_opt(name).await.map_err(api_error)? else {
            return Ok(None);
        };
        self.owned(&object)?;
        if object.uid().as_deref() != Some(uid) {
            return Err(rejected("Sandbox name now belongs to a different UID"));
        }
        Ok(Some(object))
    }

    pub async fn inventory(&self) -> Result<Vec<DynamicObject>> {
        let mut parameters = ListParams::default().labels(&self.selector()).limit(200);
        let mut result = Vec::new();
        let mut cursors = BTreeSet::new();
        loop {
            let page = self.sandboxes.list(&parameters).await.map_err(api_error)?;
            for object in page.items {
                // Also verify ownership locally: a misconfigured/faulty endpoint
                // returning foreign resources cannot make them cleanup candidates.
                if object.labels().get(OWNER_LABEL) != Some(&self.config.deployment) {
                    continue;
                }
                self.owned(&object)?;
                result.push(object);
                if result.len() > 10_000 {
                    return Err(unavailable("CMA inventory exceeds its bound"));
                }
            }
            let Some(cursor) = page.metadata.continue_.filter(|s| !s.is_empty()) else {
                return Ok(result);
            };
            if !cursors.insert(cursor.clone()) {
                return Err(unavailable("Sandbox inventory did not advance"));
            }
            parameters = parameters.continue_token(&cursor);
        }
    }

    pub async fn pod_binding(&self, runtime: &RuntimeHandle) -> Result<PodBinding> {
        let (object, pod) = self.ready_runtime_pod(runtime).await?;
        model::pod_binding(&object, &pod)
    }

    async fn ready_runtime_pod(&self, runtime: &RuntimeHandle) -> Result<(DynamicObject, Pod)> {
        let (name, _) = parse_handle(runtime)?;
        let parameters = ListParams::default().labels(&pods::selector(name)).limit(3);
        // The handle already supplies the selector. Both reads remain fresh;
        // consume Sandbox authority and readiness before any Pod result.
        let (object, page) = tokio::join!(self.get(runtime), self.pods.list(&parameters));
        let object = object?.ok_or_else(|| unavailable("Sandbox no longer exists"))?;
        if object.metadata.deletion_timestamp.is_some() || !model::ready(&object) {
            return Err(unavailable("Sandbox is not ready"));
        }
        let page = if object.name_any() == name {
            page
        } else {
            // Preserve discovery from the returned object's name even for a
            // misaddressed API response; a speculative selector is not proof.
            self.pods
                .list(
                    &ListParams::default()
                        .labels(&pods::selector(&object.name_any()))
                        .limit(3),
                )
                .await
        }
        .map_err(api_error)?;
        let pod = self.owned_pod_from_page(&object, page)?;
        Ok((object, pod))
    }

    fn pod_binding_from_page(
        &self,
        object: &DynamicObject,
        page: kube::api::ObjectList<Pod>,
    ) -> Result<PodBinding> {
        if object.metadata.deletion_timestamp.is_some() || !model::ready(object) {
            return Err(unavailable("Sandbox is not ready"));
        }
        model::pod_binding(object, &self.owned_pod_from_page(object, page)?)
    }

    fn owned_pod_from_page(
        &self,
        object: &DynamicObject,
        page: kube::api::ObjectList<Pod>,
    ) -> Result<Pod> {
        let name = object.name_any();
        let uid = object.uid().ok_or_else(|| rejected("Sandbox has no UID"))?;
        if page.metadata.continue_.is_some_and(|s| !s.is_empty()) {
            return Err(unavailable("Sandbox Pod inventory is incomplete"));
        }
        let mut owned = page
            .items
            .into_iter()
            .filter(|pod| model::owned_pod(pod, &self.config, &name, &uid));
        let pod = owned
            .next()
            .ok_or_else(|| unavailable("Sandbox has no owned Pod"))?;
        if owned.next().is_some() {
            return Err(unavailable("Sandbox has multiple owned Pods"));
        }
        Ok(pod)
    }

    pub(crate) fn control_token(&self, runtime_name: &str) -> String {
        use ring::hmac;
        let key = hmac::Key::new(hmac::HMAC_SHA256, self.config.control_key.as_bytes());
        let message = format!(
            "cma-agent-sandbox-control-v1\0{}\0{}\0{runtime_name}",
            self.config.namespace, self.config.deployment
        );
        hmac::sign(&key, message.as_bytes())
            .as_ref()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }
}

fn rejected(detail: &str) -> AdapterError {
    AdapterError::Rejected(detail.into())
}
fn unavailable(detail: &str) -> AdapterError {
    AdapterError::Unavailable(detail.into())
}
fn api_error(error: kube::Error) -> AdapterError {
    match error {
        kube::Error::Api(response) if response.code == 409 && response.reason == "Conflict" => {
            // Let the durable worker re-read and verify the same UID-bound resources.
            unavailable("Kubernetes Sandbox resource changed; retry after revalidation")
        }
        kube::Error::Api(response) if response.code == 429 => {
            AdapterError::AtCapacity("Kubernetes API throttled the request".into())
        }
        kube::Error::Api(response) if matches!(response.code, 400 | 401 | 403 | 409 | 422) => {
            rejected(&format!(
                "Kubernetes rejected Sandbox operation ({})",
                response.code
            ))
        }
        _ => unavailable("Kubernetes Sandbox operation failed"),
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod http_tests;

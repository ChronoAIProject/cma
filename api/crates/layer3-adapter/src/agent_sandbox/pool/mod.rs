//! Native extension resources; pool members carry no tenant configuration.
mod claim_guard;
#[cfg(test)]
mod claim_guard_tests;
mod claims;
mod cleanup;
mod cleanup_inventory;
mod control;
mod drain;
mod home;
mod home_origin;
mod metric_controller;
mod native_home;
mod native_home_recovery;
mod native_home_snapshots;
mod native_home_store;
mod native_proof;
pub use native_home_snapshots::{
    NativeHomeSnapshotObservation, NativeHomeSnapshotRequest, NativeHomeSnapshotSource,
};
pub use native_home_store::NativeHomeVolumes;
mod binding_resources;
mod management;
mod model;
mod pod_policy;
mod startup_observation;
pub use pod_policy::NativeFsGroupChangePolicy;
mod reconciliation;
mod recovery;
mod retirement;
mod retirement_inventory;
mod runtime;
mod spare_inventory;
mod target_fence;
pub use target_fence::NativePoolTarget;
pub(crate) mod runtime_upgrade;

use kube::{
    api::{DynamicObject, PostParams},
    core::{ApiResource, GroupVersionKind},
    Api, Client, ResourceExt,
};

use super::{api_error, rejected, unavailable, AgentSandboxConfig, OWNER_LABEL};
use crate::{HomeVolumePolicy, Result};

pub use cleanup::{NativePoolCleanup, NativePoolMember};
pub use control::{NativePoolControl, NativePoolDrain};
pub use home_origin::NativePoolHomeOrigin;
pub use management::NativePoolConfiguration;
pub use model::{
    NativePoolBinding, NativePoolClaim, NativePoolClaimRequest, NativePoolPodPolicy,
    NativePoolSpec, NativePoolStatus,
};
pub(crate) const NATIVE_HOME_ANNOTATION: &str = "cma/native-home-origin";
pub(crate) const NATIVE_HOME_REQUEST_LABEL: &str = "cma/home-request";
pub(super) use model::contains as contains_spec;
pub(super) use runtime::RUNTIME_REQUEST_ANNOTATION;

const API_VERSION: &str = "extensions.agents.x-k8s.io/v1beta1";
const DIGEST_ANNOTATION: &str = "cma/native-pool-config";
const DEFINITION_ANNOTATION: &str = "cma/native-pool-definition";

#[derive(Clone)]
pub struct AgentSandboxPools {
    templates: Api<DynamicObject>,
    pools: Api<DynamicObject>,
    claims: Api<DynamicObject>,
    seeds: Api<DynamicObject>,
    native_snapshots: Api<DynamicObject>,
    native_snapshot_contents: Api<DynamicObject>,
    members: Api<DynamicObject>,
    pods: Api<DynamicObject>,
    volumes: Api<k8s_openapi::api::core::v1::PersistentVolumeClaim>,
    config: AgentSandboxConfig,
}

impl AgentSandboxPools {
    pub(super) fn new(client: Client, config: AgentSandboxConfig) -> Self {
        let extensions = |kind, plural| {
            resource(
                client.clone(),
                &config.namespace,
                "extensions.agents.x-k8s.io",
                "v1beta1",
                kind,
                plural,
            )
        };
        Self {
            templates: extensions("SandboxTemplate", "sandboxtemplates"),
            pools: extensions("SandboxWarmPool", "sandboxwarmpools"),
            claims: extensions("SandboxClaim", "sandboxclaims"),
            seeds: resource(
                client.clone(),
                &config.namespace,
                "snapshot.storage.k8s.io",
                "v1",
                "VolumeSnapshot",
                "volumesnapshots",
            ),
            native_snapshots: resource(
                client.clone(),
                &config.namespace,
                "snapshot.storage.k8s.io",
                "v1",
                "VolumeSnapshot",
                "volumesnapshots",
            ),
            native_snapshot_contents: cluster_resource(
                client.clone(),
                "snapshot.storage.k8s.io",
                "v1",
                "VolumeSnapshotContent",
                "volumesnapshotcontents",
            ),
            members: resource(
                client.clone(),
                &config.namespace,
                "agents.x-k8s.io",
                "v1beta1",
                "Sandbox",
                "sandboxes",
            ),
            pods: resource(client.clone(), &config.namespace, "", "v1", "Pod", "pods"),
            volumes: Api::namespaced(client.clone(), &config.namespace),
            config,
        }
    }

    /// Reconcile one immutable pool's replica target. The caller records its target
    /// before I/O and serializes target updates; replicas are not part of pool identity.
    pub async fn configure(
        &self,
        spec: &NativePoolSpec,
        replicas: u32,
    ) -> Result<NativePoolStatus> {
        if replicas > 1000 {
            return Err(rejected("native warm pool target exceeds its bound"));
        }
        management::require_canary_target(spec, replicas)?;
        let desired_template = model::template(&self.config, spec)?;
        // Scaling an existing pool down must remain possible after its seed is
        // retired. A zero-sized pool cannot create a Pod/PVC from that seed.
        if replicas != 0 {
            self.verify_seed(&spec.home).await?;
        }
        let template = ensure(&self.templates, &desired_template).await?;
        let desired = model::pool(&self.config, spec, &template, replicas)?;
        let pool = match self
            .pools
            .get_opt(&desired.name_any())
            .await
            .map_err(api_error)?
        {
            None => create(&self.pools, &desired).await?,
            Some(actual) => self.apply_target(actual, &desired).await?,
        };
        model::verify(&pool, &desired)?;
        management::status(
            management::binding(&self.config, spec, &template, &pool)?,
            &pool,
        )
    }

    async fn verify_seed(&self, policy: &HomeVolumePolicy) -> Result<()> {
        policy.validate()?;
        if policy.is_native_vct() {
            return Ok(());
        }
        let seed = self
            .seeds
            .get(&policy.template_name)
            .await
            .map_err(api_error)?;
        if seed.uid().as_deref() != Some(&policy.template_generation)
            || seed.metadata.deletion_timestamp.is_some()
            || seed
                .annotations()
                .get("cma/home-seed-format")
                .map(String::as_str)
                != Some("1")
            || seed.data["status"]["readyToUse"] != true
        {
            return Err(rejected(
                "native pool home seed is unavailable or changed identity",
            ));
        }
        Ok(())
    }

    pub(crate) async fn verify_explicit_template_policy(
        &self,
        binding: &NativePoolBinding,
    ) -> Result<()> {
        if binding.pod_policy.template_network_policy.is_some()
            || binding.pod_policy.fs_group_change_policy.is_some()
        {
            self.bound_resources(binding).await?;
        }
        Ok(())
    }

    async fn verify_binding(&self, binding: &NativePoolBinding) -> Result<()> {
        let (_, pool) = self.bound_resources(binding).await?;
        retirement::admitting(&pool)?;
        self.verify_seed(&binding.spec.home).await
    }
}

fn resource(
    client: Client,
    namespace: &str,
    group: &str,
    version: &str,
    kind: &str,
    plural: &str,
) -> Api<DynamicObject> {
    Api::namespaced_with(
        client,
        namespace,
        &ApiResource::from_gvk_with_plural(&GroupVersionKind::gvk(group, version, kind), plural),
    )
}

fn cluster_resource(
    client: Client,
    group: &str,
    version: &str,
    kind: &str,
    plural: &str,
) -> Api<DynamicObject> {
    Api::all_with(
        client,
        &ApiResource::from_gvk_with_plural(&GroupVersionKind::gvk(group, version, kind), plural),
    )
}

async fn ensure(api: &Api<DynamicObject>, desired: &DynamicObject) -> Result<DynamicObject> {
    match api.get_opt(&desired.name_any()).await.map_err(api_error)? {
        Some(actual) => {
            model::verify(&actual, desired)?;
            Ok(actual)
        }
        None => create(api, desired).await,
    }
}

async fn create(api: &Api<DynamicObject>, desired: &DynamicObject) -> Result<DynamicObject> {
    let actual = match api.create(&PostParams::default(), desired).await {
        Ok(actual) => actual,
        Err(error) => match api.get_opt(&desired.name_any()).await.map_err(api_error)? {
            Some(actual) => actual,
            None => return Err(api_error(error)),
        },
    };
    model::verify(&actual, desired)?;
    Ok(actual)
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod fixture;

#[cfg(test)]
mod fixture_exec;

#[cfg(test)]
mod home_tests;

#[cfg(test)]
mod recovery_tests;

#[cfg(test)]
mod runtime_tests;

#[cfg(test)]
mod backend_tests;

#[cfg(test)]
mod cleanup_tests;
#[cfg(test)]
mod management_tests;

#[cfg(test)]
mod native_home_tests;

#[cfg(test)]
mod network_policy_tests;

#[cfg(test)]
mod fs_group_policy_tests;

#[cfg(test)]
mod native_admission_read_tests;
#[cfg(test)]
mod native_startup_read_tests;
#[cfg(test)]
mod policy_read_tests;
#[cfg(test)]
mod read_fixture;
#[cfg(test)]
mod selection_read_tests;

#[cfg(test)]
mod drain_tests;

mod claim_inventory;
mod home_inventory;

#[cfg(test)]
mod home_inventory_tests;

#[cfg(test)]
mod cleanup_inventory_tests;

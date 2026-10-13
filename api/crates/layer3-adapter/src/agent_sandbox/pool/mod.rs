//! Native extension resources; pool members carry no tenant configuration.
mod claims;
mod home;
mod home_origin;
mod model;
mod recovery;

use kube::{
    api::{DynamicObject, Patch, PatchParams, PostParams},
    core::{ApiResource, GroupVersionKind},
    Api, Client, ResourceExt,
};
use serde_json::json;

use super::{api_error, rejected, unavailable, AgentSandboxConfig, OWNER_LABEL};
use crate::{HomeVolumePolicy, Result};

pub use home_origin::NativePoolHomeOrigin;
pub use model::{
    NativePoolBinding, NativePoolClaim, NativePoolClaimRequest, NativePoolPodPolicy,
    NativePoolSpec, NativePoolStatus,
};
pub(crate) const NATIVE_HOME_ANNOTATION: &str = "cma/native-home-origin";
pub(crate) const NATIVE_HOME_REQUEST_LABEL: &str = "cma/home-request";

const API_VERSION: &str = "extensions.agents.x-k8s.io/v1beta1";
const DIGEST_ANNOTATION: &str = "cma/native-pool-config";

#[derive(Clone)]
pub struct AgentSandboxPools {
    templates: Api<DynamicObject>,
    pools: Api<DynamicObject>,
    claims: Api<DynamicObject>,
    seeds: Api<DynamicObject>,
    members: Api<DynamicObject>,
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
            members: resource(
                client.clone(),
                &config.namespace,
                "agents.x-k8s.io",
                "v1beta1",
                "Sandbox",
                "sandboxes",
            ),
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
            Some(actual) => {
                let mut prior = desired.clone();
                prior.data["spec"]["replicas"] = actual.data["spec"]["replicas"].clone();
                if prior.data["spec"]["replicas"]
                    .as_u64()
                    .is_none_or(|v| v > 1000)
                {
                    return Err(rejected("native warm pool has an invalid replica target"));
                }
                model::verify(&actual, &prior)?;
                if actual.data["spec"]["replicas"] == replicas {
                    actual
                } else {
                    let patch = json!({"metadata":{"uid":actual.uid(),"resourceVersion":actual.resource_version()},
                        "spec":{"replicas":replicas}});
                    match self
                        .pools
                        .patch(
                            &actual.name_any(),
                            &PatchParams::default(),
                            &Patch::Merge(patch),
                        )
                        .await
                    {
                        Ok(updated) => {
                            if updated.uid() != actual.uid() {
                                return Err(rejected("native pool UID changed during scaling"));
                            }
                            model::verify(&updated, &desired)?;
                            updated
                        }
                        Err(error) => {
                            // A lost PATCH acknowledgement is replayable only against
                            // that exact pool UID and the requested final target.
                            let recovered = self
                                .pools
                                .get_opt(&actual.name_any())
                                .await
                                .map_err(api_error)?;
                            match recovered {
                                Some(recovered)
                                    if recovered.uid() == actual.uid()
                                        && model::verify(&recovered, &desired).is_ok() =>
                                {
                                    recovered
                                }
                                _ => return Err(api_error(error)),
                            }
                        }
                    }
                }
            }
        };
        model::verify(&pool, &desired)?;
        let observed = pool.metadata.generation.is_some_and(|g| {
            g > 0 && pool.data["status"]["observedGeneration"].as_i64() == Some(g)
        });
        let ready = if observed {
            pool.data["status"]["readyReplicas"].as_u64().unwrap_or(0)
        } else {
            0
        };
        Ok(NativePoolStatus {
            binding: NativePoolBinding {
                spec: spec.clone(),
                pod_policy: NativePoolPodPolicy::from(&self.config),
                name: pool.name_any(),
                uid: pool
                    .uid()
                    .ok_or_else(|| rejected("native pool UID missing"))?,
                template_name: template.name_any(),
                template_uid: template
                    .uid()
                    .ok_or_else(|| rejected("native template UID missing"))?,
                config_sha256: model::digest(&self.config, spec)?,
            },
            desired_replicas: replicas,
            ready_replicas: u32::try_from(ready)
                .map_err(|_| unavailable("native pool readiness is invalid"))?,
            observed,
        })
    }

    async fn verify_seed(&self, policy: &HomeVolumePolicy) -> Result<()> {
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

    async fn verify_binding(&self, binding: &NativePoolBinding) -> Result<()> {
        let original_config = model::binding_config(&self.config, binding);
        let desired = model::template(&original_config, &binding.spec)?;
        let template = self
            .templates
            .get(&binding.template_name)
            .await
            .map_err(api_error)?;
        model::verify(&template, &desired)?;
        if template.uid().as_deref() != Some(&binding.template_uid) {
            return Err(rejected("native pool template UID changed"));
        }
        let pool = self.pools.get(&binding.name).await.map_err(api_error)?;
        let replicas = pool.data["spec"]["replicas"]
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| rejected("native pool has no replica target"))?;
        let desired = model::pool(&original_config, &binding.spec, &template, replicas)?;
        model::verify(&pool, &desired)?;
        if pool.uid().as_deref() != Some(&binding.uid) {
            return Err(rejected("native pool UID changed"));
        }
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
mod home_tests;

#[cfg(test)]
mod recovery_tests;

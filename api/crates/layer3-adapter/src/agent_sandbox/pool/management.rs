//! Discover and resize original pools without replacing their admission policy.
use std::collections::BTreeSet;

use kube::{
    api::{DynamicObject, ListParams, Patch, PatchParams},
    ResourceExt,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{
    api_error, model, rejected, unavailable, AgentSandboxConfig, AgentSandboxPools,
    NativePoolBinding, NativePoolPodPolicy, NativePoolSpec, NativePoolStatus, Result,
    DEFINITION_ANNOTATION, OWNER_LABEL,
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Definition {
    spec: NativePoolSpec,
    pod_policy: NativePoolPodPolicy,
}

/// Persist before the initial configure call, including deployment Pod defaults.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativePoolConfiguration {
    pub spec: NativePoolSpec,
    pub pod_policy: NativePoolPodPolicy,
    pub namespace: String,
    pub deployment: String,
    pub config_sha256: String,
}

pub(super) fn definition(config: &AgentSandboxConfig, spec: &NativePoolSpec) -> Result<String> {
    serde_json::to_string(&Definition {
        spec: spec.clone(),
        pod_policy: NativePoolPodPolicy::from(config),
    })
    .map_err(|_| rejected("native pool definition is invalid"))
}

impl AgentSandboxPools {
    pub fn plan(&self, spec: &NativePoolSpec) -> Result<NativePoolConfiguration> {
        Ok(NativePoolConfiguration {
            spec: spec.clone(),
            pod_policy: NativePoolPodPolicy::from(&self.config),
            namespace: self.config.namespace.clone(),
            deployment: self.config.deployment.clone(),
            config_sha256: model::digest(&self.config, spec)?,
        })
    }

    pub async fn configure_planned(
        &self,
        plan: &NativePoolConfiguration,
        replicas: u32,
    ) -> Result<NativePoolStatus> {
        let mut original = self.clone();
        original.config.runtime_class = plan.pod_policy.runtime_class.clone();
        original.config.service_account = plan.pod_policy.service_account.clone();
        original.config.image_pull_secrets = plan.pod_policy.image_pull_secrets.clone();
        original.config.template_network_policy = plan.pod_policy.template_network_policy.clone();
        original.config.template_fs_group_change_policy = plan.pod_policy.fs_group_change_policy;
        if plan.namespace != self.config.namespace
            || plan.deployment != self.config.deployment
            || original.plan(&plan.spec)? != *plan
        {
            return Err(rejected(
                "native pool configuration belongs to another authority",
            ));
        }
        original.configure(&plan.spec, replicas).await
    }

    pub(crate) async fn select_runtime(
        &self,
        request: &crate::provider::ProviderSandboxRequest,
        image: &crate::RuntimeImage,
        home: &crate::HomeVolumeRequest,
    ) -> Result<Option<NativePoolBinding>> {
        home.validate()?;
        let identity = crate::hosted::provider_request_identity(request)?;
        if identity.sandbox_id != home.sandbox_id || !request.volumes.is_empty() {
            return Err(rejected("native pool selection has another home authority"));
        }
        if home.policy.is_native_vct() {
            if request.image.uri != image.uri {
                return Err(rejected(
                    "native cold allocation requires an admitted runtime image",
                ));
            }
            let quantity = |name: &str, suffix: &str| {
                request
                    .resource_limits
                    .get(name)
                    .and_then(|v| v.strip_suffix(suffix))
                    .and_then(|v| v.parse::<u32>().ok())
                    .ok_or_else(|| rejected("native resource quantity is invalid"))
            };
            let spec = NativePoolSpec {
                image: image.clone(),
                cpu_millis: quantity("cpu", "m")?,
                memory_mib: quantity("memory", "Mi")?,
                home: home.policy.clone(),
            };
            let policy = NativePoolPodPolicy::from(&self.config);
            if let Some(pool) = self
                .inventory()
                .await?
                .into_iter()
                .find(|pool| pool.binding.spec == spec && pool.binding.pod_policy == policy)
            {
                return Ok(Some(pool.binding));
            }
            // A zero-sized pool is the controller's cold-creation template. The
            // claim controller falls back to createSandbox when no warm member exists.
            return self
                .ensure_cold_pool(&spec)
                .await
                .map(|pool| Some(pool.binding));
        }
        let policy = NativePoolPodPolicy::from(&self.config);
        let selected = self.inventory().await?.into_iter().find(|pool| {
            pool.binding.spec.image.uri == request.image.uri
                && request.resource_limits.get("cpu")
                    == Some(&format!("{}m", pool.binding.spec.cpu_millis))
                && request.resource_limits.get("memory")
                    == Some(&format!("{}Mi", pool.binding.spec.memory_mib))
                && pool.binding.spec.home == home.policy
                && pool.binding.pod_policy == policy
                && pool.desired_replicas > 0
                && pool.ready_replicas > 0
                && pool.observed
        });
        if let Some(pool) = selected {
            self.verify_seed(&home.policy).await?;
            Ok(Some(pool.binding))
        } else {
            Ok(None)
        }
    }

    /// Create a cold template without ever changing an administrator's replica target.
    async fn ensure_cold_pool(&self, spec: &NativePoolSpec) -> Result<NativePoolStatus> {
        let desired = model::template(&self.config, spec)?;
        let template = super::ensure(&self.templates, &desired).await?;
        let desired = model::pool(&self.config, spec, &template, 0)?;
        let pool = match self
            .pools
            .get_opt(&desired.name_any())
            .await
            .map_err(api_error)?
        {
            Some(actual) => actual,
            None => super::create(&self.pools, &desired).await?,
        };
        let status = self.discover(&pool).await?;
        if status.binding.spec != *spec
            || status.binding.pod_policy != NativePoolPodPolicy::from(&self.config)
            || status.binding.template_uid != template.uid().unwrap_or_default()
        {
            return Err(rejected("native cold pool authority changed"));
        }
        Ok(status)
    }

    /// Return a complete, bounded inventory. Readiness is useful only when the
    /// controller has observed the exact generation; it is not a reservation.
    pub async fn inventory(&self) -> Result<Vec<NativePoolStatus>> {
        let mut params = ListParams::default()
            .labels(&format!("{OWNER_LABEL}={}", self.config.deployment))
            .limit(100);
        let mut cursors = BTreeSet::new();
        let mut names = BTreeSet::new();
        let mut uids = BTreeSet::new();
        let mut version = None;
        let mut result = Vec::new();
        for _ in 0..100 {
            let page = self.pools.list(&params).await.map_err(api_error)?;
            let observed_version = page
                .metadata
                .resource_version
                .filter(|v| !v.is_empty())
                .ok_or_else(|| unavailable("native pool inventory has no resource version"))?;
            if version.as_ref().is_some_and(|v| v != &observed_version) {
                return Err(unavailable("native pool inventory changed between pages"));
            }
            version = Some(observed_version);
            for pool in page.items {
                if !names.insert(pool.name_any()) || !uids.insert(pool.uid()) {
                    return Err(rejected("native pool inventory repeats an identity"));
                }
                result.push(self.discover(&pool).await?);
                if result.len() > 10_000 {
                    return Err(unavailable("native pool inventory exceeds its bound"));
                }
            }
            let Some(cursor) = page.metadata.continue_.filter(|v| !v.is_empty()) else {
                result.sort_by(|a, b| a.binding.name.cmp(&b.binding.name));
                return Ok(result);
            };
            if cursor.len() > 16_384 || !cursors.insert(cursor.clone()) {
                return Err(unavailable("native pool inventory did not advance"));
            }
            params = params.continue_token(&cursor);
        }
        Err(unavailable("native pool inventory exceeds its page bound"))
    }

    /// Select only the exact immutable image/resources/home and current Pod
    /// policy. The caller must persist this binding before creating a claim.
    pub async fn select(&self, spec: &NativePoolSpec) -> Result<Option<NativePoolBinding>> {
        spec.validate()?;
        let policy = NativePoolPodPolicy::from(&self.config);
        let selected = self.inventory().await?.into_iter().find(|status| {
            status.binding.spec == *spec
                && status.binding.pod_policy == policy
                && status.desired_replicas > 0
                && status.ready_replicas > 0
                && status.observed
        });
        if let Some(status) = selected {
            self.verify_seed(&spec.home).await?;
            Ok(Some(status.binding))
        } else {
            Ok(None)
        }
    }

    /// Resize only a recorded original pool/template pair. In particular, a
    /// default-policy change, deletion or UID replacement never creates a pool.
    pub async fn resize(
        &self,
        binding: &NativePoolBinding,
        replicas: u32,
    ) -> Result<NativePoolStatus> {
        if replicas > 1000 {
            return Err(rejected("native warm pool target exceeds its bound"));
        }
        let (template, actual) = self.bound_resources(binding).await?;
        if replicas > 0 {
            self.verify_seed(&binding.spec.home).await?;
        }
        let config = model::binding_config(&self.config, binding);
        let desired = model::pool(&config, &binding.spec, &template, replicas)?;
        let updated = self.apply_target(actual, &desired).await?;
        status(binding.clone(), &updated)
    }

    pub(super) async fn bound_resources(
        &self,
        binding: &NativePoolBinding,
    ) -> Result<(DynamicObject, DynamicObject)> {
        let config = model::binding_config(&self.config, binding);
        let desired = model::template(&config, &binding.spec)?;
        if binding.config_sha256 != model::digest(&config, &binding.spec)?
            || binding.template_name != desired.name_any()
        {
            return Err(rejected("native pool binding has another configuration"));
        }
        let template = self
            .templates
            .get(&binding.template_name)
            .await
            .map_err(api_error)?;
        model::verify(&template, &desired)?;
        if template.uid().as_deref() != Some(&binding.template_uid) {
            return Err(rejected("native pool template UID changed"));
        }
        let actual = self.pools.get(&binding.name).await.map_err(api_error)?;
        let desired = model::pool(&config, &binding.spec, &template, target(&actual)?)?;
        model::verify(&actual, &desired)?;
        if actual.uid().as_deref() != Some(&binding.uid) {
            return Err(rejected("native pool UID changed"));
        }
        Ok((template, actual))
    }

    async fn discover(&self, pool: &DynamicObject) -> Result<NativePoolStatus> {
        let encoded = pool
            .annotations()
            .get(DEFINITION_ANNOTATION)
            .filter(|v| v.len() <= 128 * 1024)
            .ok_or_else(|| rejected("native pool definition is missing or exceeds its bound"))?;
        let definition: Definition = serde_json::from_str(encoded)
            .map_err(|_| rejected("native pool definition is invalid"))?;
        let config = AgentSandboxConfig {
            runtime_class: definition.pod_policy.runtime_class,
            service_account: definition.pod_policy.service_account,
            image_pull_secrets: definition.pod_policy.image_pull_secrets,
            template_network_policy: definition.pod_policy.template_network_policy,
            template_fs_group_change_policy: definition.pod_policy.fs_group_change_policy,
            ..self.config.clone()
        };
        let desired = model::template(&config, &definition.spec)?;
        let template = self
            .templates
            .get(&desired.name_any())
            .await
            .map_err(api_error)?;
        model::verify(&template, &desired)?;
        let desired = model::pool(&config, &definition.spec, &template, target(pool)?)?;
        model::verify(pool, &desired)?;
        status(binding(&config, &definition.spec, &template, pool)?, pool)
    }

    pub(super) async fn apply_target(
        &self,
        actual: DynamicObject,
        desired: &DynamicObject,
    ) -> Result<DynamicObject> {
        let mut prior = desired.clone();
        prior.data["spec"]["replicas"] = json!(target(&actual)?);
        model::verify(&actual, &prior)?;
        if actual.data["spec"]["replicas"] == desired.data["spec"]["replicas"] {
            return Ok(actual);
        }
        let patch = json!({"metadata":{"uid":actual.uid(),"resourceVersion":actual.resource_version()},
            "spec":{"replicas":desired.data["spec"]["replicas"]}});
        let updated = match self
            .pools
            .patch(
                &actual.name_any(),
                &PatchParams::default(),
                &Patch::Merge(patch),
            )
            .await
        {
            Ok(updated) => updated,
            Err(error) => match self
                .pools
                .get_opt(&actual.name_any())
                .await
                .map_err(api_error)?
            {
                Some(recovered)
                    if recovered.uid() == actual.uid()
                        && model::verify(&recovered, desired).is_ok() =>
                {
                    recovered
                }
                _ => return Err(api_error(error)),
            },
        };
        if updated.uid() != actual.uid() {
            return Err(rejected("native pool UID changed during scaling"));
        }
        model::verify(&updated, desired)?;
        Ok(updated)
    }
}

pub(super) fn binding(
    config: &AgentSandboxConfig,
    spec: &NativePoolSpec,
    template: &DynamicObject,
    pool: &DynamicObject,
) -> Result<NativePoolBinding> {
    Ok(NativePoolBinding {
        spec: spec.clone(),
        pod_policy: NativePoolPodPolicy::from(config),
        name: pool.name_any(),
        uid: pool
            .uid()
            .ok_or_else(|| rejected("native pool UID missing"))?,
        template_name: template.name_any(),
        template_uid: template
            .uid()
            .ok_or_else(|| rejected("native template UID missing"))?,
        config_sha256: model::digest(config, spec)?,
    })
}

fn target(pool: &DynamicObject) -> Result<u32> {
    pool.data["spec"]["replicas"]
        .as_u64()
        .filter(|v| *v <= 1000)
        .map(|v| v as u32)
        .ok_or_else(|| rejected("native warm pool has an invalid replica target"))
}

pub(super) fn status(binding: NativePoolBinding, pool: &DynamicObject) -> Result<NativePoolStatus> {
    let observed = pool
        .metadata
        .generation
        .is_some_and(|g| g > 0 && pool.data["status"]["observedGeneration"].as_i64() == Some(g));
    let ready = if observed {
        match pool.data["status"].get("readyReplicas") {
            None => 0,
            Some(value) => value
                .as_u64()
                .filter(|v| *v <= 10_000)
                .ok_or_else(|| unavailable("native pool readiness is invalid"))?,
        }
    } else {
        0
    };
    Ok(NativePoolStatus {
        binding,
        desired_replicas: target(pool)?,
        ready_replicas: ready as u32,
        observed,
    })
}

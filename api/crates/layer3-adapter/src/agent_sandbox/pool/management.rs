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
        self.select_runtime_with_cold_policy(request, image, home, false)
            .await
    }

    pub(crate) async fn select_runtime_with_cold_policy(
        &self,
        request: &crate::provider::ProviderSandboxRequest,
        image: &crate::RuntimeImage,
        home: &crate::HomeVolumeRequest,
        cold_canary: bool,
    ) -> Result<Option<NativePoolBinding>> {
        home.validate()?;
        if cold_canary && !(home.policy.is_native_vct() && home.policy.mounts_full_home()) {
            return Err(rejected(
                "runtime canary pools require native full-home allocation",
            ));
        }
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
                cold_canary,
                image: image.clone(),
                cpu_millis: quantity("cpu", "m")?,
                memory_mib: quantity("memory", "Mi")?,
                home: home.policy.clone(),
            };
            return self
                .exact_native_pool(&spec)
                .await
                .map(|pool| Some(pool.binding));
        }
        let policy = NativePoolPodPolicy::from(&self.config);
        let selected = self.inventory().await?.into_iter().find(|pool| {
            !pool.binding.spec.cold_canary
                && pool.binding.spec.image.uri == request.image.uri
                && request.resource_limits.get("cpu")
                    == Some(&format!("{}m", pool.binding.spec.cpu_millis))
                && request.resource_limits.get("memory")
                    == Some(&format!("{}Mi", pool.binding.spec.memory_mib))
                && pool.binding.spec.home == home.policy
                && pool.binding.pod_policy == policy
                && pool.has_ready_spare()
        });
        if let Some(pool) = selected {
            self.verify_binding(&pool.binding).await?;
            Ok(Some(pool.binding))
        } else {
            Ok(None)
        }
    }

    /// New native reservations know their complete immutable configuration.
    /// Read that exact pair in one wave; historical pools remain discoverable
    /// through inventory and persisted bindings without delaying this selection.
    async fn exact_native_pool(&self, spec: &NativePoolSpec) -> Result<NativePoolStatus> {
        let desired_template = model::template(&self.config, spec)?;
        let template_name = desired_template.name_any();
        let pool_name = model::pool_name(&model::digest(&self.config, spec)?);
        let (template, pool) = tokio::join!(
            self.templates.get_opt(&template_name),
            self.pools.get_opt(&pool_name),
        );
        let template = template.map_err(api_error)?;
        if let Some(template) = &template {
            model::verify(template, &desired_template)?;
        }
        let Some(pool) = pool.map_err(api_error)? else {
            // A zero-sized pool is the controller's cold-creation template.
            // Re-read before creation to preserve concurrent-create recovery.
            return self.ensure_cold_pool(spec).await;
        };
        let template = template.ok_or_else(|| rejected("native pool template is missing"))?;
        let replicas = target(&pool)?;
        require_canary_target(spec, replicas)?;
        let desired_pool = model::pool(&self.config, spec, &template, replicas)?;
        model::verify(&pool, &desired_pool)?;
        super::retirement::admitting(&pool)?;
        status(binding(&self.config, spec, &template, &pool)?, &pool)
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
        super::retirement::admitting(&pool)?;
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
                && status.has_ready_spare()
        });
        if let Some(status) = selected {
            self.verify_binding(&status.binding).await?;
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
        require_canary_target(&binding.spec, replicas)?;
        let resources = self
            .bound_resources_with(binding, |pool| {
                super::target_fence::allow_unversioned(pool, replicas)
            })
            .await;
        if replicas > 0
            && (resources.is_ok()
                || matches!(
                    &resources,
                    Err(crate::AdapterError::NativePoolConfigurationDrift)
                ))
        {
            self.verify_seed(&binding.spec.home).await?;
        }
        let (template, actual) = resources?;
        let config = model::binding_config(&self.config, binding);
        let desired = model::pool(&config, &binding.spec, &template, replicas)?;
        let updated = self.apply_target(actual, &desired).await?;
        status(binding.clone(), &updated)
    }

    pub(super) async fn discover(&self, pool: &DynamicObject) -> Result<NativePoolStatus> {
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
            .get_opt(&desired.name_any())
            .await
            .map_err(api_error)?;
        let Some(template) = template else {
            let uid = pool
                .annotations()
                .get("cma/pool-template-uid")
                .filter(|uid| !uid.is_empty())
                .ok_or_else(|| rejected("native pool template identity is missing"))?;
            let mut expected = desired;
            expected.metadata.uid = Some(uid.clone());
            let desired_pool = model::pool(&config, &definition.spec, &expected, target(pool)?)?;
            model::verify_identity(pool, &desired_pool)?;
            model::verify_spec(pool, &desired_pool)?;
            return Err(crate::AdapterError::NativePoolDefinitionAbsent {
                pool: false,
                template: true,
            });
        };
        model::verify_identity(&template, &desired)?;
        let desired_pool = model::pool(&config, &definition.spec, &template, target(pool)?)?;
        model::verify_identity(pool, &desired_pool)?;
        Self::verify_pool_specs(&template, &desired, pool, &desired_pool)?;
        status(binding(&config, &definition.spec, &template, pool)?, pool)
    }

    pub(super) fn verify_pool_specs(
        template: &DynamicObject,
        desired_template: &DynamicObject,
        pool: &DynamicObject,
        desired_pool: &DynamicObject,
    ) -> Result<()> {
        let specs = model::verify_spec(template, desired_template)
            .and_then(|()| model::verify_spec(pool, desired_pool));
        if specs.is_err() {
            super::retirement::admitting(pool)?;
        }
        specs
    }

    pub(super) async fn apply_target(
        &self,
        actual: DynamicObject,
        desired: &DynamicObject,
    ) -> Result<DynamicObject> {
        super::target_fence::allow_unversioned(&actual, target(desired)?)?;
        let mut prior = desired.clone();
        prior.data["spec"]["replicas"] = json!(target(&actual)?);
        model::verify(&actual, &prior)?;
        if target(desired)? > 0 {
            super::retirement::admitting(&actual)?;
        }
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
        if target(desired)? > 0 {
            super::retirement::admitting(&updated)?;
        }
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

pub(super) fn target(pool: &DynamicObject) -> Result<u32> {
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

pub(super) fn require_canary_target(spec: &NativePoolSpec, replicas: u32) -> Result<()> {
    if spec.cold_canary && replicas != 0 {
        Err(rejected("runtime canary pools have a zero spare target"))
    } else {
        Ok(())
    }
}

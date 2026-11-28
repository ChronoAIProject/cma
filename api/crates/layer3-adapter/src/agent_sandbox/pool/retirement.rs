//! Definition retirement preserves every retained Claim, Sandbox and PVC.
use kube::{
    api::{DeleteParams, DynamicObject, Patch, PatchParams, Preconditions, PropagationPolicy},
    Api, ResourceExt,
};
use serde_json::json;

use super::{
    api_error, management, model, rejected, AgentSandboxPools, NativePoolBinding,
    NativePoolConfiguration, Result,
};
use crate::HomeVolumeAttachment;

pub(super) const RETIRING: &str = "cma/native-pool-retiring";

pub(super) fn admitting(pool: &DynamicObject) -> Result<()> {
    if pool.annotations().contains_key(RETIRING) {
        return Err(rejected(
            "native pool is retiring and refuses new admission",
        ));
    }
    Ok(())
}

impl AgentSandboxPools {
    /// The caller first persists its retirement state under the durable lease.
    /// This marker alone cannot close a cross-resource Claim creation race:
    /// recheck every durable reservation after marking, before resizing to zero.
    pub async fn begin_retirement(&self, binding: &NativePoolBinding) -> Result<()> {
        let (_, pool) = self.retirement_definitions(binding).await?;
        let Some(pool) = pool else { return Ok(()) };
        if pool.annotations().get(RETIRING).map(String::as_str) == Some("true") {
            return Ok(());
        }
        admitting(&pool)?;
        if pool.metadata.deletion_timestamp.is_some() {
            return Err(rejected("unmarked native pool is already deleting"));
        }
        let patch = json!({"metadata":{"uid":pool.uid(),"resourceVersion":pool.resource_version(),
            "annotations":{RETIRING:"true"}}});
        let result = self
            .pools
            .patch(&binding.name, &PatchParams::default(), &Patch::Merge(patch))
            .await;
        let (_, current) = self.retirement_definitions(binding).await?;
        if current
            .as_ref()
            .is_some_and(|p| p.annotations().get(RETIRING).map(String::as_str) == Some("true"))
        {
            return Ok(());
        }
        result.map_err(api_error)?;
        Err(rejected("native pool retirement marker was not retained"))
    }

    /// Deletes only immutable definitions after all surviving capacity has an
    /// exact supplied, upgraded home authority. The caller owns the durable
    /// admission barrier, current-default guard and complete DB allocation check.
    pub async fn retire_pool(
        &self,
        binding: &NativePoolBinding,
        retained_homes: &[HomeVolumeAttachment],
    ) -> Result<bool> {
        let (_, pool) = self.retirement_definitions(binding).await?;
        if let Some(pool) = &pool {
            if !retirement_target(pool)? {
                return Ok(false);
            }
        }
        if !self.retirement_inventory(binding, retained_homes).await? {
            return Ok(false);
        }
        // Refresh definitions and all retained authority immediately before CAS.
        let (_, pool) = self.retirement_definitions(binding).await?;
        self.prove_retirement_homes(binding, retained_homes).await?;
        if let Some(pool) = pool {
            if !retirement_target(&pool)? {
                return Ok(false);
            }
            if !delete_definition(&self.pools, &pool).await? {
                return Ok(false);
            }
        }
        // The second inventory also supports restart after the Pool disappeared.
        if !self.retirement_inventory(binding, retained_homes).await? {
            return Ok(false);
        }
        let (template, pool) = self.retirement_definitions(binding).await?;
        if pool.is_some() {
            return Err(rejected("native pool reappeared during retirement"));
        }
        self.prove_retirement_homes(binding, retained_homes).await?;
        if let Some(template) = template {
            if !delete_definition(&self.templates, &template).await? {
                return Ok(false);
            }
        }
        let (template, pool) = self.retirement_definitions(binding).await?;
        if template.is_some() || pool.is_some() {
            return Ok(false);
        }
        self.retirement_inventory(binding, retained_homes).await
    }

    pub(super) async fn recover_retirement_binding(
        &self,
        plan: &NativePoolConfiguration,
    ) -> Result<Option<NativePoolBinding>> {
        if plan.namespace != self.config.namespace || plan.deployment != self.config.deployment {
            return Err(rejected(
                "native retirement plan belongs to another authority",
            ));
        }
        let mut binding = NativePoolBinding {
            spec: plan.spec.clone(),
            pod_policy: plan.pod_policy.clone(),
            name: model::pool_name(&verified_plan_digest(self, plan)?),
            uid: String::new(),
            template_name: String::new(),
            template_uid: String::new(),
            config_sha256: plan.config_sha256.clone(),
        };
        let config = model::binding_config(&self.config, &binding);
        let desired_template = model::template(&config, &plan.spec)?;
        binding.template_name = desired_template.name_any();
        let (template, pool) = tokio::join!(
            self.templates.get_opt(&binding.template_name),
            self.pools.get_opt(&binding.name),
        );
        match (template.map_err(api_error)?, pool.map_err(api_error)?) {
            (Some(template), Some(pool)) => {
                model::verify(&template, &desired_template)?;
                let desired_pool =
                    model::pool(&config, &plan.spec, &template, management::target(&pool)?)?;
                model::verify(&pool, &desired_pool)?;
                Ok(Some(management::binding(
                    &config, &plan.spec, &template, &pool,
                )?))
            }
            (None, None) => {
                // Blank UIDs are used only in this read-only proof. Any matching
                // original name, plan label, or dependent remains a blocker.
                if !self.retirement_inventory(&binding, &[]).await?
                    || self
                        .templates
                        .get_opt(&binding.template_name)
                        .await
                        .map_err(api_error)?
                        .is_some()
                    || self
                        .pools
                        .get_opt(&binding.name)
                        .await
                        .map_err(api_error)?
                        .is_some()
                {
                    return Err(rejected("native retirement still has provider dependants"));
                }
                Ok(None)
            }
            _ => Err(rejected("native retirement found a partial definition")),
        }
    }

    /// Validate each original definition independently so lost deletion replies
    /// and restarts do not require recreating an already removed definition.
    pub(super) async fn retirement_definitions(
        &self,
        binding: &NativePoolBinding,
    ) -> Result<(Option<DynamicObject>, Option<DynamicObject>)> {
        let config = model::binding_config(&self.config, binding);
        let mut desired_template = model::template(&config, &binding.spec)?;
        if binding.config_sha256 != model::digest(&config, &binding.spec)?
            || binding.template_name != desired_template.name_any()
            || binding.name != model::pool_name(&binding.config_sha256)
            || [&binding.uid, &binding.template_uid]
                .into_iter()
                .any(|s| !super::super::model::dns_label(s))
        {
            return Err(rejected("native retirement binding has another authority"));
        }
        desired_template.metadata.uid = Some(binding.template_uid.clone());
        let (template, pool) = tokio::join!(
            self.templates.get_opt(&binding.template_name),
            self.pools.get_opt(&binding.name)
        );
        let template = template.map_err(api_error)?;
        if let Some(actual) = &template {
            verify_definition(actual, &desired_template, &binding.template_uid)?;
        }
        let pool = pool.map_err(api_error)?;
        if let Some(actual) = &pool {
            let desired = model::pool(
                &config,
                &binding.spec,
                &desired_template,
                management::target(actual)?,
            )?;
            verify_definition(actual, &desired, &binding.uid)?;
            if template.is_none() {
                return Err(rejected("native pool still references a missing template"));
            }
        }
        Ok((template, pool))
    }
}

fn verified_plan_digest(
    pools: &AgentSandboxPools,
    plan: &NativePoolConfiguration,
) -> Result<String> {
    let digest = model::policy_digest(
        &pools.config.namespace,
        &pools.config.deployment,
        &plan.pod_policy,
        &plan.spec,
    )?;
    if digest != plan.config_sha256 {
        return Err(rejected("native retirement plan digest changed"));
    }
    Ok(digest)
}

fn retirement_target(pool: &DynamicObject) -> Result<bool> {
    if pool.annotations().get(RETIRING).map(String::as_str) != Some("true") {
        return Err(rejected(
            "native pool retirement requires its barrier and zero target",
        ));
    }
    Ok(management::target(pool)? == 0)
}

fn verify_definition(actual: &DynamicObject, desired: &DynamicObject, uid: &str) -> Result<()> {
    let mut observed = actual.clone();
    observed.metadata.deletion_timestamp = None;
    model::verify(&observed, desired)?;
    if actual.uid().as_deref() != Some(uid) {
        return Err(rejected("native retirement definition UID changed"));
    }
    Ok(())
}

async fn delete_definition(api: &Api<DynamicObject>, original: &DynamicObject) -> Result<bool> {
    if original.metadata.deletion_timestamp.is_some() {
        return Ok(false);
    }
    let params = DeleteParams {
        propagation_policy: Some(PropagationPolicy::Foreground),
        preconditions: Some(Preconditions {
            uid: original.uid(),
            resource_version: original.resource_version(),
        }),
        ..Default::default()
    };
    let result = api.delete(&original.name_any(), &params).await;
    let current = api.get_opt(&original.name_any()).await.map_err(api_error)?;
    match current {
        None => Ok(true),
        Some(current) if current.uid() != original.uid() => {
            Err(rejected("native retirement definition was replaced"))
        }
        Some(_) => {
            result.map_err(api_error)?;
            Ok(false)
        }
    }
}

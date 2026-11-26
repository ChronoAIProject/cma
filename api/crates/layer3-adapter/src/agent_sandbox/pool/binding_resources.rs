use super::{
    api_error, management::target, model, rejected, retirement, AgentSandboxPools,
    NativePoolBinding, Result,
};
use kube::{api::DynamicObject, ResourceExt};

impl AgentSandboxPools {
    pub(super) async fn bound_resources(
        &self,
        binding: &NativePoolBinding,
    ) -> Result<(DynamicObject, DynamicObject)> {
        self.bound_resources_with(binding, |_| Ok(())).await
    }

    pub(super) async fn bound_resources_with(
        &self,
        binding: &NativePoolBinding,
        authority: impl FnOnce(&DynamicObject) -> Result<()>,
    ) -> Result<(DynamicObject, DynamicObject)> {
        let config = model::binding_config(&self.config, binding);
        let desired = model::template(&config, &binding.spec)?;
        if binding.config_sha256 != model::digest(&config, &binding.spec)?
            || binding.template_name != desired.name_any()
            || binding.name != model::pool_name(&binding.config_sha256)
            || binding.template_uid.is_empty()
            || binding.uid.is_empty()
        {
            return Err(rejected("native pool binding has another configuration"));
        }
        let (template, pool) = tokio::join!(
            self.templates.get_opt(&binding.template_name),
            self.pools.get_opt(&binding.name),
        );
        let template = template.map_err(api_error)?;
        if let Some(template) = &template {
            model::verify_identity(template, &desired)?;
            if template.uid().as_deref() != Some(&binding.template_uid) {
                return Err(rejected("native pool template UID changed"));
            }
        }
        let pool = pool.map_err(api_error)?;
        // The persisted template UID is sufficient to reconstruct the expected
        // pool identity for a read-only comparison when the original is absent.
        // It cannot be used to create a replacement or admit a claim.
        let mut expected_template = desired.clone();
        expected_template.metadata.uid = Some(binding.template_uid.clone());
        let desired_pool = if let Some(pool) = &pool {
            let desired_pool =
                model::pool(&config, &binding.spec, &expected_template, target(pool)?)?;
            model::verify_identity(pool, &desired_pool)?;
            if pool.uid().as_deref() != Some(&binding.uid) {
                return Err(rejected("native pool UID changed"));
            }
            authority(pool)?;
            if template.is_none() {
                retirement::admitting(pool)?;
            }
            Some(desired_pool)
        } else {
            None
        };
        match (template, pool, desired_pool) {
            (Some(template), Some(pool), Some(desired_pool)) => {
                Self::verify_pool_specs(&template, &desired, &pool, &desired_pool)?;
                Ok((template, pool))
            }
            (template, pool, _) => Err(crate::AdapterError::NativePoolDefinitionAbsent {
                pool: pool.is_none(),
                template: template.is_none(),
            }),
        }
    }
}

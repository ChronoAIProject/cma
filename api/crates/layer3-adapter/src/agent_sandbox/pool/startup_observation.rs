use super::{
    api_error, management::status, model, rejected, retirement, AgentSandboxPools,
    NativePoolBinding, NativePoolConfiguration, NativePoolStatus, NativePoolTarget, Result,
};
use kube::ResourceExt;

impl AgentSandboxPools {
    pub(super) async fn observe_planned_pool(
        &self,
        plan: &NativePoolConfiguration,
        target: &NativePoolTarget,
        fenced: bool,
    ) -> Result<Option<NativePoolStatus>> {
        if &self.plan(&plan.spec)? != plan {
            return Err(rejected(
                "native pool plan differs from provider configuration",
            ));
        }
        let desired_template = model::template(&self.config, &plan.spec)?;
        let template_name = desired_template.name_any();
        let pool_name = model::pool_name(&plan.config_sha256);
        let (template, pool) = tokio::join!(
            self.templates.get_opt(&template_name),
            self.pools.get_opt(&pool_name),
        );
        let template = template.map_err(api_error)?;
        let pool = pool.map_err(api_error)?;
        self.verify_seed(&plan.spec.home).await?;
        if let Some(template) = &template {
            model::verify_identity(template, &desired_template)?;
        }
        let Some(pool) = pool else {
            if let Some(template) = &template {
                model::verify_spec(template, &desired_template)?;
            }
            return Ok(None);
        };
        let template = template.ok_or_else(|| rejected("native pool template is missing"))?;
        let desired_pool = model::pool(
            &self.config,
            &plan.spec,
            &template,
            super::management::target(&pool)?,
        )?;
        model::verify_identity(&pool, &desired_pool)?;
        retirement::admitting(&pool)?;
        if fenced {
            super::target_fence::check_target_authority(&pool, target)?;
        } else {
            super::target_fence::allow_unversioned(&pool, target.replicas)?;
        }
        model::verify_spec(&template, &desired_template)?;
        model::verify_spec(&pool, &desired_pool)?;
        let binding = super::management::binding(&self.config, &plan.spec, &template, &pool)?;
        Ok(Some(status(binding, &pool)?))
    }

    pub async fn observe_admitting_pool(
        &self,
        binding: &NativePoolBinding,
        target: &NativePoolTarget,
        fenced: bool,
    ) -> Result<NativePoolStatus> {
        let resources = self
            .bound_resources_with(binding, |pool| {
                if fenced {
                    super::target_fence::check_target_authority(pool, target).map(|_| ())
                } else {
                    super::target_fence::allow_unversioned(pool, target.replicas)
                }
            })
            .await;
        match resources {
            Ok((_, pool)) => {
                retirement::admitting(&pool)?;
                self.verify_seed(&binding.spec.home).await?;
                status(binding.clone(), &pool)
            }
            Err(error @ crate::AdapterError::NativePoolConfigurationDrift) => {
                self.verify_seed(&binding.spec.home).await?;
                Err(error)
            }
            Err(error) => Err(error),
        }
    }
}

use super::{
    api_error, management::status, model, rejected, retirement, AgentSandboxPools,
    NativePoolBinding, NativePoolConfiguration, NativePoolStatus, Result,
};

impl AgentSandboxPools {
    pub(super) async fn observe_planned_pool(
        &self,
        plan: &NativePoolConfiguration,
    ) -> Result<Option<NativePoolStatus>> {
        if &self.plan(&plan.spec)? != plan {
            return Err(rejected(
                "native pool plan differs from provider configuration",
            ));
        }
        let pool = self
            .pools
            .get_opt(&model::pool_name(&plan.config_sha256))
            .await
            .map_err(api_error)?;
        let Some(pool) = pool else { return Ok(None) };
        retirement::admitting(&pool)?;
        self.verify_seed(&plan.spec.home).await?;
        let observed = self.discover(&pool).await?;
        if observed.binding.spec != plan.spec
            || observed.binding.pod_policy != plan.pod_policy
            || observed.binding.config_sha256 != plan.config_sha256
        {
            return Err(rejected("native pool differs from the requested plan"));
        }
        Ok(Some(observed))
    }

    pub async fn observe_admitting_pool(
        &self,
        binding: &NativePoolBinding,
    ) -> Result<NativePoolStatus> {
        let resources = self.bound_resources(binding).await;
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

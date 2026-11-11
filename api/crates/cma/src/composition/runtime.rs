//! Shared deployment runtime authority for pool admission and owner updates.
use std::sync::Arc;

use crate::{config::Config, store::MongoStores, warm_pools::NativeWarmPools};

pub(super) struct RuntimeServices {
    pub warm_pools: Arc<NativeWarmPools>,
    pub updates: Arc<layer2_core::sandbox::SandboxRuntimeUpdateService>,
    pub recovery: Arc<layer2_core::sandbox::RetainedRecoveryService>,
    pub reopen: Arc<layer2_core::sandbox::RetainedReopenService>,
}

impl RuntimeServices {
    pub fn new(
        config: &Config,
        stores: &MongoStores,
        adapter: Arc<dyn layer3_adapter::SandboxAdapter>,
    ) -> Self {
        let inspector = Arc::new(
            crate::sandbox::retained_recovery::RetainedRuntimeInspector::new(
                adapter.clone(),
                config.deployment_id.clone(),
            ),
        );
        Self {
            reopen: Arc::new(layer2_core::sandbox::RetainedReopenService::new(
                stores.sandbox_model.clone(),
                inspector.clone(),
                config.features.retained_reopen_enabled,
            )),
            recovery: Arc::new(layer2_core::sandbox::RetainedRecoveryService::new(
                stores.sandbox_model.clone(),
                inspector,
            )),
            warm_pools: NativeWarmPools::new(
                stores.native_warm_pools.clone(),
                adapter.clone(),
                config.runtime.sandbox_image.clone(),
                config.runtime.agent_sandbox_home_policy.clone(),
                config.agent_manager_sandbox_capacity,
                config.runtime.native_warm_pool_target,
                config
                    .runtime
                    .machine_sandbox_image
                    .clone()
                    .map(|image| (image, config.runtime.native_machine_warm_pool_target)),
            ),
            updates: Arc::new(
                layer2_core::sandbox::SandboxRuntimeUpdateService::new(
                    stores.sandbox_model.clone(),
                    config.runtime.sandbox_image.clone(),
                )
                .with_unpublished_recovery(Arc::new(
                    crate::sandbox::unpublished_recovery::UnpublishedRuntimeRecovery::new(
                        adapter,
                        config.deployment_id.clone(),
                    ),
                )),
            ),
        }
    }
}

pub(super) fn funding(
    config: &Config,
    stores: &MongoStores,
    adapter: Arc<dyn layer3_adapter::SandboxAdapter>,
) -> Result<Arc<dyn crate::sandbox::funding::SandboxFundingAdapter>, String> {
    crate::sandbox::funding::production(
        config.sandbox_billing.clone(),
        stores.sandbox_model.clone(),
        adapter,
        config.deployment_id.clone(),
    )
}

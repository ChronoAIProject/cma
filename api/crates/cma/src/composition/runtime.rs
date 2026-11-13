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

/// Keep deployment lifecycle options together with runtime authority setup.
pub(super) fn lifecycle_config(
    config: &Config,
    holder: String,
) -> crate::sandbox::SandboxLifecycleConfig {
    crate::sandbox::SandboxLifecycleConfig {
        parent_not_ready_pacing_enabled: config.features.parent_not_ready_pacing_enabled,
        retry_single_marker_enabled: config.features.retry_single_marker_enabled,
        bounded_live_fragments: config.features.bounded_live_fragments,
        coordinated_ingress: config.features.coordinated_ingress,
        access_transport_migration: config.agent_access_transport_migration.clone(),
        managed_documents_enabled: config.document_storage.is_some(),
        event_driven_reconciliation: config.features.event_driven_reconciliation,
        sandbox_reconciliation_reserved_lane: config.features.sandbox_reconciliation_reserved_lane,
        home_policy: config
            .runtime
            .home_policy
            .clone()
            .or_else(|| config.runtime.agent_sandbox_home_policy.clone()),
        execution: config.execution,
        holder,
        deployment_id: config.deployment_id.clone(),
        runtime_version: config.runtime.sandbox_image.clone(),
        title_model: config.default_profiles.agent_model.clone(),
    }
}

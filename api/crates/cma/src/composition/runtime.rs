//! Shared deployment runtime authority for pool admission and owner updates.
use std::sync::Arc;

use crate::{
    agent_lifecycle::guidance, config::Config, store::MongoStores, warm_pools::NativeWarmPools,
};

pub(super) fn configure_inline_lifecycle(
    config: &Config,
    stores: &MongoStores,
    lifecycle: Arc<crate::agent_lifecycle::InlineAgentLifecycle>,
    broker: Arc<crate::trigger::CmaServiceConnectionBroker>,
) -> Arc<crate::agent_lifecycle::InlineAgentLifecycle> {
    lifecycle
        .with_released_runtime(stores.released_runtime.clone())
        .with_task_experience(config.task_experience.clone())
        .with_admission_guidance(guidance::configured_admission_instructions(
            &config.features,
        ))
        .with_failure_metadata(config.lifecycle_failure_metadata_enabled)
        .with_repository_preflight_config(config, broker)
        .with_public_repository_images(
            config.public_repository_verified_runtime_images.clone(),
            config.public_repository_snapshot_enabled,
        )
}

pub(super) fn worker_observations(
    config: &Config,
    stores: &MongoStores,
) -> crate::store::worker_observations::Observations {
    stores
        .worker_observations()
        .with_work_retry_policy(config.features.work_retry_policy_enabled)
}

pub(super) struct RuntimeServices {
    pub migrations: Arc<layer2_core::sandbox::SandboxCustomRuntimeMigrationService>,
    pub warm_pools: Arc<NativeWarmPools>,
    pub pointer: Option<Arc<crate::runtime_pointer::RuntimePointerController>>,
    pub updates: Arc<layer2_core::sandbox::SandboxRuntimeUpdateService>,
    pub recovery: Arc<layer2_core::sandbox::RetainedRecoveryService>,
    pub reopen: Arc<layer2_core::sandbox::RetainedReopenService>,
}

impl RuntimeServices {
    pub fn attach_admin(
        &self,
        admin: layer1_public::rest::admin::AdminState,
        stores: &MongoStores,
    ) -> layer1_public::rest::admin::AdminState {
        admin
            .with_runtime_updates(self.updates.clone())
            .with_runtime_migrations(
                self.migrations.clone(),
                stores.sandbox_model.runtime_migration_verifier(),
            )
    }

    pub fn new(
        config: &Config,
        stores: &MongoStores,
        adapter: Arc<dyn layer3_adapter::SandboxAdapter>,
    ) -> Result<Self, String> {
        let inspector = Arc::new(
            crate::sandbox::retained_recovery::RetainedRuntimeInspector::new(
                adapter.clone(),
                config.deployment_id.clone(),
            ),
        );
        let warm_pools = NativeWarmPools::new(
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
        )
        .with_runtime_versions()
        .with_target_fencing(config.agent_manager_replicas.is_some())
        .with_default_pool_holds(stores.sandbox_model.default_pool_holds.clone())
        .with_empty_pool_startup(config.features.am_empty_warm_pool_startup)
        .with_released_runtime(stores.released_runtime.clone());
        let pointer = crate::runtime_pointer::RuntimePointerController::compose(
            config,
            stores,
            warm_pools.clone(),
        )?;
        let mut migrations = layer2_core::sandbox::SandboxCustomRuntimeMigrationService::new(
            stores.sandbox_model.clone(),
            stores.sandbox_model.clone(),
        );
        if let Some(authority) = stores.sandbox_model.runtime_migration_authority() {
            migrations = migrations.with_authority(authority, config.runtime.migration.enabled);
        }
        Ok(Self {
            migrations: Arc::new(migrations),
            reopen: Arc::new(
                layer2_core::sandbox::RetainedReopenService::new(
                    stores.sandbox_model.clone(),
                    inspector.clone(),
                    config.features.retained_reopen_enabled,
                )
                .with_suspended_admission(config.features.retained_suspended_reopen_enabled)
                .with_repeat_admission(config.features.retained_repeat_reopen_enabled),
            ),
            recovery: Arc::new(layer2_core::sandbox::RetainedRecoveryService::new(
                stores.sandbox_model.clone(),
                inspector,
            )),
            warm_pools,
            pointer,
            updates: Arc::new(
                layer2_core::sandbox::SandboxRuntimeUpdateService::with_released(
                    stores.sandbox_model.clone(),
                    stores.released_runtime.clone(),
                )
                .with_unpublished_recovery(Arc::new(
                    crate::sandbox::unpublished_recovery::UnpublishedRuntimeRecovery::new(
                        adapter,
                        config.deployment_id.clone(),
                    ),
                )),
            ),
        })
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
    ownership: Option<Arc<layer2_core::agent_ownership::OwnershipCoordinator>>,
    stores: &MongoStores,
) -> crate::sandbox::SandboxLifecycleConfig {
    crate::sandbox::SandboxLifecycleConfig {
        renewable_work: config.agent_manager_replicas.is_some(),
        workspace_prepare_observations: config.features.workspace_prepare_observations,
        ownership,
        runtime_loss_bounded_proof_enabled: config.features.runtime_loss_bounded_proof_enabled,
        native_retirement_finality_enabled: config.features.native_retirement_finality_enabled,
        native_retirement_capture_enabled: config.features.native_retirement_capture_enabled,
        parent_not_ready_pacing_enabled: config.features.parent_not_ready_pacing_enabled,
        inspection_refusal: config.runtime.inspection_refusal.clone(),
        startup_sibling_hold_enabled: config.features.startup_sibling_hold_enabled,
        retry_single_marker_enabled: config.features.retry_single_marker_enabled,
        work_retry_policy_enabled: config.features.work_retry_policy_enabled,
        native_create_retry_enabled: config.features.native_create_retry_enabled,
        reopen_output_refusal_enabled: config.features.reopen_output_refusal_enabled,
        park_runtime_update_sandbox: config.runtime.staging_sandbox.park_runtime_update.clone(),
        cleanup_continuation_enabled: config.features.cleanup_continuation_enabled,
        owner_stop_settlement_enabled: config.features.owner_stop_settlement_enabled,
        bounded_live_fragments: config.features.bounded_live_fragments,
        coordinated_ingress: config.features.coordinated_ingress,
        quiet_output: config.features.quiet_output,
        incremental_text: config.features.incremental_text,
        access_transport_migration: config.agent_access_transport_migration.clone(),
        managed_documents_enabled: config.document_storage.is_some(),
        event_driven_reconciliation: config.features.event_driven_reconciliation,
        resume_agent_reconciliation: config.features.resume_agent_reconciliation,
        stale_turn_reconciliation: config.features.stale_turn_reconciliation,
        stale_turn_terminal: config.features.stale_turn_terminal,
        skip_ineligible_reconciliation: config
            .agent_manager_replicas
            .as_ref()
            .is_some_and(|replicas| replicas.skip_ineligible_reconciliation),
        defer_foreign_reconciliation: config
            .agent_manager_replicas
            .as_ref()
            .is_some_and(|replicas| replicas.defer_foreign_reconciliation),
        sandbox_reconciliation_reserved_lane: config.features.sandbox_reconciliation_reserved_lane,
        sandbox_binding_hold_metrics: config.features.sandbox_binding_hold_metrics,
        startup_native_home_hold_enabled: config.features.startup_native_home_hold_enabled,
        home_policy: config
            .runtime
            .home_policy
            .clone()
            .or_else(|| config.runtime.agent_sandbox_home_policy.clone()),
        execution: config.execution,
        holder,
        deployment_id: config.deployment_id.clone(),
        released: stores.released_runtime.clone(),
        title_model: config.default_profiles.agent_model.clone(),
    }
}

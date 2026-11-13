//! Durable store connection and deployment-specific writer policy.
use std::sync::Arc;

use crate::{config::Config, store::MongoStores};

pub(super) async fn connect(config: &Config) -> Result<Arc<MongoStores>, String> {
    let mut stores = MongoStores::connect_with_authority(
        &config.mongodb_uri,
        &config.mongodb_database,
        &config.deployment_id,
        &config.auth_encryption_key,
        config.activity_retention,
        (
            &config.runtime.sandbox_image,
            config.runtime.home_policy.as_ref(),
        ),
        config.mongodb_authority,
    )
    .await
    .map_err(|error| format!("durable stores are not usable: {error}"))?;
    stores.sandbox_model = Arc::new(
        (*stores.sandbox_model)
            .clone()
            .with_machine_runtime_version(config.runtime.machine_sandbox_image.clone()),
    );
    stores.sandbox_model = Arc::new(
        (*stores.sandbox_model)
            .clone()
            .with_runtime_loss_bounded_proof(config.features.runtime_loss_bounded_proof_enabled)
            .with_global_sandbox_limit(config.global_sandbox_limit)
            .with_platform_counter_shards(config.features.platform_counter_shards)
            .with_bounded_close_retries(config.features.bounded_close_retries)
            .with_retry_single_marker(config.features.retry_single_marker_enabled)
            .with_scan_cooperation(config.features.agent_log_scan_cooperative_enabled)
            .with_bounded_approval_reads(config.features.bounded_approval_reads)
            .with_fleet_log_percentage(config.agent_log_fleet_percent)
            .with_segment_adoption_enabled(config.features.segment_adoption_enabled)
            .with_bounded_title_reads(config.features.bounded_title_reads)
            .with_access_snapshot_session(config.features.access_snapshot_session)
            .with_sandbox_agent_projection(config.features.sandbox_agent_projection)
            .with_sandbox_snapshot_reads(config.features.sandbox_snapshot_reads)
            .with_sandbox_batched_reads(config.features.sandbox_batched_reads)
            .with_bounded_response_reads(config.features.bounded_response_reads)
            .with_bounded_transcript_reads(config.features.bounded_transcript_reads)
            .with_bounded_usage_reads(config.features.bounded_usage_reads)
            .prepare_bounded_admission(&config.bounded_admission_agents)
            .await?,
    );
    Ok(Arc::new(
        stores.with_writer_policy(config.task_experience.clone()),
    ))
}

/// The operator's platform spend service over the Sandbox store's period row,
/// writable only behind the platform settings' third write gate.
pub(super) fn platform_spend(
    stores: &MongoStores,
    activity: &Arc<layer2_core::ActivityService>,
    settings: &layer2_core::PlatformSettingsService,
) -> Arc<layer2_core::budget::PlatformSpendService> {
    Arc::new(
        layer2_core::budget::PlatformSpendService::new(
            stores.sandbox_model.clone(),
            Arc::clone(activity),
        )
        .with_writes_enabled(settings.platform_spend_writes_enabled()),
    )
}

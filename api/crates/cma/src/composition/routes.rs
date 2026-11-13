//! Apply human authentication to owner routes and capability auth to tool routes.
use std::sync::Arc;

use axum::{extract::DefaultBodyLimit, Router};

use crate::{
    activity::ActivityRecorder, auth::AuthState, connections::BrowserConnections,
    rate_limit::RateLimiters, store::MongoStores, REQUEST_BODY_LIMIT_BYTES,
};

pub(super) fn stream_budget(resources: Router, limit: usize) -> Router {
    resources.layer(axum::middleware::from_fn_with_state(
        crate::public_streams::StreamBudget::new(limit),
        crate::public_streams::enforce,
    ))
}

pub(super) fn compose(
    resources: Router,
    connections: Arc<BrowserConnections>,
    auth: Arc<AuthState>,
    recorder: Arc<ActivityRecorder>,
    (limits, reads): (Arc<RateLimiters>, Arc<crate::expensive_reads::ReadBudget>),
    health: Router,
    tools: Router,
) -> Router {
    let public = crate::rest_router(resources)
        .merge(crate::connections::router(connections))
        .merge(crate::auth::router(auth.clone()))
        .layer(axum::middleware::from_fn_with_state(
            recorder,
            crate::activity::record,
        ))
        .layer(axum::middleware::from_fn_with_state(
            limits.clone(),
            crate::rate_limit::enforce,
        ))
        .layer(axum::middleware::from_fn_with_state(
            reads,
            crate::expensive_reads::enforce,
        ))
        .layer(axum::middleware::from_fn_with_state(
            auth,
            crate::auth::attach_subject,
        ));
    Router::new()
        .merge(public)
        .merge(tools)
        .layer(axum::middleware::from_fn(
            crate::background::interactive_request,
        ))
        .merge(health)
        .merge(crate::telemetry::browser::router())
        .layer(axum::middleware::from_fn_with_state(
            limits,
            crate::rate_limit::admit,
        ))
        .layer(DefaultBodyLimit::max(REQUEST_BODY_LIMIT_BYTES))
}

/// Bind the public catalogue to owner lineage, exact credential custody and
/// the current runtime's explicitly advertised model-selection capability.
pub(super) fn models(
    config: &crate::config::Config,
    reads: Arc<layer2_core::sandbox::AgentReadService>,
    credentials: Arc<dyn crate::sandbox::SandboxCredentialController>,
    adapter: Arc<dyn layer3_adapter::SandboxAdapter>,
) -> Result<Router, String> {
    Ok(layer1_public::rest::agent_models_router(
        reads,
        crate::agent_models::ChronoAgentModels::new(
            &config.runtime.platform_base_url,
            credentials,
        )?,
        Arc::new(crate::projections::AdapterAgentModelSupport(adapter)),
    ))
}

/// Compose explicit file publication and owner uploads to the retained home.
pub(super) fn agent_files(
    store: Arc<dyn layer2_core::sandbox::agent_artifacts::AgentArtifactStore>,
    access: Arc<layer2_core::resource_access::ResourceAccessService>,
    adapter: Arc<dyn layer3_adapter::SandboxAdapter>,
    policy: Arc<layer2_core::sandbox::task_experience::TaskExperiencePolicy>,
) -> Router {
    let filesystem_adapter = adapter.clone();
    layer1_public::rest::agent_attachments_router(
        layer2_core::sandbox::agent_attachments::AgentAttachmentService::new(
            access.clone(),
            adapter,
        ),
    )
    .merge(layer1_public::rest::agent_filesystem_router(
        layer2_core::sandbox::agent_filesystem::AgentFileService::new(
            access.clone(),
            filesystem_adapter,
        )
        .with_task_experience(policy),
    ))
    .merge(layer1_public::rest::agent_artifacts_router(
        layer2_core::sandbox::agent_artifacts::AgentArtifactService::new(store, access),
    ))
}

/// Compose owner-authorized creation, provenance and retained membership reads.
pub(super) fn agent_metadata(
    stores: &MongoStores,
    access: Arc<layer2_core::resource_access::ResourceAccessService>,
) -> Router {
    use layer1_public::rest;
    use layer2_core::sandbox::{
        channel_usage::AgentChannelUsageService,
        provider_associations::AgentProviderAssociationService, AgentProvenanceInventoryService,
        AgentProvenanceService,
    };

    rest::agent_provenance_inventory_router(AgentProvenanceInventoryService::new(
        stores.sandbox_model.clone(),
        access.clone(),
    ))
    .merge(rest::agent_channel_usage_router(
        AgentChannelUsageService::new(stores.sandbox_model.clone(), access.clone()),
    ))
    .merge(rest::sandbox_usage_router(
        layer2_core::sandbox_usage_service::SandboxUsageService::new(stores.sandbox_model.clone()),
    ))
    .merge(rest::agent_provider_associations_router(
        AgentProviderAssociationService::new(stores.sandbox_model.clone(), access.clone()),
    ))
    .merge(rest::agent_capacity_recovery_router(Arc::new(
        layer2_core::sandbox::agent_capacity_recovery::preflight::CapacityRecoveryPreflightService::new(
            access.clone(),
        ),
    )))
    .merge(rest::agent_provenance_router(AgentProvenanceService::new(
        stores.sandbox_model.clone(),
        access,
    )))
}

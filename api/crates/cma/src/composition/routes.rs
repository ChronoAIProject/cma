//! Apply human authentication to owner routes and capability auth to tool routes.
use std::sync::Arc;

use axum::{extract::DefaultBodyLimit, Router};

use crate::{
    activity::ActivityRecorder, auth::AuthState, connections::BrowserConnections,
    rate_limit::RateLimiters, store::MongoStores, REQUEST_BODY_LIMIT_BYTES,
};

pub(super) fn accounts(
    config: &crate::config::Config,
    stores: &MongoStores,
    deletions: Arc<layer2_core::account_deletion::AccountDeletionService>,
    standing: Arc<dyn layer2_core::account_deletion::AccountStanding>,
    reads: Arc<layer2_core::sandbox::AgentReadService>,
) -> Router {
    let routes = layer1_public::rest::account_deletion_router(deletions).merge(
        layer1_public::rest::legal_consent::legal_consent_router(super::legal_consent::service(
            config, stores,
        )),
    );
    if config.features.privacy_export_enabled {
        use layer2_core::sandbox::privacy_export::PrivacyExport;
        routes.merge(layer1_public::rest::privacy_export::router(Arc::new(
            PrivacyExport::new(reads, standing, stores.sandbox_model.clone()),
        )))
    } else {
        routes
    }
}

/// Apply before cloning for delegated dispatch: all entry points share slots.
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
    (limits, config): (Arc<RateLimiters>, &crate::config::Config),
    (health, tools): (Router, Router),
    relay: Option<&Arc<crate::agent_manager::relay::Relay>>,
) -> Router {
    let public = crate::rest_router(resources)
        .merge(crate::connections::router(connections, auth.clone()))
        .merge(crate::auth::router(auth.clone()))
        .merge(crate::auth::service_authorization::router(auth.clone()))
        .layer(axum::middleware::from_fn_with_state(
            recorder,
            crate::activity::record,
        ))
        .layer(axum::middleware::from_fn_with_state(
            limits.clone(),
            crate::rate_limit::enforce,
        ))
        .layer(axum::middleware::from_fn_with_state(
            crate::expensive_reads::ReadBudget::new(config.features.retain_read_frames),
            crate::expensive_reads::enforce,
        ));
    // Public Stop/Delete charge directly after authentication. Runtime-effect
    // routes retain owner routing before their existing operation budgets.
    let public = match relay {
        Some(relay) => relay.authenticated_control_routes(public, auth, limits.lifecycle()),
        None => {
            let public = crate::control_admission::routes(public, limits.lifecycle());
            public.layer(axum::middleware::from_fn_with_state(
                auth,
                crate::auth::attach_subject,
            ))
        }
    };
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
        .layer(axum::middleware::from_fn(creation_outcome_cache_policy))
}

async fn creation_outcome_cache_policy(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let lookup = request.uri().path() == layer1_public::rest::creation_outcomes::LOOKUP_PATH;
    let mut response = next.run(request).await;
    if lookup {
        response.headers_mut().insert(
            axum::http::header::CACHE_CONTROL,
            axum::http::HeaderValue::from_static("private, no-store"),
        );
    }
    response
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
        )?
        .with_cache(config.features.agent_model_discovery_cache),
        crate::agent_models::CachedModelSupport::wrap(
            adapter,
            config.features.agent_model_discovery_cache,
        ),
    ))
}

/// Compose explicit file publication and owner uploads to the retained home.
pub(super) fn agent_files(
    store: Arc<dyn layer2_core::sandbox::agent_artifacts::AgentArtifactStore>,
    access: Arc<layer2_core::resource_access::ResourceAccessService>,
    adapter: Arc<dyn layer3_adapter::SandboxAdapter>,
    config: &crate::config::Config,
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
        .with_task_experience(config.task_experience.clone())
        .with_background_environment(config.features.agent_environment_async)
        .with_read_timings(config.features.agent_file_read_timings),
    ))
    .merge(layer1_public::rest::agent_artifacts_router(
        layer2_core::sandbox::agent_artifacts::AgentArtifactService::new(store, access),
    ))
}

/// Compose owner-authorized creation, provenance and retained membership reads.
pub(super) fn agent_metadata(
    stores: &MongoStores,
    access: Arc<layer2_core::resource_access::ResourceAccessService>,
    config: &crate::config::Config,
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
        super::usage::service(stores.sandbox_model.clone(), config),
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

pub(super) fn resource_budgets(
    resources: Router,
    bindings: &super::bindings::BindingComposition,
    config: &crate::config::Config,
    limits: &RateLimiters,
) -> Result<Router, String> {
    let resources = resources.merge(bindings.router(config.gateway.as_ref())?);
    let resources = stream_budget(resources, config.public_stream_limit);
    Ok(crate::control_admission::routes(
        resources,
        limits.lifecycle(),
    ))
}

pub(super) fn tools(
    tools: Router,
    auth: &Arc<AuthState>,
    config: &crate::config::Config,
) -> Result<Router, String> {
    Ok(tools.merge(crate::auth::service_portal::router(
        auth.clone(),
        config.gateway.as_ref(),
    )?))
}

pub(super) fn runtime_canaries(
    config: &crate::config::Config,
    lifecycle: Arc<crate::agent_lifecycle::InlineAgentLifecycle>,
) -> Result<Router, String> {
    let canaries =
        crate::agent_lifecycle::runtime_canary::Canaries::new(&config.runtime, lifecycle)?;
    Ok(layer1_public::rest::runtime_canary::runtime_canary_router(
        Arc::new(canaries),
    ))
}

pub(super) fn retained_runtime_transitions(
    config: &crate::config::Config,
    stores: &MongoStores,
    adapter: Arc<dyn layer3_adapter::SandboxAdapter>,
) -> Result<Router, String> {
    let service = crate::sandbox::retained_runtime_service::RetainedRuntimeService::new(
        &config.runtime,
        config.deployment_id.clone(),
        stores.sandbox_model.clone(),
        stores.agent_lifecycle.canary_database(),
        adapter,
    )?;
    Ok(layer1_public::rest::retained_runtime_transition::router(
        Arc::new(service),
    ))
}

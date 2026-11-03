//! Apply human authentication to owner routes and capability auth to tool routes.
use std::sync::Arc;

use axum::{extract::DefaultBodyLimit, Router};

use crate::{
    activity::ActivityRecorder, auth::AuthState, connections::BrowserConnections,
    rate_limit::RateLimiters, store::MongoStores, REQUEST_BODY_LIMIT_BYTES,
};

pub(super) fn compose(
    resources: Router,
    connections: Arc<BrowserConnections>,
    auth: Arc<AuthState>,
    recorder: Arc<ActivityRecorder>,
    limits: Arc<RateLimiters>,
    stores: Arc<MongoStores>,
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
            auth,
            crate::auth::attach_subject,
        ));
    Router::new()
        .merge(public)
        .merge(tools)
        .merge(crate::health::router(stores))
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

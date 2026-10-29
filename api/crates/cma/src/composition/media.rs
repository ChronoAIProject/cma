use crate::{
    auth::AuthState, config::Config, media_continuations::MediaContinuations, store::MongoStores,
};
use layer2_core::{
    resource_access::ResourceAccessService,
    sandbox::{
        agent_attachments::AgentAttachmentService,
        agent_io::{AgentCommandService, AgentCommandWorker},
    },
};
pub(super) fn guard(
    service: &Option<Arc<MediaContinuations>>,
) -> Option<Arc<dyn layer2_core::sandbox::agent_io::CommandSourceGuard>> {
    service
        .clone()
        .map(|value| value as Arc<dyn layer2_core::sandbox::agent_io::CommandSourceGuard>)
}
pub(super) fn router(service: Option<Arc<MediaContinuations>>) -> axum::Router {
    service.map_or_else(axum::Router::new, |value| {
        layer1_public::rest::binding_media::binding_media_router(value)
    })
}
use std::sync::Arc;
pub(super) fn compose(
    config: &Config,
    stores: &MongoStores,
    access: Arc<ResourceAccessService>,
    auth: Arc<AuthState>,
    adapter: Arc<dyn layer3_adapter::SandboxAdapter>,
) -> Result<Option<Arc<MediaContinuations>>, String> {
    config
        .gateway
        .as_ref()
        .map(|gateway| {
            MediaContinuations::new(
                AgentCommandService::new(
                    stores.sandbox_model.clone(),
                    access.clone(),
                    auth.clone(),
                ),
                stores.sandbox_model.clone(),
                stores.sandbox_model.clone(),
                access.clone(),
                auth,
                AgentAttachmentService::new(access, adapter),
                &gateway.base_url,
            )
        })
        .transpose()
}
pub(super) fn commands(
    dependencies: crate::commands::ProductionCommandDependencies,
    holder: String,
) -> Result<crate::commands::CommandScheduler, String> {
    let store = dependencies.store.clone();
    let worker = AgentCommandWorker::new(
        store,
        crate::commands::ResourceCommandExecution::production(dependencies),
        holder,
    )
    .map_err(|error| format!("Agent command worker is not usable: {error}"))?;
    Ok(crate::commands::CommandScheduler::new(worker))
}

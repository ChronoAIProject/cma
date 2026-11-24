use std::sync::Arc;

use crate::{
    agent_manager::AgentManager, binding_diagnostics::BindingDiagnostics, config::Config,
    sandbox::SandboxLifecycleComposition, store::MongoStores,
};
use layer1_public::rest::admin::AdminState;

pub(super) fn compose(
    config: &Config,
    manager: &Option<Arc<AgentManager>>,
    stores: &MongoStores,
    lifecycle: &SandboxLifecycleComposition,
    admin: AdminState,
) -> Result<(AdminState, Option<Arc<BindingDiagnostics>>), String> {
    if !config.features.binding_diagnostics {
        return Ok((admin, None));
    }
    let manager = manager
        .as_ref()
        .ok_or("CHRONO_BINDING_DIAGNOSTICS_ENABLED requires Agent Manager replica membership")?;
    let diagnostics = BindingDiagnostics::new(
        manager.clone(),
        stores.sandbox_model.clone(),
        lifecycle.binding_diagnostics(),
        config
            .auth_encryption_key
            .binding_diagnostics_key(&config.deployment_id),
    )?;
    let service =
        layer2_core::binding_diagnostics::BindingDiagnosticsService::new(diagnostics.clone());
    Ok((
        admin.with_binding_diagnostics(Arc::new(service)),
        Some(diagnostics),
    ))
}

pub(super) fn router(
    diagnostics: Option<Arc<BindingDiagnostics>>,
    router: axum::Router,
) -> axum::Router {
    match diagnostics {
        Some(diagnostics) => router.merge(diagnostics.router()),
        None => router,
    }
}

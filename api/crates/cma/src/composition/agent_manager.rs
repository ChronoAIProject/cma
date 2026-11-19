use super::*;

pub(super) async fn register(
    config: &Config,
    stores: &MongoStores,
    live_agents: &Arc<LiveAgentRegistry>,
) -> Result<Option<Arc<crate::agent_manager::AgentManager>>, String> {
    match &config.agent_manager_replicas {
        Some(settings) => Ok(Some(
            crate::agent_manager::AgentManager::register(
                stores.sandbox_model.clone(),
                live_agents.clone(),
                settings.relay_address.clone(),
                settings.max_owned_agents,
                config.features.ownership_cleanup_pacing,
            )
            .await?,
        )),
        None => Ok(None),
    }
}

pub(super) fn relay(
    config: &Config,
    manager: &Option<Arc<crate::agent_manager::AgentManager>>,
    lifecycle: &SandboxLifecycleComposition,
    stores: Arc<crate::store::MongoSandboxStore>,
    controls: layer3_adapter::owner_control::ControlRoutes,
    (auth, reads, access, adapter): (
        Arc<crate::auth::AuthState>,
        Arc<layer2_core::sandbox::AgentReadService>,
        Arc<layer2_core::resource_access::ResourceAccessService>,
        Arc<dyn layer3_adapter::SandboxAdapter>,
    ),
) -> Result<Option<Arc<crate::agent_manager::relay::Relay>>, String> {
    let Some(manager) = manager else {
        return Ok(None);
    };
    let updates = lifecycle.live_updates();
    let relay = crate::agent_manager::relay::Relay::with_capacity(
        manager.coordinator.clone(),
        &updates,
        config
            .auth_encryption_key
            .agent_manager_signing_key(&config.deployment_id),
        config.relay_capacity,
    )?;
    relay.install_public_access(crate::agent_manager::relay::PublicAccess::new(
        auth, reads, access, adapter,
    ))?;
    relay.install_controls(stores.clone(), controls);
    relay.install_reopen(lifecycle.reopen_worker.clone(), stores);
    updates.install_live_source(relay.live_source());
    Ok(Some(relay))
}

pub(super) fn internal_router(
    relay: &Option<Arc<crate::agent_manager::relay::Relay>>,
    manager: &Option<Arc<crate::agent_manager::AgentManager>>,
    router: Router,
) -> Router {
    match (relay, manager) {
        (Some(relay), Some(manager)) => relay.internal_router(router, manager.clone()),
        _ => router,
    }
}

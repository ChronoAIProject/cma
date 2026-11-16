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
    (auth, reads, access, adapter, media, setup): (
        Arc<crate::auth::AuthState>,
        Arc<layer2_core::sandbox::AgentReadService>,
        Arc<layer2_core::resource_access::ResourceAccessService>,
        Arc<dyn layer3_adapter::SandboxAdapter>,
        Option<Arc<crate::media_continuations::MediaContinuations>>,
        Option<Arc<crate::bot_setups::SetupDependencies>>,
    ),
) -> Result<Option<Arc<crate::agent_manager::relay::Relay>>, String> {
    let Some(manager) = manager else {
        return Ok(None);
    };
    let updates = lifecycle.live_updates();
    let relay = crate::agent_manager::relay::Relay::new(
        manager.coordinator.clone(),
        &updates,
        config
            .auth_encryption_key
            .agent_manager_signing_key(&config.deployment_id),
    )?;
    relay.install_public_access(
        crate::agent_manager::relay::PublicAccess::new(auth, reads, access, adapter)
            .with_media(media)
            .with_setup(setup),
    )?;
    relay.install_controls(stores.clone(), controls);
    relay.install_reopen(lifecycle.reopen_worker.clone(), stores);
    updates.install_live_source(relay.live_source());
    Ok(Some(relay))
}

pub(super) fn admission_routers(
    config: &Config,
    manager: &Option<Arc<crate::agent_manager::AgentManager>>,
    auth: &Arc<AuthState>,
) -> [Option<Arc<crate::agent_manager::relay::AdmissionRouter>>; 3] {
    use crate::agent_manager::relay::AdmissionRouter;
    [
        manager
            .as_ref()
            .map(|_| AdmissionRouter::new("public", auth.clone())),
        manager
            .as_ref()
            .map(|_| AdmissionRouter::new("access", auth.clone())),
        manager
            .as_ref()
            .filter(|_| config.gateway.is_some())
            .map(|_| AdmissionRouter::new("media", auth.clone())),
    ]
}

pub(super) fn install_admissions(
    relay: &Option<Arc<crate::agent_manager::relay::Relay>>,
    routers: [Option<Arc<crate::agent_manager::relay::AdmissionRouter>>; 3],
) -> Result<(), String> {
    if let Some(relay) = relay {
        relay.install_admissions(routers.into_iter().flatten().collect())?;
    }
    Ok(())
}

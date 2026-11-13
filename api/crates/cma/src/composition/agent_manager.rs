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
    relay.install_controls(stores.clone(), controls);
    relay.install_reopen(lifecycle.reopen_worker.clone(), stores);
    updates.install_live_source(relay.live_source());
    Ok(Some(relay))
}

use super::*;

pub(super) async fn stores(config: &Config) -> Result<Arc<MongoStores>, String> {
    let mut stores = MongoStores::connect_with_policy(
        &config.mongodb_uri,
        &config.mongodb_database,
        &config.deployment_id,
        &config.auth_encryption_key,
        config.activity_retention,
        (
            &config.runtime.sandbox_image,
            config.runtime.home_policy.as_ref(),
        ),
        crate::store::mongo::MongoConnectionPolicy {
            authority: config.mongodb_authority,
            agent_manager_replicas: config.agent_manager_replicas.is_some(),
        },
    )
    .await
    .map_err(|error| format!("durable stores are not usable: {error}"))?;
    stores.sandbox_model = Arc::new(
        (*stores.sandbox_model)
            .clone()
            .with_machine_runtime_version(config.runtime.machine_sandbox_image.clone()),
    );
    if config.agent_manager_replicas.is_some() {
        stores.sandbox_model = Arc::new(
            (*stores.sandbox_model)
                .clone()
                .enable_agent_ownership(uuid::Uuid::new_v4().to_string())
                .await
                .map_err(|_| "Agent Manager ownership store is unavailable")?,
        );
    }
    Ok(Arc::new(
        stores.with_writer_policy(config.task_experience.clone()),
    ))
}

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
    router: Router,
    stores: Arc<crate::store::MongoSandboxStore>,
    controls: layer3_adapter::owner_control::ControlRoutes,
) -> Result<Router, String> {
    let Some(manager) = manager else {
        return Ok(router);
    };
    let updates = lifecycle.live_updates();
    let relay = crate::agent_manager::relay::Relay::new(
        manager.coordinator.clone(),
        &updates,
        config
            .auth_encryption_key
            .agent_manager_signing_key(&config.deployment_id),
    )?;
    relay.install_controls(stores, controls);
    updates.install_live_source(relay.live_source());
    Ok(relay.router(router, manager.clone()))
}

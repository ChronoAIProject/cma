use crate::config::Config;
use std::sync::Arc;

pub(super) async fn production_adapter(
    config: &Config,
    checkpoint_objects: Arc<crate::store::MongoCheckpointObjectStore>,
    subscriptions: Arc<dyn layer3_adapter::hosted::HostedSubscriptionSource>,
    provider_tools: Arc<dyn layer3_adapter::hosted::HostedProviderToolSource>,
) -> Result<Arc<dyn layer3_adapter::SandboxAdapter>, String> {
    use layer3_adapter::codex::{CodexAdapter, CodexSandboxSpec};
    use layer3_adapter::hosted::{
        CheckpointArchiveStoreFactory, HostedDeployment, ProviderHostedBackend,
        ProviderHostedControlTransport,
    };
    use layer3_adapter::provider::{BatchSandboxReclaimer, SandboxClient};

    let archive_factory: Arc<dyn CheckpointArchiveStoreFactory> = checkpoint_objects;
    let routing_client = mongodb::Client::with_uri_str(&config.mongodb_uri)
        .await
        .map_err(|_| "runtime routing database unavailable")?;
    let runtime_database = routing_client.database(&config.mongodb_database);
    crate::store::isolation::verify(&runtime_database)
        .await
        .map_err(|error| format!("runtime database authority is not isolated: {error}"))?;
    let routing = crate::store::runtime_routing::MongoRuntimeRouting::open(
        &runtime_database,
        &config.deployment_id,
        config.runtime.allocate_agent_sandbox,
    )
    .await?;
    let mut backends = layer3_adapter::hosted::RuntimeBackends {
        routing,
        agent_sandbox: None,
        opensandbox: None,
        allocate_agent_sandbox: config.runtime.allocate_agent_sandbox,
    };
    if !config.runtime.allocate_agent_sandbox || config.runtime.retain_opensandbox {
        let reclaimer =
            BatchSandboxReclaimer::infer_namespaced(config.runtime.sandbox_namespace.clone())
                .await
                .map_err(|e| format!("legacy runtime cleanup authority is unavailable: {e}"))?
                .for_creator(config.deployment_id.clone());
        reclaimer
            .preflight_cleanup_authority()
            .await
            .map_err(|e| format!("legacy runtime cleanup authority is unreadable: {e}"))?;
        let client = SandboxClient::new(
            config.runtime.sandbox_base_url.clone(),
            config.runtime.sandbox_api_key.clone(),
        )
        .with_reclaimer(reclaimer);
        let transport =
            ProviderHostedControlTransport::new(client.clone(), archive_factory.clone())
                .map_err(|e| e.to_string())?
                .with_subscriptions(subscriptions.clone());
        backends.opensandbox = Some(Arc::new(
            ProviderHostedBackend::new(client, Arc::new(transport), &config.deployment_id)
                .map_err(|e| e.to_string())?,
        ));
    }
    if let Some(settings) = &config.runtime.agent_sandbox {
        let client = layer3_adapter::agent_sandbox::AgentSandboxClient::infer(settings.clone())
            .await
            .map_err(|e| e.to_string())?;
        client
            .inventory()
            .await
            .map_err(|e| format!("agent-sandbox API unavailable: {e}"))?;
        let transport =
            ProviderHostedControlTransport::for_agent_sandbox(client.clone(), archive_factory)
                .map_err(|e| e.to_string())?
                .with_subscriptions(subscriptions);
        backends.agent_sandbox = Some(Arc::new(layer3_adapter::hosted::AgentSandboxBackend::new(
            client,
            Arc::new(transport),
        )));
    }
    let mut deployment = HostedDeployment::default();
    deployment.image.uri = config.runtime.sandbox_image.clone();
    deployment.upstream_base_url = config.runtime.upstream_base_url.clone();
    deployment.upstream_wire_api = config.runtime.upstream_wire_api.clone();
    deployment.platform_base_url = config.runtime.platform_base_url.clone();
    deployment
        .contract()
        .map_err(|error| format!("hosted runtime image is not usable: {error}"))?;
    let mut adapter = CodexAdapter::with_backend(
        backends,
        CodexSandboxSpec {
            deployment,
            ..CodexSandboxSpec::default()
        },
    )
    .with_provider_tools(provider_tools);
    if let Some(policy) = &config.runtime.home_policy {
        let homes = layer3_adapter::provider::PersistentHomeVolumes::infer_namespaced(
            &config.runtime.sandbox_namespace,
            &config.deployment_id,
        )
        .await
        .map_err(|error| format!("durable home authority is not usable: {error}"))?;
        homes
            .preflight(policy)
            .await
            .map_err(|error| format!("durable home seed is not usable: {error}"))?;
        adapter = adapter.with_home_volumes(Arc::new(homes));
    }
    Ok(Arc::new(adapter))
}

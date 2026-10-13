use crate::config::Config;
use std::sync::Arc;

pub(super) async fn production_adapter(
    config: &Config,
    subscriptions: Arc<dyn layer3_adapter::hosted::HostedSubscriptionSource>,
    provider_tools: Arc<dyn layer3_adapter::hosted::HostedProviderToolSource>,
    access_tools: Arc<dyn layer3_adapter::hosted::HostedAccessToolSource>,
) -> Result<Arc<dyn layer3_adapter::SandboxAdapter>, String> {
    use layer3_adapter::codex::{CodexAdapter, CodexSandboxSpec};
    use layer3_adapter::hosted::{
        HostedDeployment, ProviderHostedBackend, ProviderHostedControlTransport,
    };
    use layer3_adapter::provider::{BatchSandboxReclaimer, SandboxClient};

    let routing_client = mongodb::Client::with_uri_str(&config.mongodb_uri)
        .await
        .map_err(|_| "runtime routing database unavailable")?;
    let runtime_database = routing_client.database(&config.mongodb_database);
    crate::store::isolation::verify_with_authority(&runtime_database, config.mongodb_authority)
        .await
        .map_err(|error| format!("runtime database authority is not usable: {error}"))?;
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
        home_routes: None,
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
        let transport = ProviderHostedControlTransport::for_retained_opensandbox(client.clone())
            .map_err(|e| e.to_string())?
            .with_subscriptions(subscriptions.clone());
        backends.opensandbox = Some(Arc::new(
            ProviderHostedBackend::new(client, Arc::new(transport), &config.deployment_id)
                .map_err(|e| e.to_string())?,
        ));
    }
    let mut native_homes = None;
    let separate_native = config.runtime.agent_sandbox_connection.is_some()
        || config
            .runtime
            .agent_sandbox
            .as_ref()
            .is_some_and(|settings| settings.namespace != config.runtime.sandbox_namespace)
        || config
            .runtime
            .agent_sandbox_home_policy
            .as_ref()
            .is_some_and(|policy| policy.is_native_vct());
    if let Some(settings) = &config.runtime.agent_sandbox {
        let connection = layer3_adapter::agent_sandbox::resolve_kubernetes_connection(
            config.runtime.agent_sandbox_connection.as_ref(),
        )
        .await
        .map_err(|e| e.to_string())?;
        if separate_native {
            if let Some(policy) = &config.runtime.agent_sandbox_home_policy {
                let homes: Arc<dyn layer3_adapter::HomeVolumeStore> = if policy.is_native_vct() {
                    let homes = layer3_adapter::agent_sandbox::NativeHomeVolumes::new(
                        connection.client.clone(),
                        settings.clone(),
                        &connection.authority,
                    )
                    .map_err(|e| e.to_string())?;
                    homes
                        .preflight(policy)
                        .await
                        .map_err(|e| format!("native VCT authority is not usable: {e}"))?;
                    Arc::new(homes)
                } else {
                    let homes = layer3_adapter::provider::PersistentHomeVolumes::namespaced(
                        connection.client.clone(),
                        &settings.namespace,
                        &config.deployment_id,
                        &connection.authority,
                    )
                    .map_err(|e| e.to_string())?;
                    homes
                        .preflight(policy)
                        .await
                        .map_err(|e| format!("retained native seed is not usable: {e}"))?;
                    Arc::new(homes)
                };
                native_homes = Some((homes, policy.clone()));
            }
        }
        let client = layer3_adapter::agent_sandbox::AgentSandboxClient::new(
            connection.client,
            settings.clone(),
            &connection.authority,
        )
        .map_err(|e| e.to_string())?;
        client
            .inventory()
            .await
            .map_err(|e| format!("agent-sandbox API unavailable: {e}"))?;
        let transport = ProviderHostedControlTransport::for_agent_sandbox(client.clone())
            .map_err(|e| e.to_string())?
            .with_subscriptions(subscriptions);
        backends.agent_sandbox = Some(Arc::new(layer3_adapter::hosted::AgentSandboxBackend::new(
            client,
            Arc::new(transport),
        )));
    }
    let mut home_store: Option<Arc<dyn layer3_adapter::HomeVolumeStore>> = None;
    let mut local_homes = None;
    if let Some(policy) = &config.runtime.home_policy {
        let homes = layer3_adapter::provider::PersistentHomeVolumes::infer_namespaced(
            &config.runtime.sandbox_namespace,
            &config.deployment_id,
        )
        .await
        .map_err(|e| format!("durable home authority is not usable: {e}"))?;
        homes
            .preflight(policy)
            .await
            .map_err(|e| format!("durable home seed is not usable: {e}"))?;
        let homes = Arc::new(homes) as Arc<dyn layer3_adapter::HomeVolumeStore>;
        home_store = Some(homes.clone());
        local_homes = Some((homes, policy.clone()));
    }
    if separate_native && (local_homes.is_some() || native_homes.is_some()) {
        let routes = Arc::new(
            layer3_adapter::RoutedHomeVolumes::new(local_homes, native_homes)
                .map_err(|e| e.to_string())?,
        );
        backends.home_routes = Some(routes.clone());
        home_store = Some(routes);
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
    .with_provider_tools(provider_tools)
    .with_access_tools(access_tools);
    if let Some(homes) = home_store {
        adapter = adapter.with_home_volumes(homes);
    }
    Ok(Arc::new(adapter))
}

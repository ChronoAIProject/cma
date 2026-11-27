use crate::config::Config;
use std::sync::Arc;

pub(super) struct ProductionAdapters {
    pub model_funding: Option<Arc<layer2_core::sandbox::model_credit::AgentTurnFunding>>,
    pub runtime_capacity: Arc<layer3_adapter::agent_sandbox::RuntimeCapacityAuthority>,
    pub owned: Arc<dyn layer3_adapter::SandboxAdapter>,
    pub owner_controls: layer3_adapter::owner_control::ControlRoutes,
    pub preparation: crate::sandbox::native_prepared::NativePreparedWorker,
}

pub(super) fn desktop_creation(
    config: &Config,
    adapter: Arc<dyn layer3_adapter::SandboxAdapter>,
    stores: &crate::store::MongoStores,
) -> Result<Arc<layer2_core::sandbox::desktop_creation::DesktopCreationService>, String> {
    layer2_core::sandbox::desktop_creation::DesktopCreationService::with_released(
        adapter,
        stores.released_runtime.clone(),
        config.desktop_verified_runtime_image.clone(),
    )
    .map_err(str::to_owned)
}

pub(super) async fn production_adapter(
    config: &Config,
    store: Arc<crate::store::MongoSandboxStore>,
    subscriptions: Arc<crate::connections::CodexSubscriptions>,
    access_tools: Arc<dyn layer3_adapter::hosted::HostedAccessToolSource>,
    provider_tools: Arc<crate::store::MongoProviderToolSource>,
    standing: Arc<dyn layer2_core::account_deletion::AccountStanding>,
) -> Result<ProductionAdapters, String> {
    use layer3_adapter::codex::{CodexAdapter, CodexSandboxSpec};
    use layer3_adapter::hosted::{
        HostedDeployment, ProviderHostedBackend, ProviderHostedControlTransport,
    };
    use layer3_adapter::provider::{BatchSandboxReclaimer, SandboxClient};

    let mut owner_controls = Vec::new();
    let subscriptions = crate::connections::RuntimeSubscriptions::new(store.clone(), subscriptions)
        .with_account_standing(standing);

    let mut routing_options = mongodb::options::ClientOptions::parse(&config.mongodb_uri)
        .await
        .map_err(|_| "runtime routing database unavailable")?;
    // Every transaction opens with a barrier read in this database (#3217).
    crate::store::transaction_barrier::bind(&mut routing_options, &config.mongodb_database);
    routing_options.cmap_event_handler = Some(mongodb::event::EventHandler::callback(
        crate::store::mongo_latency::observer(&routing_options, "routing"),
    ));
    let routing_client = mongodb::Client::with_options(routing_options)
        .map_err(|_| "runtime routing database unavailable")?;
    let runtime_database = routing_client.database(&config.mongodb_database);
    crate::store::isolation::verify_with_authority(&runtime_database, config.mongodb_authority)
        .await
        .map_err(|error| format!("runtime database authority is not usable: {error}"))?;
    let routing = crate::store::runtime_routing::MongoRuntimeRouting::open_with_default_pool_holds(
        &runtime_database,
        &config.deployment_id,
        config.runtime.allocate_agent_sandbox,
        store.default_pool_holds.clone(),
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
            .with_subscriptions(subscriptions.clone())
            .with_retained_json(config.features.retained_json)
            .with_coordinated_ingress(config.features.coordinated_ingress)
            .with_bounded_raw_output(config.features.bounded_raw_output)
            .with_bounded_permission_state(config.features.bounded_permission_state)
            .with_bounded_update_relay(config.features.bounded_update_relay)
            .with_output_json_budget(config.features.output_json_budget)
            .with_shared_runtime_output(config.features.shared_runtime_output);
        let transport = register_owner_control(config, "hosted", transport, &mut owner_controls);
        backends.opensandbox = Some(Arc::new(
            ProviderHostedBackend::new(client, transport, &config.deployment_id)
                .map_err(|e| e.to_string())?,
        ));
    }
    // Initialize the native gauges after retained transport construction.
    let runtime_capacity =
        layer3_adapter::agent_sandbox::RuntimeCapacityAuthority::new(config.runtime.capacity)?;
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
        let client = layer3_adapter::agent_sandbox::AgentSandboxClient::new_with_authority(
            connection.client,
            settings.clone(),
            &connection.authority,
            runtime_capacity.clone(),
        )
        .map_err(|e| e.to_string())?
        .with_staging_startup_failure(config.staging_startup_failure.clone());
        client
            .inventory()
            .await
            .map_err(|e| format!("agent-sandbox API unavailable: {e}"))?;
        let transport = ProviderHostedControlTransport::for_agent_sandbox_with_transport(
            client.clone(),
            config.runtime.agent_sandbox_runtime_transport,
        )
        .map_err(|e| e.to_string())?
        .with_subscriptions(subscriptions)
        .with_retained_json(config.features.retained_json)
        .with_coordinated_ingress(config.features.coordinated_ingress)
        .with_bounded_raw_output(config.features.bounded_raw_output)
        .with_bounded_permission_state(config.features.bounded_permission_state)
        .with_bounded_update_relay(config.features.bounded_update_relay)
        .with_output_json_budget(config.features.output_json_budget)
        .with_shared_runtime_output(config.features.shared_runtime_output)
        .with_native_close_agent_lane(config.features.native_close_agent_lane)
        .with_legacy_receiver_image(config.runtime.legacy_receiver_image.clone());
        let transport = match &config.runtime.stale_turn_runtime_digest {
            Some(digest)
                if config.features.stale_turn_reconciliation
                    && config.features.stale_turn_terminal =>
            {
                transport
                    .with_qualified_stale_turn_image(digest.clone())
                    .map_err(|error| error.to_string())?
            }
            _ => transport,
        };
        let transport = match &config.runtime.open_reply_loss_sandbox {
            Some(sandbox_id) => transport.with_open_reply_loss(
                sandbox_id.clone(),
                Arc::new(store.open_reply_loss_claim(sandbox_id.clone())),
            ),
            None => transport,
        };
        let transport = register_owner_control(config, "native", transport, &mut owner_controls);
        backends.agent_sandbox = Some(Arc::new(layer3_adapter::hosted::AgentSandboxBackend::new(
            client, transport,
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
    let deployment = HostedDeployment {
        image: crate::warm_pools::deployment_image(config.runtime.sandbox_image.clone()),
        upstream_base_url: config.runtime.upstream_base_url.clone(),
        upstream_wire_api: config.runtime.upstream_wire_api.clone(),
        platform_base_url: config.runtime.platform_base_url.clone(),
        ..HostedDeployment::default()
    };
    deployment
        .contract()
        .map_err(|error| format!("hosted runtime image is not usable: {error}"))?;
    let backend: Arc<dyn layer3_adapter::hosted::HostedBackend> = Arc::new(backends);
    let spec = CodexSandboxSpec {
        deployment,
        ..CodexSandboxSpec::default()
    };
    // Only the bounded native preparation worker receives this adapter. It
    // has no owner tool sources; that worker receives no credential controller.
    let mut preparation = CodexAdapter::new(backend.clone(), spec.clone());
    let mut adapter = CodexAdapter::new(backend, spec)
        .with_provider_tools(provider_tools)
        .with_access_tools(access_tools);
    if let Some(image) = &config.runtime.machine_sandbox_image {
        preparation = preparation
            .with_native_runtime_image(crate::warm_pools::deployment_image(image.clone()))
            .map_err(|error| format!("prepared machine runtime image is not usable: {error}"))?;
        adapter = adapter
            .with_native_runtime_image(crate::warm_pools::deployment_image(image.clone()))
            .map_err(|error| format!("machine runtime image is not usable: {error}"))?;
    }
    if let Some(homes) = home_store {
        preparation = preparation.with_home_volumes(homes.clone());
        adapter = adapter.with_home_volumes(homes);
    }
    Ok(ProductionAdapters {
        // None until the model gateway can attest to the paying credential and enforce phase limits.
        model_funding: None,
        runtime_capacity,
        owner_controls,
        owned: Arc::new(adapter),
        preparation: crate::sandbox::native_prepared::NativePreparedWorker::new(
            store,
            Arc::new(preparation),
            config,
        )?,
    })
}

fn register_owner_control(
    config: &Config,
    name: &str,
    transport: layer3_adapter::hosted::ProviderHostedControlTransport,
    routes: &mut layer3_adapter::owner_control::ControlRoutes,
) -> Arc<dyn layer3_adapter::hosted::HostedControlTransport> {
    if config.agent_manager_replicas.is_none() {
        return Arc::new(transport);
    }
    let route = layer3_adapter::owner_control::OwnerControlRoute::new(name);
    let transport: Arc<dyn layer3_adapter::hosted::HostedControlTransport> =
        Arc::new(transport.with_owner_router(route.clone()));
    routes.push((route, transport.clone()));
    transport
}

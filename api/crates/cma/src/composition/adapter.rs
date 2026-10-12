use crate::config::Config;
use std::sync::Arc;

pub(super) async fn production_adapter(
    config: &Config,
    checkpoint_objects: Arc<crate::store::MongoCheckpointObjectStore>,
    subscriptions: Arc<dyn layer3_adapter::hosted::HostedSubscriptionSource>,
    channel_tools: Arc<dyn layer3_adapter::hosted::HostedChannelToolSource>,
) -> Result<Arc<dyn layer3_adapter::SandboxAdapter>, String> {
    use layer3_adapter::codex::{CodexAdapter, CodexSandboxSpec};
    use layer3_adapter::hosted::{
        CheckpointArchiveStoreFactory, HostedDeployment, ProviderHostedBackend,
        ProviderHostedControlTransport,
    };
    use layer3_adapter::provider::{BatchSandboxReclaimer, SandboxClient};

    let reclaimer =
        BatchSandboxReclaimer::infer_namespaced(config.runtime.sandbox_namespace.clone())
            .await
            .map_err(|error| format!("sandbox cleanup authority is not usable: {error}"))?
            .for_creator(config.deployment_id.clone());
    reclaimer
        .preflight_cleanup_authority()
        .await
        .map_err(|error| format!("sandbox cleanup authority is not readable: {error}"))?;
    let client = SandboxClient::new(
        config.runtime.sandbox_base_url.clone(),
        config.runtime.sandbox_api_key.clone(),
    )
    .with_reclaimer(reclaimer);
    let archive_factory: Arc<dyn CheckpointArchiveStoreFactory> = checkpoint_objects;
    let transport = ProviderHostedControlTransport::new(client.clone(), archive_factory)
        .map_err(|error| format!("hosted runtime transport is not usable: {error}"))?
        .with_subscriptions(subscriptions);
    let backend = ProviderHostedBackend::new(client, Arc::new(transport), &config.deployment_id)
        .map_err(|error| format!("sandbox provider scope is not usable: {error}"))?;
    let mut deployment = HostedDeployment::default();
    deployment.image.uri = config.runtime.sandbox_image.clone();
    deployment.upstream_base_url = config.runtime.upstream_base_url.clone();
    deployment.upstream_wire_api = config.runtime.upstream_wire_api.clone();
    deployment.platform_base_url = config.runtime.platform_base_url.clone();
    deployment
        .contract()
        .map_err(|error| format!("hosted runtime image is not usable: {error}"))?;
    let mut adapter = CodexAdapter::with_backend(
        backend,
        CodexSandboxSpec {
            deployment,
            ..CodexSandboxSpec::default()
        },
    )
    .with_channel_tools(channel_tools);
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

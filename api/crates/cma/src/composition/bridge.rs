//! Exact deployment capabilities shared by channel setup and allocation.
use std::sync::Arc;

use crate::{
    auth::{keys::KeyIssuer, AuthMode},
    bridge::provisioning::BridgeProvisioningDependencies,
    config::Config,
    sandbox::{LiveSandboxPolicy, OrnnAgentSkillResolver},
    store::MongoStores,
};

pub(super) fn key_issuer(config: &Config) -> Option<Arc<KeyIssuer>> {
    let AuthMode::Provider { base_url, .. } = &config.auth_mode else {
        return None;
    };
    Some(Arc::new(KeyIssuer::new(
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("the rustls HTTP client builds"),
        base_url,
        &config.deployment_id,
    )))
}

pub(super) fn channel_schedulers(
    config: &Config,
    stores: &MongoStores,
    bridge: &crate::bridge::composition::BridgeComposition,
    definitions: &layer2_core::bridge::BridgeService,
    credentials: Option<Arc<crate::bridge::ManagedBridgeCredentials>>,
    adapter: Arc<dyn layer3_adapter::SandboxAdapter>,
) -> Result<Vec<crate::bridge::channel_scheduler::ChannelScheduler>, String> {
    use crate::bridge::{
        channel_scheduler::ChannelScheduler,
        channel_work::{ChannelAllocationWork, ChannelInboxWork},
        provisioning::BridgeProvisioningAuthority,
    };
    use crate::sandbox::{
        channel_children::ChannelChildDriver,
        channel_provisioning::{
            state::ChannelProvisioningSettings, ChannelProvisioningDependencies,
            ChannelProvisioningDriver,
        },
    };
    let Some(credentials) = credentials else {
        return Ok(Vec::new());
    };
    let Some(dependencies) = credentials.provisioning.clone() else {
        return Ok(Vec::new());
    };
    let authority = Arc::new(BridgeProvisioningAuthority::new(
        credentials.clone(),
        dependencies.clone(),
    ));
    let allocations = ChannelAllocationWork {
        definitions: definitions.clone(),
        allocations: bridge.allocations.clone(),
        authority: authority.clone(),
        driver: ChannelProvisioningDriver::new(
            ChannelProvisioningDependencies {
                allocations: bridge.allocations.clone(),
                credentials: credentials.clone(),
                issuer: dependencies.issuer.clone(),
                custody: stores.sandbox_credentials.clone(),
                adapter: adapter.clone(),
            },
            ChannelProvisioningSettings {
                deployment_id: config.deployment_id.clone(),
                runtime_version: config.runtime.sandbox_image.clone(),
                execution: config.execution,
                home_policy: config.runtime.home_policy.clone(),
            },
            format!("{}:channel:{}", config.deployment_id, uuid::Uuid::new_v4()),
        )
        .map_err(|error| format!("channel provisioning is not usable: {error}"))?,
        children: ChannelChildDriver {
            store: bridge.allocations.clone(),
            credentials: credentials.clone(),
            adapter: adapter.clone(),
        },
        startup: crate::sandbox::channel_startup::ChannelStartupDriver {
            allocations: bridge.allocations.clone(),
            resources: stores.sandbox_model.clone(),
            credentials: credentials.clone(),
            adapter,
            skills: dependencies.skills.clone(),
        },
    };
    let inputs = ChannelInboxWork {
        definitions: definitions.clone(),
        credentials,
        inbox: bridge.inbox.clone(),
        allocations: bridge.allocations.clone(),
        authority,
        invokes: bridge.invokes.clone(),
        invoke_store: bridge.invoke_store.clone(),
        commands: bridge.channel.clone(),
    };
    Ok(vec![
        ChannelScheduler::new(Arc::new(inputs)),
        ChannelScheduler::new(Arc::new(allocations)),
    ])
}

pub(super) fn provisioning(
    config: &Config,
    stores: &MongoStores,
    policy: Arc<LiveSandboxPolicy>,
    adapter: Arc<dyn layer3_adapter::SandboxAdapter>,
    capacity_bounds: layer2_core::profiles::ProfileCapacityBounds,
    issuer: Option<Arc<KeyIssuer>>,
) -> Result<Option<Arc<BridgeProvisioningDependencies>>, String> {
    let Some(issuer) = issuer else {
        return Ok(None);
    };
    Ok(Some(Arc::new(BridgeProvisioningDependencies {
        sandbox_profiles: stores.sandbox_profiles.clone(),
        workspace_profiles: stores.sandbox_profiles.clone(),
        agent_profiles: stores.sandbox_profiles.clone(),
        policy,
        adapter,
        issuer,
        skills: OrnnAgentSkillResolver::anonymous(&config.runtime.platform_base_url)?,
        capacity_bounds,
    })))
}

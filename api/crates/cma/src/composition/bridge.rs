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

pub(super) async fn channel_schedulers(
    config: &Config,
    stores: &MongoStores,
    bridge: &crate::bridge::composition::BridgeComposition,
    definitions: &layer2_core::bridge::BridgeService,
    credentials: Option<Arc<crate::bridge::ManagedBridgeCredentials>>,
    adapter: Arc<dyn layer3_adapter::SandboxAdapter>,
    registry: Arc<crate::sandbox::LiveAgentRegistry>,
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
    let cleanup_authority = credentials.as_ref().and_then(|credentials| {
        credentials.provisioning.clone().map(|dependencies| {
            Arc::new(BridgeProvisioningAuthority::new(
                credentials.clone(),
                dependencies,
            ))
        })
    });
    // New cleanup admission follows management feature policy. Already accepted
    // jobs continue their independently authorized drain when message execution
    // or allocation is disabled; those flags never supply removal authority.
    let cleanup = crate::bridge::cleanup_work::ChannelCleanupWork {
        allocations: bridge.allocations.clone(),
        authority: Arc::new(crate::bridge::cleanup_work::ManagedCleanupWorkAuthority {
            definitions: definitions.clone(),
            authority: cleanup_authority,
        }),
        adapter: adapter.clone(),
        registry,
        deployment_id: config.deployment_id.clone(),
        holder: format!("{}:cleanup:{}", config.deployment_id, uuid::Uuid::new_v4()),
    };
    let cleanup = ChannelScheduler::new(Arc::new(cleanup));
    let Some(credentials) = credentials else {
        return Ok(vec![cleanup]);
    };
    let Some(dependencies) = credentials.provisioning.clone() else {
        return Ok(vec![cleanup]);
    };
    let authority = Arc::new(BridgeProvisioningAuthority::new(
        credentials.clone(),
        dependencies.clone(),
    ));
    bridge
        .allocations
        .initialize_channel_renewal_discovery()
        .await
        .map_err(|error| format!("channel renewal discovery is not usable: {error}"))?;
    let renewals = crate::bridge::channel_renewal_work::ChannelRenewalWork {
        definitions: definitions.clone(),
        allocations: bridge.allocations.clone(),
        authority: authority.clone(),
        driver: crate::sandbox::channel_renewal::ChannelCredentialRenewalDriver {
            allocations: bridge.allocations.clone(),
            credentials: credentials.clone(),
            issuer: dependencies.issuer.clone(),
            custody: stores.sandbox_credentials.clone(),
            adapter: adapter.clone(),
            holder: format!("{}:renewal:{}", config.deployment_id, uuid::Uuid::new_v4()),
        },
    };
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
        reuse: crate::sandbox::channel_reuse::ChannelReuseDriver {
            allocations: bridge.allocations.clone(),
            credentials: credentials.clone(),
            adapter: adapter.clone(),
            settings: ChannelProvisioningSettings {
                deployment_id: config.deployment_id.clone(),
                runtime_version: config.runtime.sandbox_image.clone(),
                execution: config.execution,
                home_policy: config.runtime.home_policy.clone(),
            },
            holder: format!(
                "{}:channel-reuse:{}",
                config.deployment_id,
                uuid::Uuid::new_v4()
            ),
            startup: crate::sandbox::channel_reuse::startup::ChannelReuseStartupDriver {
                allocations: bridge.allocations.clone(),
                resources: stores.sandbox_model.clone(),
                credentials: credentials.clone(),
                adapter: adapter.clone(),
                skills: dependencies.skills.clone(),
            },
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
        ChannelScheduler::new(Arc::new(renewals)),
        cleanup,
    ])
}

pub(super) fn provisioning(
    config: &Config,
    stores: &MongoStores,
    policy: Arc<LiveSandboxPolicy>,
    adapter: Arc<dyn layer3_adapter::SandboxAdapter>,
    capacity_bounds: layer2_core::profiles::ProfileCapacityBounds,
    issuer: Option<Arc<KeyIssuer>>,
    organizations: Arc<crate::auth::AuthState>,
) -> Result<Option<Arc<BridgeProvisioningDependencies>>, String> {
    let Some(issuer) = issuer else {
        return Ok(None);
    };
    Ok(Some(Arc::new(BridgeProvisioningDependencies {
        resources: stores.sandbox_model.clone(),
        resource_access: Arc::new(layer2_core::resource_access::ResourceAccessService::new(
            stores.sandbox_model.clone(),
            organizations,
        )),
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

pub(super) fn sandbox_credentials(
    issuer: Option<Arc<KeyIssuer>>,
    auth: Arc<crate::auth::AuthState>,
    stores: &MongoStores,
    bridge: &crate::bridge::composition::BridgeComposition,
    definitions: &layer2_core::bridge::BridgeService,
    credentials: Option<Arc<crate::bridge::ManagedBridgeCredentials>>,
    adapter: Arc<dyn layer3_adapter::SandboxAdapter>,
) -> Arc<dyn crate::sandbox::SandboxCredentialController> {
    use crate::sandbox::{
        ManagedSandboxCredentials, SandboxCredentialController, UnavailableSandboxCredentials,
    };
    let ordinary: Arc<dyn SandboxCredentialController> = match issuer {
        Some(issuer) => {
            ManagedSandboxCredentials::new(issuer, auth, stores.sandbox_credentials.clone())
        }
        None => Arc::new(UnavailableSandboxCredentials),
    };
    crate::sandbox::channel_credentials::ChannelGuardedCredentials::with_retirement(
        ordinary,
        bridge.allocations.clone(),
        channel_lifecycle_credentials(
            stores,
            bridge,
            definitions,
            credentials.clone(),
            adapter.clone(),
        ),
        channel_retirement(stores, bridge, definitions, credentials, adapter),
    )
}

fn channel_retirement(
    stores: &MongoStores,
    bridge: &crate::bridge::composition::BridgeComposition,
    definitions: &layer2_core::bridge::BridgeService,
    credentials: Option<Arc<crate::bridge::ManagedBridgeCredentials>>,
    adapter: Arc<dyn layer3_adapter::SandboxAdapter>,
) -> Option<Arc<dyn crate::sandbox::channel_retirement::ChannelRetirement>> {
    use crate::bridge::provisioning::{
        retirement::ManagedChannelRetirementAuthority, BridgeProvisioningAuthority,
    };
    let credentials = credentials?;
    let dependencies = credentials.provisioning.clone()?;
    Some(Arc::new(
        crate::sandbox::channel_retirement::ChannelRetirementDriver {
            authority: Arc::new(ManagedChannelRetirementAuthority {
                definitions: definitions.clone(),
                allocations: bridge.allocations.clone(),
                authority: Arc::new(BridgeProvisioningAuthority::new(
                    credentials.clone(),
                    dependencies.clone(),
                )),
            }),
            allocations: bridge.allocations.clone(),
            credentials,
            issuer: dependencies.issuer.clone(),
            custody: stores.sandbox_credentials.clone(),
            adapter,
        },
    ))
}

fn channel_lifecycle_credentials(
    stores: &MongoStores,
    bridge: &crate::bridge::composition::BridgeComposition,
    definitions: &layer2_core::bridge::BridgeService,
    credentials: Option<Arc<crate::bridge::ManagedBridgeCredentials>>,
    adapter: Arc<dyn layer3_adapter::SandboxAdapter>,
) -> Option<Arc<dyn crate::sandbox::channel_lifecycle_credentials::ChannelLifecycleCredentials>> {
    use crate::bridge::provisioning::{
        revival::ManagedChannelRevivalAuthority, BridgeProvisioningAuthority,
    };
    let credentials = credentials?;
    let dependencies = credentials.provisioning.clone()?;
    Some(Arc::new(
        crate::sandbox::channel_lifecycle_credentials::ChannelLifecycleCredentialDriver {
            resources: stores.sandbox_model.clone(),
            authority: Arc::new(ManagedChannelRevivalAuthority {
                definitions: definitions.clone(),
                allocations: bridge.allocations.clone(),
                authority: Arc::new(BridgeProvisioningAuthority::new(
                    credentials.clone(),
                    dependencies.clone(),
                )),
            }),
            allocations: bridge.allocations.clone(),
            credentials,
            issuer: dependencies.issuer.clone(),
            custody: stores.sandbox_credentials.clone(),
            adapter,
        },
    ))
}

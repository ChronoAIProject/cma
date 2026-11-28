use super::*;
use crate::profile_authoring::CmaProfileAuthoringSource;
use layer2_core::profiles::{authoring::ProfileAuthoringService, ProfileCapacityBounds};

pub(super) fn authoring_router(
    config: &Config,
    auth: &Arc<AuthState>,
    bounds: ProfileCapacityBounds,
) -> Result<axum::Router, String> {
    let service = ProfileAuthoringService::new(
        bounds,
        CmaProfileAuthoringSource::production(config, Arc::clone(auth))?,
    );
    let public_origin = config
        .nyxid_public_url
        .as_deref()
        .map(reqwest::Url::parse)
        .transpose()
        .map_err(|_| "NyxID public origin is invalid".to_string())?;
    Ok(rest::profile_authoring_router(service, public_origin))
}

pub(super) async fn seed_profiles(
    stores: &MongoStores,
    config: &Config,
    policy: &layer1_public::rest::ProfileRoutePolicy,
) -> Result<(), String> {
    let Some(stored) = stores
        .settings
        .read_for_startup()
        .await
        .map_err(|e| e.to_string())?
    else {
        return Ok(());
    };
    let defaults = stored.settings.defaults;
    if defaults.agent_profile.id.as_str() != layer2_core::DEFAULT_AGENT_PROFILE_ID
        && defaults.workspace_profile.id.as_str() != layer2_core::DEFAULT_WORKSPACE_PROFILE_ID
        && defaults.sandbox_profile.id.as_str() != layer2_core::DEFAULT_SANDBOX_PROFILE_ID
    {
        return Ok(());
    }
    stores
        .sandbox_profiles
        .seed_default_profiles(
            &DefaultProfileSpec {
                cpu_millis: config.default_profiles.cpu_millis,
                memory_mib: config.default_profiles.memory_mib,
                agent_model: config.default_profiles.agent_model.clone(),
                agent_effort: config.default_profiles.agent_effort,
            },
            policy.capacity_bounds,
            policy.public_value_scanner.as_ref(),
        )
        .await
        .map_err(|error| format!("default profiles are not usable: {error}"))
}

pub(super) fn live_policy(
    config: &Config,
    settings: &Arc<layer2_core::PlatformSettingsService>,
) -> Arc<crate::sandbox::LiveSandboxPolicy> {
    Arc::new(crate::sandbox::LiveSandboxPolicy::new(
        Arc::clone(settings),
        config.baseline_services.clone(),
    ))
}

pub(super) fn platform_settings(
    config: &Config,
    stores: &MongoStores,
    activity: &Arc<ActivityService>,
) -> Arc<PlatformSettingsService> {
    Arc::new(
        PlatformSettingsService::new(stores.settings.clone(), Arc::clone(activity))
            .with_model_budget_writes_enabled(config.features.model_budget_writes_enabled)
            .with_model_request_budget_writes_enabled(
                config.features.model_request_budget_writes_enabled,
            )
            .with_platform_spend_writes_enabled(config.features.platform_spend_writes_enabled),
    )
}

pub(super) fn route_state(
    stores: &MongoStores,
    operations: &Arc<dyn OperationStore>,
    settings: &Arc<PlatformSettingsService>,
    policy: layer1_public::rest::ProfileRoutePolicy,
) -> ProfilesState {
    ProfilesState::new(
        stores.sandbox_profiles.clone(),
        Arc::clone(operations),
        Arc::clone(settings),
        policy,
    )
}

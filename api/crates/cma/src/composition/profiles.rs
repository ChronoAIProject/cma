use super::*;

pub(super) async fn seed_profiles(
    stores: &MongoStores,
    config: &Config,
    policy: &layer1_public::rest::ProfileRoutePolicy,
) -> Result<(), String> {
    use layer2_core::PlatformSettingsStore;
    let defaults = stores
        .settings
        .read()
        .await
        .map_err(|e| e.to_string())?
        .settings
        .defaults;
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

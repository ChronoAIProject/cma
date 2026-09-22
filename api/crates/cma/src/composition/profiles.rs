use super::*;

pub(super) async fn seed_profiles(
    stores: &MongoStores,
    config: &Config,
    policy: &layer1_public::rest::ProfileRoutePolicy,
) -> Result<(), String> {
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

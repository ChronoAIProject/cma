use super::*;

pub(super) fn connection(
    owner: &ResourceOwner,
    config: &BridgeConfig,
    key: &BridgeCredentialLease,
) -> Result<BridgeValidation, BridgeError> {
    connection_with_target(owner, config, key, None)
}

pub(super) fn connection_with_target(
    owner: &ResourceOwner,
    config: &BridgeConfig,
    key: &BridgeCredentialLease,
    target: Option<layer2_core::resource_access::ResourceAccessSnapshot>,
) -> Result<BridgeValidation, BridgeError> {
    BridgeValidation::from_observation(
        owner,
        config,
        BridgeConnectionObservation {
            connector: config.connector.clone(),
            bot_owner: owner.clone(),
            key_owner: owner.clone(),
            routes: key.routes.clone(),
            custody_owner: key.owner.clone(),
            custody_revision: key.custody_revision,
            key_state_version: key.key_state_version,
            key_active: true,
            custody_available: true,
            callback_conflict: false,
            renewable_sandbox_authority: true,
            exact_profiles_available: true,
            existing_target: target,
            deadline: key
                .deadline
                .min(tokio::time::Instant::now() + Duration::from_secs(30)),
        },
    )
}

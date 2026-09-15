use super::*;
use layer2_core::resource_access::{ResourceAccessStore, ResourceOwner};

/// Provider availability is a fixture; target lineage is read from real Mongo.
pub(super) struct Connections(pub Arc<crate::store::MongoSandboxStore>);

#[async_trait::async_trait]
impl BridgeConnectionValidator for Connections {
    async fn validate_connection(
        &self,
        owner: &ResourceOwner,
        config: &BridgeConfig,
    ) -> Result<BridgeValidation, BridgeError> {
        let target = config
            .placement
            .reused_parent()
            .ok_or(BridgeError::AuthorityUnavailable)?;
        let snapshot = self
            .0
            .resource_access_snapshot(&target)
            .await
            .map_err(|_| BridgeError::AuthorityUnavailable)?;
        BridgeValidation::from_observation(
            owner,
            config,
            BridgeConnectionObservation {
                connector: config.connector.clone(),
                bot_owner: owner.clone(),
                key_owner: owner.clone(),
                routes: config
                    .connector
                    .route_ids
                    .iter()
                    .map(|route_id| BridgeRouteObservation {
                        route_id: route_id.clone(),
                        owner: owner.clone(),
                        bot_id: config.connector.bot_id.clone(),
                        key_id: config.connector.key_id.clone(),
                        enabled: true,
                    })
                    .collect(),
                custody_owner: owner.clone(),
                custody_revision: 1,
                key_state_version: 3,
                key_active: true,
                custody_available: true,
                callback_conflict: false,
                renewable_sandbox_authority: true,
                exact_profiles_available: true,
                existing_target: Some(snapshot),
                deadline: tokio::time::Instant::now() + Duration::from_secs(30),
            },
        )
    }
}

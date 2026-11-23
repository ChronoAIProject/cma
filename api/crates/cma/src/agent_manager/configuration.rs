use super::*;
use crate::config::{AgentManagerReplicas, FeatureFlags};

impl AgentManager {
    pub(crate) async fn register_configured(
        store: Arc<MongoSandboxStore>,
        registry: Arc<LiveAgentRegistry>,
        settings: &AgentManagerReplicas,
        features: &FeatureFlags,
    ) -> Result<Arc<Self>, String> {
        if features.am_failure_containment_enabled {
            return Self::register_with_policy(
                store,
                registry,
                settings.relay_address.clone(),
                settings.max_owned_agents,
                features.ownership_cleanup_pacing,
                MaintenancePolicy::ContainFailures,
            )
            .await;
        }
        let manager = Self::register(
            store,
            registry,
            settings.relay_address.clone(),
            settings.max_owned_agents,
            features.ownership_cleanup_pacing,
        )
        .await?;
        if features.receiver_housekeeping_containment {
            manager.enable_receiver_containment();
        }
        Ok(manager)
    }
}

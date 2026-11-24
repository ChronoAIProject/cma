use super::*;
use crate::config::{AgentManagerReplicas, FeatureFlags};

impl AgentManager {
    pub(crate) async fn register_configured(
        store: Arc<MongoSandboxStore>,
        registry: Arc<LiveAgentRegistry>,
        settings: &AgentManagerReplicas,
        features: &FeatureFlags,
    ) -> Result<Arc<Self>, String> {
        let policy = if features.am_failure_containment_enabled {
            MaintenancePolicy::ContainFailures
        } else {
            MaintenancePolicy::Legacy
        };
        let manager = Self::register_with_policy_options(
            store,
            registry,
            settings.relay_address.clone(),
            settings.max_owned_agents,
            features.ownership_cleanup_pacing,
            policy,
            features.batch_ownership_renewal,
        )
        .await?;
        if features.receiver_housekeeping_containment && !features.am_failure_containment_enabled {
            manager.enable_receiver_containment();
        }
        Ok(manager)
    }
}

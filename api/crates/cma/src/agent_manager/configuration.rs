use super::*;
use crate::config::{AgentManagerReplicas, FeatureFlags};

fn batch_renewal_enabled(features: &FeatureFlags) -> bool {
    features.batch_ownership_renewal && !features.am_failure_containment_enabled
}

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
        // Containment owns independently supervised renewal and retry state.
        // If both rollout flags are set, containment takes precedence and the
        // legacy batch scheduler is deliberately not allocated.
        let batch_renewal = batch_renewal_enabled(features);
        let manager = Self::register_with_policy_options(
            store,
            registry,
            settings.relay_address.clone(),
            settings.max_owned_agents,
            features.ownership_cleanup_pacing,
            policy,
            batch_renewal,
        )
        .await?;
        if features.receiver_housekeeping_containment && !features.am_failure_containment_enabled {
            manager.enable_receiver_containment();
        }
        Ok(manager)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn containment_precedes_batch_renewal() {
        let features = FeatureFlags {
            batch_ownership_renewal: true,
            am_failure_containment_enabled: true,
            ..FeatureFlags::default()
        };
        assert!(!batch_renewal_enabled(&features));
    }
}

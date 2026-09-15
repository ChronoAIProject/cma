//! Rollout availability wraps the existing current custody/resource verifier.
use std::sync::Arc;

use layer2_core::bridge::{
    BridgeConfig, BridgeConnectionValidator, BridgeError, BridgeFeature, BridgePlatform,
    BridgeValidation,
};
use layer2_core::resource_access::ResourceOwner;

use crate::config::{
    bridge_flags::{BridgePlatformFlag, BridgeStage},
    BridgeFeatureFlags,
};

pub(super) fn configured(
    inner: Arc<dyn BridgeConnectionValidator>,
    flags: BridgeFeatureFlags,
) -> Arc<dyn BridgeConnectionValidator> {
    Arc::new(ConfiguredBridgeConnections { inner, flags })
}

struct ConfiguredBridgeConnections {
    inner: Arc<dyn BridgeConnectionValidator>,
    flags: BridgeFeatureFlags,
}

#[async_trait::async_trait]
impl BridgeConnectionValidator for ConfiguredBridgeConnections {
    fn check_feature(
        &self,
        feature: BridgeFeature,
        platform: &BridgePlatform,
    ) -> Result<(), BridgeError> {
        let stage = match feature {
            BridgeFeature::Management => BridgeStage::Management,
            BridgeFeature::Ingress => BridgeStage::Ingress,
            BridgeFeature::Allocation => BridgeStage::Allocation,
            BridgeFeature::Execution => BridgeStage::Execution,
            BridgeFeature::Outbound => BridgeStage::Outbound,
        };
        let platform = match platform {
            BridgePlatform::Telegram => BridgePlatformFlag::Telegram,
            BridgePlatform::Discord => BridgePlatformFlag::Discord,
            BridgePlatform::Lark => BridgePlatformFlag::Lark,
            BridgePlatform::Feishu => BridgePlatformFlag::Feishu,
        };
        let result = if stage == BridgeStage::Execution {
            self.flags.check_command_release(Some(platform))
        } else {
            self.flags.check_new_effect(stage, platform)
        };
        result.map_err(|_| BridgeError::Unavailable)
    }

    async fn validate_connection(
        &self,
        owner: &ResourceOwner,
        config: &BridgeConfig,
    ) -> Result<BridgeValidation, BridgeError> {
        // Released FIFO commands, maintenance and receipt reconciliation retain
        // current authorization even when new stage effects are disabled.
        self.inner.validate_connection(owner, config).await
    }
}

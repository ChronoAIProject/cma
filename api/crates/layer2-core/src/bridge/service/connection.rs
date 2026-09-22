//! Full setup validation and the narrower retained-channel invocation entry.
use super::*;

impl BridgeService {
    pub(in crate::bridge) async fn connection(
        &self,
        owner: &ResourceOwner,
        config: &BridgeConfig,
    ) -> Result<BridgeValidation, BridgeError> {
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            self.connections.validate_connection(owner, config),
        )
        .await
        .map_err(|_| BridgeError::AuthorityUnavailable)??;
        result.require(owner, config)?;
        Ok(result)
    }

    pub(in crate::bridge) async fn retained_channel_connection(
        &self,
        grant: &super::invoke::BridgeInvokeGrant,
        effect: &super::invoke::BridgeInvokeEffect,
        resources: &crate::resource_access::ResourceAccessSnapshot,
    ) -> Result<BridgeValidation, super::invoke::BridgeInvokeError> {
        let request = super::RetainedChannelConnection::new(grant, effect, resources)?;
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            self.connections.validate_retained_channel(&request),
        )
        .await
        .map_err(|_| BridgeError::AuthorityUnavailable)??;
        result.require_invocation(grant, effect)?;
        Ok(result)
    }
}

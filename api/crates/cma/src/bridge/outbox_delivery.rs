use std::sync::Arc;

use layer2_core::bridge::outbox::*;

use super::{custody::BridgeCredentialResolver, ManagedBridgeCredentials};
use crate::store::MongoBridgeReplyToolStore;

pub(crate) struct NyxOutboxDelivery {
    store: Arc<MongoBridgeReplyToolStore>,
    credentials: Arc<ManagedBridgeCredentials>,
}
impl NyxOutboxDelivery {
    pub(crate) fn new(
        store: Arc<MongoBridgeReplyToolStore>,
        credentials: Arc<ManagedBridgeCredentials>,
    ) -> Self {
        Self { store, credentials }
    }
    async fn ready(
        &self,
        request: &RelayDeliveryRequest,
    ) -> Result<super::custody::BridgeCredentialLease, DeliveryFailure> {
        request
            .require_current()
            .map_err(|_| DeliveryFailure::CredentialExpired)?;
        let item = request.item();
        let DeliveryCredentialRef::RetainedKey {
            reference,
            revision,
            key_state_version,
        } = &item.credential
        else {
            return Err(DeliveryFailure::CapabilityUnsupported);
        };
        let lease = self
            .credentials
            .resolve(item.source.owner(), item.source.config())
            .await
            .map_err(|_| DeliveryFailure::CredentialRevoked)?;
        if &lease.custody != reference
            || lease.custody_revision != *revision
            || lease.key_state_version != *key_state_version
            || lease.key_id != item.source.config().connector.key_id
            || lease.owner != *item.source.owner()
        {
            return Err(DeliveryFailure::CredentialRevoked);
        }
        self.store
            .require_sending(request)
            .await
            .map_err(|_| DeliveryFailure::RouteUnavailable)?;
        self.credentials
            .revalidate(&lease)
            .await
            .map_err(|_| DeliveryFailure::CredentialRevoked)?;
        request
            .require_current()
            .map_err(|_| DeliveryFailure::CredentialExpired)?;
        Ok(lease)
    }
}
#[async_trait::async_trait]
impl RelayDeliveryPort for NyxOutboxDelivery {
    async fn deliver(&self, request: RelayDeliveryRequest) -> DeliveryOutcome {
        // Local verification errors prove no send. Once the provider call
        // starts, every uncertain result is reported as Unknown by that port.
        let lease = match tokio::time::timeout_at(request.deadline(), self.ready(&request)).await {
            Ok(Ok(lease)) => lease,
            Ok(Err(reason)) => return DeliveryOutcome::DefinitiveRejection(reason),
            Err(_) => {
                return DeliveryOutcome::DefinitiveRejection(DeliveryFailure::CredentialExpired)
            }
        };
        self.credentials
            .provider
            .reply(&request, &lease.value)
            .await
    }
}

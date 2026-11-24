use super::*;
use crate::owner_protocol::{
    ReceiverRevocation, ReceiverSettlementFailure, ReceiverSettlementWatch,
};

#[async_trait::async_trait]
impl ReceiverRevocation for ReceiverFence {
    fn identity(&self) -> String {
        self.identity.clone()
    }

    fn sandbox_id(&self) -> Option<&str> {
        self.endpoint.native_identity()?.sandbox_id.as_deref()
    }

    async fn revoke(&self) -> crate::Result<()> {
        self.settle(ReceiverConfirmationCustody::PossiblyDelivered)
            .await?;
        Ok(())
    }

    async fn settle(&self, custody: ReceiverConfirmationCustody) -> crate::Result<ReceiverCleanup> {
        match self.settlement_exchange(custody).await {
            Ok(outcome) => Ok(outcome),
            Err(failure) => {
                if let Some(authority) = self.authority.upgrade() {
                    authority.record_failure(failure.observation).await;
                }
                Err(failure.error)
            }
        }
    }

    async fn settle_typed(
        &self,
        custody: ReceiverConfirmationCustody,
        outcome: &ReceiverSettlementWatch,
    ) -> Result<ReceiverCleanup, ReceiverSettlementFailure> {
        match self.settlement_exchange(custody).await {
            Ok(cleanup) => Ok(cleanup),
            Err(failure) => {
                let kind = failure.settlement_failure();
                outcome.record(kind);
                if let Some(authority) = self.authority.upgrade() {
                    authority.record_failure(failure.observation).await;
                }
                Err(kind)
            }
        }
    }
}

impl ReceiverFence {
    async fn settlement_exchange(
        &self,
        custody: ReceiverConfirmationCustody,
    ) -> Result<ReceiverCleanup, OwnershipExchangeFailure> {
        let status = Arc::new(NativeHttpObservation::default());
        let operation = async {
            let mut transport = self.template.clone();
            // A dropped link must not discard custody of a receiver that may
            // still hold this lease. A native revoke uses independent state
            // bound to the captured incarnation; its open fences refuse a
            // replaced Pod or container before anything is sent.
            transport.state = match self.state.upgrade() {
                Some(state) => state,
                None => super::native_pool::revoke_state(&self.endpoint)
                    .map(|state| Arc::new(Mutex::new(state)))
                    .ok_or_else(|| {
                        OwnershipFailure::new(Phase::Revoke, Reason::TransportUnavailable)
                    })?,
            };
            let result: Result<(OwnerAcknowledgement, _), _> = transport
                .owner_exchange(
                    &self.endpoint,
                    &self.owner,
                    Phase::Revoke,
                    &self.owner,
                    &status,
                )
                .await;
            match result {
                Ok((reply, status)) => {
                    validate_acknowledgement(&reply, &self.owner, Phase::Revoke, status)?;
                    Ok(ReceiverCleanup::Acknowledged)
                }
                Err(failure) => {
                    if custody == ReceiverConfirmationCustody::NeverSent
                        && failure.observation.reason == Reason::MissingRoute
                        && failure.observation.http_status == Some(404)
                        && self.legacy_incarnation.is_some()
                        && matches!(
                            transport.legacy_receiver_incarnation(&self.endpoint).await,
                            Ok(current) if current == self.legacy_incarnation
                        )
                    {
                        use sha2::{Digest, Sha256};
                        tracing::info!(
                            stage = "revoke",
                            http_status = 404_u16,
                            outcome = "no_receiver_known_incompatible",
                            receiver_sha256 = %format!("{:x}", Sha256::digest(self.identity.as_bytes())),
                            "Native receiver cleanup obligation settled"
                        );
                        return Ok(ReceiverCleanup::NoReceiver);
                    }
                    Err(failure)
                }
            }
        };
        tokio::time::timeout(NEGOTIATION_DEADLINE, operation)
            .await
            .unwrap_or_else(|_| {
                let mut failure = OwnershipFailure::new(Phase::Revoke, Reason::Deadline);
                failure.http_status = status.status();
                Err(failure.into())
            })
    }
}

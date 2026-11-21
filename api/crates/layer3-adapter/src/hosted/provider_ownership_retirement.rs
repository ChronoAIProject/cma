use super::*;

struct FailedReceiver {
    transport: ProviderHostedControlTransport,
    endpoint: RuntimeEndpoint,
    owner: TransportOwner,
}
impl ProviderHostedControlTransport {
    pub(super) fn record_failed_challenge(
        &self,
        endpoint: &RuntimeEndpoint,
        owner: &TransportOwner,
        phase: Phase,
        status: http::StatusCode,
    ) {
        if phase != Phase::Challenge || !crate::receiver_failure::unavailable_status(status) {
            return;
        }
        crate::receiver_failure::record(|| {
            let mut transport = self.clone();
            transport.state = Arc::new(Mutex::new(TransportState::default()));
            Arc::new(FailedReceiver {
                transport,
                endpoint: endpoint.clone(),
                owner: owner.clone(),
            })
        });
    }
}
#[async_trait::async_trait]
impl crate::receiver_failure::ReceiverFailureProbe for FailedReceiver {
    async fn still_unavailable(&self) -> bool {
        let status = Arc::new(NativeHttpObservation::default());
        let (_, failed) = crate::receiver_failure::observe(async {
            tokio::time::timeout(
                NEGOTIATION_DEADLINE,
                self.transport.owner_exchange::<_, OwnerChallenge>(
                    &self.endpoint,
                    &self.owner,
                    Phase::Challenge,
                    &self.owner,
                    &status,
                ),
            )
            .await
        })
        .await;
        // A timeout alone or rejection of a stale owner token is not evidence.
        // Only a new 404 or server failure returned by the receiver counts.
        failed.is_some()
    }
}

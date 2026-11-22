use super::*;
use crate::owner_protocol::{OwnershipEvidence, ReceiverRevocation, TransportOwner};

struct ObservedAuthority {
    inner: Arc<TestAuthority>,
    evidence: Arc<OwnershipEvidence>,
}
impl ObservedAuthority {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: TestAuthority::new(7),
            evidence: Arc::default(),
        })
    }
}
#[async_trait::async_trait]
impl TransportAuthority for ObservedAuthority {
    fn retry_evidence(&self) -> Option<Arc<OwnershipEvidence>> {
        Some(self.evidence.clone())
    }
    fn owner(&self) -> TransportOwner {
        self.inner.owner()
    }
    fn is_current(&self) -> bool {
        self.inner.is_current()
    }
    fn register_receiver(&self, receiver: Arc<dyn ReceiverRevocation>) -> crate::Result<()> {
        self.inner.register_receiver(receiver)
    }
    fn begin_receiver_confirmation(&self, identity: &str) -> crate::Result<()> {
        self.inner.begin_receiver_confirmation(identity)
    }
    async fn check(&self) -> crate::Result<()> {
        self.inner.check().await
    }
    async fn renew_for_challenge(&self) -> crate::Result<()> {
        self.inner.renew_for_challenge().await
    }
    async fn record_failure(&self, failure: OwnershipFailure) {
        self.evidence.record_unknown(self.owner(), failure);
        self.inner.record_failure(failure).await;
    }
    async fn revoked(&self) {
        self.inner.revoked().await;
    }
    fn revoke(&self) {
        self.inner.revoke();
    }
}

#[tokio::test]
async fn bound_initial_failure_carries_target_and_never_sends_confirmation_or_control() {
    for (code, reason, valid) in [
        (404, Reason::MissingRoute, false),
        (5, Reason::ProtocolMismatch, true),
        (6, Reason::ProtocolMismatch, false),
        (8, Reason::LeaseMismatch, true),
    ] {
        let fixture = fixture().await;
        let endpoint = fixture.endpoint().await;
        let authority = ObservedAuthority::new();
        fixture.state.fault.store(1000 + code, Ordering::SeqCst);
        assert!(scope(
            authority.clone(),
            fixture.transport.receiver_lease(&endpoint)
        )
        .await
        .is_err());
        let first = authority.evidence.first_failure().unwrap();
        assert_eq!(first.owner(), &authority.owner());
        let target = first.receiver().unwrap();
        assert_eq!(target.runtime_id(), handle().as_str());
        assert_eq!(
            target,
            &crate::owner_protocol::OwnershipReceiver::from_native(
                &handle(),
                &endpoint.native_binding().unwrap().1
            )
            .unwrap()
        );
        assert_eq!(first.failure().reason, reason);
        assert_eq!(first.challenge_valid(), valid);
        assert!(!first.confirmation_sent());
        assert!(!first.previously_confirmed());
        assert_eq!(
            fixture.state.ownership_confirmations.load(Ordering::SeqCst),
            0
        );
        assert!(!fixture.state.requests.lock().unwrap().iter().any(|r| r
            .lines()
            .next()
            .unwrap()
            .contains("/confirm ")
            || r.starts_with("POST /runtime/control ")));
        let receiver = authority.inner.receivers.lock().unwrap()[0].clone();
        fixture.state.fault.store(3403, Ordering::SeqCst);
        assert!(receiver.revoke().await.is_err());
        assert_eq!(
            authority.evidence.first_failure().unwrap().failure(),
            first.failure()
        );
        fixture.transport.retire(&handle()).await.unwrap();
        fixture.drained().await;
    }
}

#[tokio::test]
async fn rejection_after_confirm_and_ambiguous_confirm_are_excluded_by_history() {
    for completed in [false, true] {
        let fixture = fixture().await;
        let endpoint = fixture.endpoint().await;
        let authority = ObservedAuthority::new();
        let lease = if completed {
            let lease = scope(
                authority.clone(),
                fixture.transport.receiver_lease(&endpoint),
            )
            .await
            .unwrap();
            fixture.state.fault.store(1005, Ordering::SeqCst);
            lease
        } else {
            fixture.state.fault.store(2002, Ordering::SeqCst);
            None
        };
        assert!(scope(
            authority.clone(),
            fixture.transport.receiver_lease(&endpoint)
        )
        .await
        .is_err());
        let first = authority.evidence.first_failure().unwrap();
        assert!(first.confirmation_sent());
        assert_eq!(first.previously_confirmed(), completed);
        assert_eq!(
            first.failure().phase,
            if completed {
                Phase::Challenge
            } else {
                Phase::Confirm
            }
        );
        drop(lease);
        fixture.transport.retire(&handle()).await.unwrap();
        fixture.drained().await;
    }
}

#[tokio::test]
async fn direct_endpoint_missing_route_does_not_claim_verified_receiver_context() {
    use crate::hosted::provider_transport::tests::{listener, read_request};
    let listener = listener().await;
    let address = listener.local_addr().unwrap();
    let endpoint =
        RuntimeEndpoint::parse("http://fixture.invalid", &format!("http://{address}")).unwrap();
    let transport = ProviderHostedControlTransport::for_retained_opensandbox(SandboxClient::new(
        "http://unused.invalid",
        "fixture-key",
    ))
    .unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = String::from_utf8(read_request(&mut stream).await).unwrap();
        assert!(request.contains("/manager-ownership/challenge"));
        stream
            .write_all(b"HTTP/1.1 404 Missing\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
    });
    let authority = ObservedAuthority::new();
    assert!(
        scope(authority.clone(), transport.receiver_lease(&endpoint))
            .await
            .is_err()
    );
    server.await.unwrap();
    let first = authority.evidence.first_failure().unwrap();
    assert_eq!(first.failure().reason, Reason::MissingRoute);
    assert!(first.receiver().is_none());
    assert!(!first.confirmation_sent());
}

use super::*;
use crate::owner_protocol::{
    OwnershipEvidence, ReceiverCleanup, ReceiverConfirmationCustody, ReceiverRevocation,
    TransportOwner,
};

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

async fn confirmed_receiver_after_transport_drop(
    fixture: &mut Fixture,
) -> (Arc<TestAuthority>, Arc<dyn ReceiverRevocation>) {
    let endpoint = fixture.endpoint().await;
    assert!(endpoint.native_binding().is_some());
    let authority = TestAuthority::new(7);
    let lease = scope(
        authority.clone(),
        fixture.transport.receiver_lease(&endpoint),
    )
    .await
    .expect("original receiver must confirm")
    .expect("the confirmed receiver must have a renewal lease");
    assert_eq!(
        fixture.state.ownership_confirmations.load(Ordering::SeqCst),
        1
    );
    let receiver = authority.receivers.lock().unwrap()[0].clone();
    drop(lease);

    // Keep the loopback server alive while dropping the state of the actual
    // transport that opened and confirmed the receiver.
    let original_state = Arc::downgrade(&fixture.transport.state);
    let mut fresh_transport = fixture.transport.as_ref().clone();
    fresh_transport.state = Arc::new(Mutex::new(TransportState::default()));
    fixture.transport = Arc::new(fresh_transport);
    assert!(
        original_state.upgrade().is_none(),
        "original transport state must be gone"
    );
    authority.revoke();
    (authority, receiver)
}

#[tokio::test]
async fn native_revocation_after_transport_drop_reaches_the_original_receiver() {
    let mut fixture = fixture().await;
    let (authority, receiver) = confirmed_receiver_after_transport_drop(&mut fixture).await;
    let before = fixture.state.requests.lock().unwrap().len();

    let settled = receiver
        .settle(ReceiverConfirmationCustody::PossiblyDelivered)
        .await;

    let requests = fixture.state.requests.lock().unwrap();
    assert!(
        matches!(settled, Ok(ReceiverCleanup::Acknowledged)),
        "retained custody must obtain an ACK from the original receiver after dropping its transport. Failure: {:?}; runtime requests sent: {}",
        authority.failure.get(),
        requests.len() - before,
    );
    let [revoke] = &requests[before..] else {
        panic!("expected exactly one revoke, with no renewal or replay");
    };
    let (head, body) = revoke
        .split_once("\r\n\r\n")
        .expect("complete revoke request");
    let original_owner = authority.owner();
    let route = format!(
        "/runtime/agents/{}/manager-ownership/revoke",
        original_owner.agent_id
    );
    let mut request_line = head
        .lines()
        .next()
        .expect("request line")
        .split_ascii_whitespace();
    assert_eq!(request_line.next(), Some("POST"));
    assert!(
        request_line.next() == Some(route.as_str()),
        "revoke must target the original agent"
    );
    let revoked: TransportOwner = serde_json::from_str(body).expect("revoke owner payload");
    assert!(
        revoked == original_owner,
        "revoke must carry the exact original owner"
    );
}

async fn assert_post_drop_incarnation_refused(replace: impl FnOnce(&State)) {
    let mut fixture = fixture().await;
    let (_authority, receiver) = confirmed_receiver_after_transport_drop(&mut fixture).await;
    replace(&fixture.state);
    let reads_before = fixture.state.pod_reads.load(Ordering::SeqCst);
    let requests_before = fixture.state.requests.lock().unwrap().len();

    let settled = receiver
        .settle(ReceiverConfirmationCustody::PossiblyDelivered)
        .await;

    assert!(
        settled.is_err(),
        "replacement cannot settle the original receiver's custody"
    );
    assert_eq!(
        fixture.state.requests.lock().unwrap().len(),
        requests_before,
        "replacement must receive no ownership request"
    );
    assert!(
        fixture.state.pod_reads.load(Ordering::SeqCst) > reads_before,
        "revoke must reach the native replacement check instead of refusing on the dropped local state"
    );
}

#[tokio::test]
async fn native_revocation_after_transport_drop_refuses_a_replacement_pod() {
    assert_post_drop_incarnation_refused(|state| state.replaced.store(true, Ordering::SeqCst))
        .await;
}

#[tokio::test]
async fn native_revocation_after_transport_drop_refuses_a_restarted_container() {
    assert_post_drop_incarnation_refused(|state| state.restarted.store(true, Ordering::SeqCst))
        .await;
}

//! Real cached Kubernetes request streams, including receiver negotiation.
use super::*;
use crate::hosted::HostedControlTransport;
use crate::owner_protocol::{scope, test_support::TestAuthority, TransportAuthority};

async fn fixture() -> Fixture {
    let fixture = Fixture::start(Mode::Persistent, NativeRuntimeTransport::KubernetesExec).await;
    fixture
        .transport
        .bind_native_runtime(&handle(), "sandbox-one", &"a".repeat(64))
        .await
        .unwrap();
    fixture
}

async fn wait_for_request(fixture: &Fixture, prefix: &str) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while !fixture
            .state
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|request| request.starts_with(prefix))
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("request reached the actual native stream");
}

#[tokio::test]
async fn long_native_control_does_not_starve_periodic_receiver_negotiation() {
    let fixture = fixture().await;
    let endpoint = fixture.endpoint().await;
    let authority = TestAuthority::new(7);
    let lease = scope(
        authority.clone(),
        fixture.transport.receiver_lease(&endpoint),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        fixture.state.ownership_confirmations.load(Ordering::SeqCst),
        1
    );
    fixture.state.hold_response.store(true, Ordering::SeqCst);
    let control = tokio::spawn({
        let transport = fixture.transport.clone();
        let authority = authority.clone();
        async move {
            scope(
                authority,
                transport.exchange(&handle(), RequestFrame::Resources),
            )
            .await
        }
    });
    wait_for_request(&fixture, "POST /runtime/control ").await;
    let confirmations_before = fixture.state.ownership_confirmations.load(Ordering::SeqCst);
    let pod_reads_before = fixture.state.pod_reads.load(Ordering::SeqCst);
    // Exceed both the three-second periodic interval and negotiation timeout.
    // The receiver endpoint stays responsive while the ordinary body is held.
    tokio::time::sleep(Duration::from_millis(7000)).await;
    let current = authority.is_current();
    let confirmations_after = fixture.state.ownership_confirmations.load(Ordering::SeqCst);
    let pod_reads_after = fixture.state.pod_reads.load(Ordering::SeqCst);
    let pool_entries = fixture.transport.state.lock().await.native_http.len();
    assert!(
        !control.is_finished(),
        "ordinary control must still hold its response"
    );
    fixture.state.hold_response.store(false, Ordering::SeqCst);
    let control_result = tokio::time::timeout(Duration::from_secs(2), control)
        .await
        .unwrap()
        .unwrap();
    drop(lease);
    fixture.transport.retire(&handle()).await.unwrap();
    fixture.drained().await;
    assert!(
        current,
        "native control blocked receiver renewal until authority was revoked"
    );
    assert!(
        confirmations_after >= confirmations_before + 2,
        "receiver did not renew while control was held"
    );
    assert!(
        pod_reads_after > pod_reads_before,
        "each ownership send requires a fresh Pod proof"
    );
    assert_eq!(
        pool_entries, 2,
        "separate connections share one bounded transport pool"
    );
    assert!(matches!(
        control_result.unwrap(),
        ResponseFrame::Resources(_)
    ));
    assert_eq!(
        fixture
            .state
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request.starts_with("POST /runtime/control "))
            .count(),
        1,
        "ordinary operation must not be replayed"
    );
}

#[tokio::test]
async fn ownership_connection_rejects_changed_pod_before_sending_request_bytes() {
    for restart in [false, true] {
        let fixture = fixture().await;
        let endpoint = fixture.endpoint().await;
        let authority = TestAuthority::new(7);
        let lease = scope(
            authority.clone(),
            fixture.transport.receiver_lease(&endpoint),
        )
        .await
        .unwrap()
        .unwrap();
        let before = fixture.state.requests.lock().unwrap().len();
        if restart {
            fixture.state.restarted.store(true, Ordering::SeqCst);
        } else {
            fixture.state.replaced.store(true, Ordering::SeqCst);
        }
        assert!(scope(
            authority.clone(),
            fixture.transport.receiver_lease(&endpoint)
        )
        .await
        .is_err());
        assert!(!authority.is_current());
        drop(lease);
        fixture.transport.retire(&handle()).await.unwrap();
        fixture.drained().await;
        assert_eq!(
            fixture.state.requests.lock().unwrap().len(),
            before,
            "stale Pod must receive no new ownership bytes"
        );
    }
}

#[tokio::test]
async fn retirement_cancels_ownership_negotiation_and_preserves_no_replay() {
    let fixture = fixture().await;
    let endpoint = fixture.endpoint().await;
    let authority = TestAuthority::new(7);
    fixture
        .state
        .hold_ownership_response
        .store(true, Ordering::SeqCst);
    let negotiating = tokio::spawn({
        let transport = fixture.transport.clone();
        let authority = authority.clone();
        async move { scope(authority, transport.receiver_lease(&endpoint)).await }
    });
    wait_for_request(
        &fixture,
        "POST /runtime/agents/agent-1/manager-ownership/challenge ",
    )
    .await;
    fixture.transport.retire(&handle()).await.unwrap();
    assert!(tokio::time::timeout(Duration::from_secs(2), negotiating)
        .await
        .unwrap()
        .unwrap()
        .is_err());
    fixture
        .state
        .hold_ownership_response
        .store(false, Ordering::SeqCst);
    fixture.drained().await;
    assert!(!authority.is_current());
    assert!(fixture.transport.state.lock().await.native_http.is_empty());
    assert_eq!(
        fixture.state.requests.lock().unwrap().len(),
        1,
        "retirement must not replay negotiation"
    );
}

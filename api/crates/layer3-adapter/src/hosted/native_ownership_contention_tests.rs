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

#[tokio::test]
async fn ownership_negotiation_respects_shared_request_and_idle_session_limits() {
    let fixture = fixture().await;
    let endpoint = fixture.endpoint().await;
    let permits = fixture
        .transport
        .native_http_budget
        .clone()
        .acquire_many_owned(32)
        .await
        .unwrap();
    let authority = TestAuthority::new(7);
    assert!(matches!(
        scope(authority, fixture.transport.receiver_lease(&endpoint)).await,
        Err(AdapterError::AtCapacity(_))
    ));
    assert!(fixture.state.requests.lock().unwrap().is_empty());
    drop(permits);

    let RuntimeDiscovery::AgentSandbox(client) = &fixture.transport.client else {
        unreachable!()
    };
    let mut handles = Vec::new();
    for index in 0..20 {
        let name = format!("ownership-budget-{index}");
        let handle = RuntimeHandle::new(format!("agent-sandbox:{name}:{}", sandbox_uid(&name)));
        fixture
            .state
            .runtimes
            .lock()
            .unwrap()
            .insert(name.clone(), client.control_token(&name));
        fixture
            .transport
            .bind_native_runtime(&handle, &format!("sandbox-budget-{index}"), &"a".repeat(64))
            .await
            .unwrap();
        let endpoint = fixture.transport.endpoint(&handle).await.unwrap();
        let authority = TestAuthority::new(index + 10);
        let lease = scope(authority, fixture.transport.receiver_lease(&endpoint))
            .await
            .unwrap()
            .unwrap();
        drop(lease);
        fixture
            .transport
            .exchange(&handle, RequestFrame::Resources)
            .await
            .unwrap();
        assert!(
            fixture.transport.state.lock().await.native_http.len()
                <= native_pool::MAX_HTTP_SESSIONS
        );
        handles.push(handle);
    }
    assert_eq!(
        fixture.transport.state.lock().await.native_http.len(),
        native_pool::MAX_HTTP_SESSIONS
    );
    for handle in handles {
        fixture.transport.retire(&handle).await.unwrap();
    }
    fixture.drained().await;
}

#[tokio::test]
async fn ownership_pod_change_cancels_held_control_for_the_old_incarnation() {
    for cached_ownership in [false, true] {
        for restart in [false, true] {
            let fixture = fixture().await;
            let endpoint = fixture.endpoint().await;
            let authority = TestAuthority::new(7);
            let lease = if cached_ownership {
                scope(
                    authority.clone(),
                    fixture.transport.receiver_lease(&endpoint),
                )
                .await
                .unwrap()
            } else {
                None
            };
            fixture.state.hold_response.store(true, Ordering::SeqCst);
            let control = tokio::spawn({
                let transport = fixture.transport.clone();
                async move { transport.exchange(&handle(), RequestFrame::Resources).await }
            });
            wait_for_request(&fixture, "POST /runtime/control ").await;
            let before = fixture.state.requests.lock().unwrap().len();
            if restart {
                fixture.state.restarted.store(true, Ordering::SeqCst);
            } else {
                fixture.state.replaced.store(true, Ordering::SeqCst);
            }
            assert!(
                scope(authority, fixture.transport.receiver_lease(&endpoint))
                    .await
                    .is_err()
            );
            let refused = tokio::time::timeout(Duration::from_secs(2), control).await;
            fixture.state.hold_response.store(false, Ordering::SeqCst);
            drop(lease);
            fixture.transport.retire(&handle()).await.unwrap();
            fixture.drained().await;
            assert!(refused
                .expect("replacement proof must cancel old control before its body arrives")
                .unwrap()
                .is_err());
            assert_eq!(
                fixture.state.requests.lock().unwrap().len(),
                before,
                "replacement must receive no replay or ownership bytes"
            );
        }
    }
}

#[tokio::test]
async fn late_old_incarnation_invalidation_preserves_new_pod_slots() {
    let fixture = fixture().await;
    let old_endpoint = fixture.endpoint().await;
    let old_authority = TestAuthority::new(7);
    let old_lease = scope(
        old_authority.clone(),
        fixture.transport.receiver_lease(&old_endpoint),
    )
    .await
    .unwrap()
    .unwrap();
    let old_slot = fixture
        .transport
        .native_slot_for(&old_endpoint, native_pool::NativeHttpLane::Ownership)
        .await
        .unwrap();
    fixture.state.replaced.store(true, Ordering::SeqCst);
    assert!(scope(
        old_authority,
        fixture.transport.receiver_lease(&old_endpoint)
    )
    .await
    .is_err());
    drop(old_lease);
    let endpoint = fixture.endpoint().await;
    let authority = TestAuthority::new(8);
    let lease = scope(
        authority.clone(),
        fixture.transport.receiver_lease(&endpoint),
    )
    .await
    .unwrap()
    .unwrap();
    fixture
        .transport
        .exchange(&handle(), RequestFrame::Resources)
        .await
        .unwrap();
    let new_slot = fixture
        .transport
        .native_slot_for(&endpoint, native_pool::NativeHttpLane::Ownership)
        .await
        .unwrap();
    fixture
        .transport
        .discard_native_incarnation(&old_endpoint, &old_slot)
        .await;
    assert!(!*new_slot.cancellation().borrow());
    assert_eq!(fixture.transport.state.lock().await.native_http.len(), 2);
    assert!(authority.is_current());
    fixture
        .transport
        .exchange(&handle(), RequestFrame::Resources)
        .await
        .unwrap();
    drop(lease);
    drop(old_slot);
    drop(new_slot);
    fixture.transport.retire(&handle()).await.unwrap();
    fixture.drained().await;
}

#[tokio::test]
async fn incarnation_cancellation_cannot_interrupt_sibling_fencing_at_a_contended_lock() {
    let fixture = fixture().await;
    let endpoint = fixture.endpoint().await;
    let observed = fixture
        .transport
        .native_slot_for(&endpoint, native_pool::NativeHttpLane::Ownership)
        .await
        .unwrap();
    fixture.state.hold_response.store(true, Ordering::SeqCst);
    let control = tokio::spawn({
        let transport = fixture.transport.clone();
        async move { transport.exchange(&handle(), RequestFrame::Resources).await }
    });
    wait_for_request(&fixture, "POST /runtime/control ").await;
    let sibling = fixture.transport.native_slot(&endpoint).await.unwrap();
    let before = fixture.state.requests.lock().unwrap().len();
    let state = fixture.transport.state.lock().await;
    let mut cancelled = observed.cancellation();
    // Reproduce the enclosing native_http_for biased select. Poll twice while
    // the state mutex is held: publishing cancellation before that lock lets
    // the second poll drop invalidation before it can reach the sibling lane.
    let invalidation = async {
        tokio::select! { biased;
            _ = cancelled.wait_for(|cancelled| *cancelled) => {},
            _ = fixture.transport.discard_native_incarnation(&endpoint, &observed) => {},
        }
    };
    tokio::pin!(invalidation);
    assert!(futures_util::poll!(&mut invalidation).is_pending());
    assert!(futures_util::poll!(&mut invalidation).is_pending());
    assert!(!*observed.cancellation().borrow());
    assert!(!*sibling.cancellation().borrow());
    drop(state);
    tokio::time::timeout(Duration::from_secs(2), &mut invalidation)
        .await
        .unwrap();
    assert!(*observed.cancellation().borrow());
    assert!(*sibling.cancellation().borrow());
    let refused = tokio::time::timeout(Duration::from_secs(2), control).await;
    fixture.state.hold_response.store(false, Ordering::SeqCst);
    fixture.transport.retire(&handle()).await.unwrap();
    fixture.drained().await;
    assert!(refused.unwrap().unwrap().is_err());
    assert_eq!(fixture.state.requests.lock().unwrap().len(), before);
}

// The Pod reply barrier drives the real endpoint and cached-native code. It
// never substitutes a mocked transport, authority failure, or fixed sleep for
// observing the replacement Pod. Control has no shared receiver authority, so
// only its own old-incarnation fence can make it fail before body release.
#[tokio::test]
async fn ownership_replacement_proof_fences_held_control_before_body_release() {
    let fixture = fixture().await;
    let endpoint = fixture.endpoint().await;
    let authority = TestAuthority::new(7);
    let initial = scope(
        authority.clone(),
        fixture.transport.receiver_lease(&endpoint),
    )
    .await
    .unwrap()
    .unwrap();
    drop(initial); // Keep the cached connection, stop periodic background reads.
    fixture.state.hold_response.store(true, Ordering::SeqCst);
    let mut control = tokio::spawn({
        let transport = fixture.transport.clone();
        async move { transport.exchange(&handle(), RequestFrame::Resources).await }
    });
    wait_for_request(&fixture, "POST /runtime/control ").await;
    let before = fixture.state.requests.lock().unwrap().len();
    fixture
        .state
        .hold_next_pod_reply
        .store(true, Ordering::SeqCst);
    let renewal = scope(
        authority.clone(),
        fixture.transport.receiver_lease(&endpoint),
    );
    tokio::pin!(renewal);
    tokio::time::timeout(Duration::from_secs(2), async {
        tokio::select! {
            _ = fixture.state.pod_reply_entered.notified() => {},
            _ = &mut renewal => panic!("ownership finished before its fresh Pod read"),
        }
    })
    .await
    .expect("cached Ownership must reach its own fresh Pod proof");
    assert!(!control.is_finished());
    fixture.state.replaced.store(true, Ordering::SeqCst);
    fixture.state.pod_reply_release.notify_one();
    let rejected = tokio::time::timeout(Duration::from_secs(2), &mut renewal)
        .await
        .expect("replacement proof must terminate ownership")
        .is_err();
    let early_control = tokio::time::timeout(Duration::from_secs(2), &mut control).await;
    let fenced_before_release = matches!(&early_control, Ok(Ok(Err(_))));
    // Cleanup precedes the regression assertion, including on the RED source.
    fixture.state.hold_response.store(false, Ordering::SeqCst);
    if early_control.is_err() {
        let _ = tokio::time::timeout(Duration::from_secs(2), &mut control)
            .await
            .expect("released old response must finish")
            .expect("control task must join");
    }
    fixture.transport.retire(&handle()).await.unwrap();
    fixture.drained().await;
    assert!(rejected, "replacement cannot receive stale ownership bytes");
    assert!(
        fenced_before_release,
        "Ownership observed replacement but held Control from the old Pod remained live"
    );
    assert_eq!(
        fixture.state.requests.lock().unwrap().len(),
        before,
        "replacement rejection must not send ownership bytes or replay Control"
    );
}

#[tokio::test]
async fn fresh_ownership_endpoint_fences_control_installed_during_discovery() {
    let fixture = fixture().await;
    let old_endpoint = fixture.endpoint().await;
    let authority = TestAuthority::new(7);
    let initial = scope(
        authority.clone(),
        fixture.transport.receiver_lease(&old_endpoint),
    )
    .await
    .unwrap()
    .unwrap();
    drop(initial);
    // Only OwnershipP1 exists. endpoint() observes no Control hint and must
    // read Kubernetes. Hold that actual read while ControlP1 is installed.
    fixture
        .state
        .hold_next_pod_reply
        .store(true, Ordering::SeqCst);
    let runtime = handle();
    let discovery = fixture.transport.endpoint(&runtime);
    tokio::pin!(discovery);
    tokio::time::timeout(Duration::from_secs(2), async {
        tokio::select! {
            _ = fixture.state.pod_reply_entered.notified() => {},
            _ = &mut discovery => panic!("fresh discovery did not reach its Pod read"),
        }
    })
    .await
    .expect("fresh discovery Pod read must enter the fixture");
    fixture.state.hold_response.store(true, Ordering::SeqCst);
    let mut control = tokio::spawn({
        let transport = fixture.transport.clone();
        async move { transport.exchange(&handle(), RequestFrame::Resources).await }
    });
    wait_for_request(&fixture, "POST /runtime/control ").await;
    assert!(!control.is_finished());
    fixture.state.replaced.store(true, Ordering::SeqCst);
    fixture.state.pod_reply_release.notify_one();
    let new_endpoint = tokio::time::timeout(Duration::from_secs(2), &mut discovery)
        .await
        .expect("replacement discovery must finish")
        .unwrap();
    assert_ne!(old_endpoint.native_binding(), new_endpoint.native_binding());
    // P2 is a freshly discovered endpoint, so new-stream admission succeeds
    // without the cached-P1 mismatch branch. That must still fence ControlP1.
    let replacement = tokio::time::timeout(
        Duration::from_secs(2),
        scope(authority, fixture.transport.receiver_lease(&new_endpoint)),
    )
    .await
    .expect("new incarnation negotiation must finish");
    let admitted = replacement.as_ref().is_ok_and(|lease| lease.is_some());
    drop(replacement);
    let early_control = tokio::time::timeout(Duration::from_secs(2), &mut control).await;
    let fenced_before_release = matches!(&early_control, Ok(Ok(Err(_))));
    fixture.state.hold_response.store(false, Ordering::SeqCst);
    if early_control.is_err() {
        let _ = tokio::time::timeout(Duration::from_secs(2), &mut control)
            .await
            .expect("released old response must finish")
            .expect("control task must join");
    }
    fixture.transport.retire(&handle()).await.unwrap();
    fixture.drained().await;
    assert!(admitted, "fresh replacement must remain usable");
    assert!(
        fenced_before_release,
        "fresh OwnershipP2 admission left held ControlP1 live"
    );
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
        "old Control must never be replayed onto the replacement"
    );
}

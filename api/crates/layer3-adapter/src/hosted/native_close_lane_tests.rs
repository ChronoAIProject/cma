use super::*;
use crate::owner_protocol::{scope, test_support::TestAuthority, TransportAuthority};
use native_pool::NativeHttpLane::{CloseAgent, Ownership};

const CLOSE_ID: &str = "agent:agent-1:close:v1";

fn close_request(epoch: u64) -> RequestFrame {
    RequestFrame::CloseAgent(FencedRequest::new(
        epoch,
        crate::AgentCloseRequest::new(CLOSE_ID, AgentSlot::new("agent-1")),
    ))
}

fn close_requests(fixture: &Fixture) -> usize {
    fixture
        .state
        .requests
        .lock()
        .unwrap()
        .iter()
        .filter(|request| request.contains(CLOSE_ID))
        .count()
}

#[tokio::test]
async fn close_lane_refuses_stale_authority_and_replaced_binding_after_checkout_wait() {
    for replace in [false, true] {
        let fixture =
            Fixture::start(Mode::Persistent, NativeRuntimeTransport::KubernetesExec).await;
        let transport = (*fixture.transport)
            .clone()
            .with_native_close_agent_lane(true);
        let endpoint = fixture.endpoint().await;
        let slot = transport
            .native_slot_for(&endpoint, CloseAgent)
            .await
            .unwrap();
        let held = transport
            .native_checkout(&endpoint, slot.clone())
            .await
            .unwrap();
        let incumbent = TestAuthority::new(7);
        let request = close_request(7);
        let mut close = Box::pin(scope(
            incumbent.clone(),
            transport.control(&endpoint, &request),
        ));
        tokio::time::timeout(Duration::from_secs(2), async {
            tokio::select! {
                result = &mut close => panic!("close bypassed its own connection lock: {result:?}"),
                _ = async {
                    while Arc::strong_count(&slot) <= 3 {
                        tokio::task::yield_now().await;
                    }
                } => {}
            }
        })
        .await
        .expect("close queued behind the dedicated slot's lock");
        if replace {
            fixture.state.replaced.store(true, Ordering::SeqCst);
        } else {
            incumbent.revoke();
        }
        drop(held);
        assert!(tokio::time::timeout(Duration::from_secs(2), close)
            .await
            .unwrap()
            .is_err());
        assert_eq!(close_requests(&fixture), 0);
        assert_eq!(transport.runtime_capacity.available_requests(), 32);

        let successor = TestAuthority::new(8);
        let response = scope(successor, transport.exchange(&handle(), close_request(8)))
            .await
            .unwrap();
        assert!(
            matches!(response, ResponseFrame::CloseAgent(receipt) if receipt.authority_fence == 8)
        );
        assert_eq!(close_requests(&fixture), 1);
        transport.retire(&handle()).await.unwrap();
        fixture.drained().await;
    }
}

#[tokio::test]
async fn close_lane_cancellation_or_timeout_releases_permits_without_resend() {
    for timed_out in [false, true] {
        let fixture = Fixture::start(
            Mode::Persistent,
            NativeRuntimeTransport::KubernetesPortforward,
        )
        .await;
        let transport = (*fixture.transport)
            .clone()
            .with_native_close_agent_lane(true);
        fixture.state.hold_response.store(true, Ordering::SeqCst);
        let runtime = handle();
        let mut close = Box::pin(scope(
            TestAuthority::new(7),
            transport.exchange(&runtime, close_request(7)),
        ));
        tokio::time::timeout(Duration::from_secs(2), async {
            tokio::select! {
                result = &mut close => panic!("close returned while its response was held: {result:?}"),
                _ = async {
                    while close_requests(&fixture) == 0 {
                        tokio::task::yield_now().await;
                    }
                } => {}
            }
        }).await.expect("close reached the receiver");
        assert_eq!(transport.runtime_capacity.available_requests(), 31);
        if timed_out {
            assert!(tokio::time::timeout(Duration::from_millis(20), close)
                .await
                .is_err());
        } else {
            drop(close);
        }
        assert_eq!(transport.runtime_capacity.available_requests(), 32);
        fixture.state.hold_response.store(false, Ordering::SeqCst);
        transport.retire(&handle()).await.unwrap();
        fixture.drained().await;
        assert_eq!(close_requests(&fixture), 1);
    }
}

#[tokio::test]
async fn close_lane_shares_the_physical_session_cap() {
    let (fixture, handles, mut held) = session_reserve::busy_fixture(1).await;
    let transport = (*fixture.transport)
        .clone()
        .with_native_close_agent_lane(true);
    let endpoint = transport.endpoint(&handles[0]).await.unwrap();
    let authority = TestAuthority::new(7);
    // Retain the last slot with the existing ownership lane. Negotiation may
    // use it, but the close must still acquire its own slot within the cap.
    let ownership_slot = transport
        .native_slot_for_class(&endpoint, Ownership, RuntimeWorkloadClass::Control)
        .await
        .unwrap();
    let result = scope(authority, transport.exchange(&handles[0], close_request(7))).await;
    let Err(AdapterError::RuntimeCapacity(failure)) = result else {
        panic!("expected typed native session refusal, got {result:?}");
    };
    assert_eq!(failure.reason, RuntimeCapacityReason::NativeHttpSessions);
    assert_eq!(failure.class, RuntimeWorkloadClass::Control);
    assert_eq!(failure.kind, RuntimePermitKind::Request);
    assert_eq!(transport.state.lock().await.native_http.len(), 16);
    assert!(held.try_join_next().is_none());
    assert_eq!(close_requests(&fixture), 0);
    assert_eq!(transport.runtime_capacity.available_requests(), 17);
    drop(ownership_slot);
    session_reserve::finish(&fixture, handles, held).await;
}
